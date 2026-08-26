use crate::cache::{Applied, ChatRow, MessageCache};
use crate::config::SyncConfig;
use crate::uplink::{Uplink, UplinkHandle, UplinkSession};
use anyhow::Result;
use imsg_proto::event::{BridgeEvent, ContactsState};
use imsg_proto::Envelope;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};

pub async fn bridge_loop(
    config: SyncConfig,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    handle: UplinkHandle,
) -> Result<()> {
    loop {
        match connect_and_sync(&config, &cache, &events, &handle).await {
            Ok(()) => warn!("bridge connection closed, reconnecting"),
            Err(e) => warn!("bridge error: {e}, retry in 5s"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn connect_and_sync(
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    handle: &UplinkHandle,
) -> Result<()> {
    let result = connect_and_sync_inner(config, cache, events, handle).await;
    handle.detach().await;
    let _ = set_link_state(cache, false, false, "").await;
    result
}

async fn set_link_state(
    cache: &Arc<RwLock<MessageCache>>,
    bridge_connected: bool,
    database_ready: bool,
    last_error: &str,
) -> Result<()> {
    let guard = cache.write().await;
    guard
        .set_meta(
            "bridge_connected",
            if bridge_connected { "true" } else { "false" },
        )
        .await?;
    guard
        .set_meta(
            "database_ready",
            if database_ready { "true" } else { "false" },
        )
        .await?;
    guard.set_meta("last_error", last_error).await?;
    Ok(())
}

fn link_error_code(err: &impl ToString) -> String {
    let s = err.to_string();
    if s.contains("Database unavailable") || s.contains("Full Disk Access") {
        "database_unavailable".into()
    } else {
        s
    }
}

#[derive(Default)]
struct ContactsLatch {
    last: ContactsState,
}

impl ContactsLatch {
    /// Rising-edge only: `Unavailable → Granted` is actionable.
    /// `Unknown → Granted` on connect is not — prefetch already ran.
    fn take_rising_grant(&mut self, state: ContactsState) -> bool {
        let rising = self.last == ContactsState::Unavailable && state == ContactsState::Granted;
        self.last = state;
        rising
    }
}

#[derive(Default)]
struct GenerationLatch {
    last: Option<String>,
}

impl GenerationLatch {
    /// First greeting is not a rotation; only a later different generation is.
    fn changed(&mut self, generation: &str) -> bool {
        match &self.last {
            None => {
                self.last = Some(generation.to_string());
                false
            }
            Some(prev) if prev == generation => false,
            Some(_) => {
                self.last = Some(generation.to_string());
                true
            }
        }
    }
}

async fn prefetch_cache(
    uplink: &Uplink,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
) -> Result<()> {
    let chats = uplink
        .call("chats.list", json!({"limit": config.prefetch_chats}))
        .await?;
    let list = chats
        .get("chats")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for chat in list {
        {
            let guard = cache.write().await;
            guard.upsert_chat(&chat).await?;
        }
        let Some(id) = chat.get("id").and_then(|v| v.as_i64()) else {
            continue;
        };
        let hist = uplink
            .call(
                "messages.history",
                json!({"chat_id": id, "limit": config.prefetch_messages}),
            )
            .await?;
        if let Some(msgs) = hist.get("messages").and_then(|v| v.as_array()) {
            let guard = cache.write().await;
            for msg in msgs {
                guard.upsert_message(msg).await?;
            }
        }
    }
    Ok(())
}

async fn connect_and_sync_inner(
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    handle: &UplinkHandle,
) -> Result<()> {
    let session = Uplink::connect(config).await?;
    info!("connected to bridge");
    handle.attach(Arc::clone(&session.uplink)).await;
    set_link_state(cache, true, false, "").await?;
    run_session(session, config, cache, events).await
}

async fn run_session(
    session: UplinkSession,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
) -> Result<()> {
    let UplinkSession {
        uplink,
        events: mut bridge_events,
        mut pump,
    } = session;

    match prefetch_cache(&uplink, config, cache).await {
        Ok(()) => set_link_state(cache, true, true, "").await?,
        Err(e) => {
            warn!("prefetch failed, staying connected: {e}");
            set_link_state(cache, true, false, &link_error_code(&e)).await?;
        }
    }

    let mut contacts = ContactsLatch::default();
    let mut generation = GenerationLatch::default();
    let mut retry = tokio::time::interval(std::time::Duration::from_secs(30));
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = &mut pump => {
                return match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(e.into()),
                };
            }
            evt = bridge_events.recv() => {
                let Some(evt) = evt else { break };
                apply_bridge_event(
                    evt,
                    &uplink,
                    config,
                    cache,
                    events,
                    &mut contacts,
                    &mut generation,
                )
                .await?;
            }
            _ = retry.tick() => {
                let ready = cache
                    .read()
                    .await
                    .get_meta("database_ready")
                    .await?
                    .is_some_and(|v| v == "true");
                if !ready {
                    match prefetch_cache(&uplink, config, cache).await {
                        Ok(()) => set_link_state(cache, true, true, "").await?,
                        Err(e) => {
                            warn!("prefetch retry failed: {e}");
                            set_link_state(cache, true, false, &link_error_code(&e)).await?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn sync_message_event(applied: Applied) -> Envelope {
    let chat = match applied.chat {
        ChatRow::Updated(v) => Some(v),
        ChatRow::Unknown { .. } => None,
    };
    Envelope::Event {
        topic: "sync.message".into(),
        payload: json!({
            "message": applied.message,
            "chat": chat,
            "is_new": applied.is_new,
        }),
    }
}

async fn reload_chats(
    uplink: &Uplink,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    reason: &str,
) -> Result<()> {
    let result = uplink
        .call("chats.list", json!({"limit": config.prefetch_chats}))
        .await?;
    let list = result
        .get("chats")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    {
        let guard = cache.write().await;
        for chat in &list {
            guard.upsert_chat(chat).await?;
        }
    }
    let _ = events.send(Envelope::Event {
        topic: "sync.chats".into(),
        payload: json!({"reason": reason, "chats": list}),
    });
    Ok(())
}

async fn apply_bridge_event(
    evt: BridgeEvent,
    uplink: &Uplink,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    contacts: &mut ContactsLatch,
    generation: &mut GenerationLatch,
) -> Result<()> {
    match evt {
        BridgeEvent::Message(payload) => {
            let applied = {
                let guard = cache.write().await;
                guard.apply_live_message(&payload).await?
            };
            let unknown = matches!(applied.chat, ChatRow::Unknown { .. });
            let _ = events.send(sync_message_event(applied));
            if unknown {
                if let Err(e) = reload_chats(uplink, config, cache, events, "unknown_chat").await {
                    warn!("reload chats after unknown chat: {e}");
                }
            }
        }
        BridgeEvent::Contacts(state) => {
            if contacts.take_rising_grant(state) {
                if let Err(e) =
                    reload_chats(uplink, config, cache, events, "contacts_granted").await
                {
                    warn!("reload chats after contacts grant: {e}");
                }
            }
        }
        BridgeEvent::DbGeneration { generation: gen } => {
            if generation.changed(&gen) {
                match prefetch_cache(uplink, config, cache).await {
                    Ok(()) => {
                        set_link_state(cache, true, true, "").await?;
                        if let Err(e) =
                            reload_chats(uplink, config, cache, events, "db_generation_changed")
                                .await
                        {
                            warn!("reload chats after db generation: {e}");
                        }
                    }
                    Err(e) => {
                        warn!("prefetch after db generation failed: {e}");
                        set_link_state(cache, true, false, &link_error_code(&e)).await?;
                    }
                }
            }
        }
        BridgeEvent::Unknown { topic } => {
            debug!(topic, "ignored bridge topic");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contacts_latch_only_fires_on_unavailable_to_granted() {
        let mut latch = ContactsLatch::default();
        assert!(!latch.take_rising_grant(ContactsState::Granted));
        assert!(!latch.take_rising_grant(ContactsState::Unavailable));
        assert!(latch.take_rising_grant(ContactsState::Granted));
        assert!(!latch.take_rising_grant(ContactsState::Granted));
    }

    #[test]
    fn generation_latch_ignores_connect_greeting() {
        let mut latch = GenerationLatch::default();
        assert!(!latch.changed("abc"));
        assert!(!latch.changed("abc"));
        assert!(latch.changed("def"));
        assert!(!latch.changed("def"));
    }
}
