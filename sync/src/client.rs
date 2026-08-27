use crate::bb::BlueBubbles;
use crate::cache::{Applied, ChatRow, MessageCache};
use crate::config::SyncConfig;
use crate::domain::Message;
use crate::uplink::UplinkHandle;
use anyhow::Result;
use imsg_proto::Envelope;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

pub async fn bridge_loop(
    config: SyncConfig,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    handle: UplinkHandle,
) -> Result<()> {
    loop {
        match connect_and_sync(&config, &cache, &events, &handle).await {
            Ok(()) => warn!("bluebubbles connection closed, reconnecting"),
            Err(e) => warn!("bluebubbles error: {e}, retry in 5s"),
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

async fn prefetch_cache(
    bb: &BlueBubbles,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
) -> Result<()> {
    let chats = bb.query_chats(config.prefetch_chats).await?;
    for chat in chats {
        let guard = cache.write().await;
        guard.upsert_domain_chat(&chat).await?;
        drop(guard);
        let msgs = bb
            .chat_messages(&chat.guid, config.prefetch_messages)
            .await?;
        let guard = cache.write().await;
        for msg in msgs {
            guard.upsert_domain_message(&msg).await?;
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
    let bb = BlueBubbles::connect(config.clone()).await?;
    info!("connected to BlueBubbles");
    handle.attach(Arc::clone(&bb)).await;
    set_link_state(cache, true, false, "").await?;

    match prefetch_cache(&bb, config, cache).await {
        Ok(()) => {
            set_link_state(cache, true, true, "").await?;
            cache
                .write()
                .await
                .set_meta("contacts", "granted")
                .await?;
        }
        Err(e) => {
            warn!("prefetch failed, staying connected: {e}");
            set_link_state(cache, true, false, &link_error_code(&e)).await?;
        }
    }

    let mut sub = Arc::clone(&bb).subscribe();
    let mut retry = tokio::time::interval(std::time::Duration::from_secs(30));
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = &mut sub.pump => {
                return match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(e) => Err(e.into()),
                };
            }
            msg = sub.events.recv() => {
                let Some(msg) = msg else { break };
                apply_live(msg, &bb, config, cache, events).await?;
            }
            _ = retry.tick() => {
                let ready = cache
                    .read()
                    .await
                    .get_meta("database_ready")
                    .await?
                    .is_some_and(|v| v == "true");
                if !ready {
                    match prefetch_cache(&bb, config, cache).await {
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
    bb: &BlueBubbles,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    reason: &str,
) -> Result<()> {
    let chats = bb.query_chats(config.prefetch_chats).await?;
    let mut list = Vec::new();
    {
        let guard = cache.write().await;
        for chat in chats {
            let id = guard.upsert_domain_chat(&chat).await?;
            list.push(chat.to_cache_json(id));
        }
    }
    let _ = events.send(Envelope::Event {
        topic: "sync.chats".into(),
        payload: json!({"reason": reason, "chats": list}),
    });
    Ok(())
}

async fn apply_live(
    msg: Message,
    bb: &BlueBubbles,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
) -> Result<()> {
    let applied = {
        let guard = cache.write().await;
        guard.apply_domain_message(&msg).await?
    };
    let unknown = matches!(applied.chat, ChatRow::Unknown { .. });
    let _ = events.send(sync_message_event(applied));
    if unknown {
        if let Err(e) = reload_chats(bb, config, cache, events, "unknown_chat").await {
            warn!("reload chats after unknown chat: {e}");
        }
    }
    Ok(())
}
