use crate::bb::BlueBubbles;
use crate::cache::{Applied, ChatRow, MessageCache};
use crate::domain::{ContactBook, Message};
use crate::link::{emit_sync_link, Credentials, Link};
use crate::uplink::UplinkHandle;
use crate::webhook::{self, HookEvent};
use anyhow::Result;
use imsg_proto::Envelope;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch, RwLock};
use tracing::{info, warn};

const RETRY_MIN: std::time::Duration = std::time::Duration::from_millis(250);
const RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(30);

struct RetryBackoff {
    failures: u32,
    entropy: u64,
}

impl RetryBackoff {
    fn new() -> Self {
        let entropy = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self {
            failures: 0,
            entropy,
        }
    }

    fn reset(&mut self) {
        self.failures = 0;
    }

    fn next_delay(&mut self) -> std::time::Duration {
        let shift = self.failures.min(7);
        let base = RETRY_MIN.saturating_mul(1 << shift).min(RETRY_MAX);
        self.failures = self.failures.saturating_add(1);
        self.entropy ^= self.entropy << 13;
        self.entropy ^= self.entropy >> 7;
        self.entropy ^= self.entropy << 17;
        let jitter = base / 4 * (self.entropy % 5) as u32 / 4;
        (base + jitter).min(RETRY_MAX)
    }
}

pub(crate) async fn run_generation(
    link: Arc<Link>,
    creds: Credentials,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    gen: u64,
    mut wake: watch::Receiver<u64>,
) {
    let handle = link.uplink();
    let mut retry = RetryBackoff::new();
    loop {
        if *wake.borrow() != gen {
            return;
        }
        link.set_connecting(true);
        emit_sync_link(&events, &cache, &link.view().await).await;
        match connect_and_sync(
            &creds,
            &cache,
            &events,
            &handle,
            &link,
            gen,
            wake.clone(),
            &mut retry,
        )
        .await
        {
            Ok(()) => warn!("bluebubbles connection closed, reconnecting"),
            Err(e) => warn!("bluebubbles error: {e}"),
        }
        if *wake.borrow() != gen {
            return;
        }
        link.set_connecting(false);
        emit_sync_link(&events, &cache, &link.view().await).await;
        let delay = retry.next_delay();
        warn!("retrying BlueBubbles in {}ms", delay.as_millis());
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = wait_new_gen(&mut wake, gen) => return,
        }
    }
}

async fn wait_new_gen(wake: &mut watch::Receiver<u64>, gen: u64) {
    loop {
        if *wake.borrow() != gen {
            return;
        }
        if wake.changed().await.is_err() {
            return;
        }
    }
}

async fn connect_and_sync(
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    handle: &UplinkHandle,
    link: &Arc<Link>,
    gen: u64,
    mut wake: watch::Receiver<u64>,
    retry: &mut RetryBackoff,
) -> Result<()> {
    let result = tokio::select! {
        r = connect_and_sync_inner(creds, cache, events, handle, link, gen, wake.clone(), retry) => r,
        _ = wait_new_gen(&mut wake, gen) => Ok(()),
    };
    link.set_webhook_listening(false);
    handle.detach().await;
    let _ = set_link_state(cache, false, false, &last_error(&result)).await;
    emit_sync_link(events, cache, &link.view().await).await;
    result
}

fn last_error(result: &Result<()>) -> String {
    match result {
        Ok(()) => String::new(),
        Err(e) => e.to_string(),
    }
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

async fn prefetch_cache(
    bb: &BlueBubbles,
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
) -> Result<()> {
    let mut chats = bb.query_chats(creds.public.prefetch_chats).await?;
    let book = bind_names(bb, cache, &mut chats).await?;
    for chat in chats {
        let guard = cache.write().await;
        guard.upsert_domain_chat(&chat).await?;
        drop(guard);
        let msgs = bb
            .chat_messages(&chat.guid, creds.public.prefetch_messages)
            .await?;
        let guard = cache.write().await;
        for mut msg in msgs {
            msg.apply_contacts(&book);
            guard.upsert_domain_message(&msg).await?;
        }
    }
    cache.write().await.apply_contact_book(&book).await?;
    Ok(())
}

async fn bind_names(
    bb: &BlueBubbles,
    cache: &Arc<RwLock<MessageCache>>,
    chats: &mut [crate::domain::Chat],
) -> Result<crate::domain::ContactBook> {
    let mut book = match bb.query_contacts().await {
        Ok(book) => book,
        Err(e) => {
            warn!("contacts fetch failed: {e}");
            ContactBook::default()
        }
    };
    for chat in chats.iter() {
        book.seed_from_chat(chat);
    }
    if let Ok(cached) = cache.read().await.list_chats(500).await {
        for chat in cached {
            book.seed_from_cache_chat(&chat);
        }
    }
    for chat in chats.iter_mut() {
        chat.apply_contacts(&book);
    }
    bb.replace_contacts(book.clone()).await;
    let label = if book.is_empty() {
        "unavailable"
    } else {
        "granted"
    };
    cache.write().await.set_meta("contacts", label).await?;
    Ok(book)
}

async fn connect_and_sync_inner(
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    handle: &UplinkHandle,
    link: &Arc<Link>,
    gen: u64,
    wake: watch::Receiver<u64>,
    retry: &mut RetryBackoff,
) -> Result<()> {
    if *wake.borrow() != gen {
        return Ok(());
    }
    let bb = BlueBubbles::connect(creds.clone()).await?;
    retry.reset();
    info!("connected to BlueBubbles");
    handle.attach(Arc::clone(&bb)).await;
    link.set_connecting(false);
    set_link_state(cache, true, true, "").await?;
    emit_sync_link(events, cache, &link.view().await).await;

    let prefetch_ok = match prefetch_cache(&bb, creds, cache).await {
        Ok(()) => true,
        Err(e) => {
            warn!("prefetch failed, staying connected: {e}");
            false
        }
    };

    if creds.public.webhook_enabled {
        live_webhook(bb, creds, cache, events, link, gen, wake, prefetch_ok).await
    } else {
        let _ = handle.webhook_clear_ours().await;
        link.set_webhook_listening(false);
        live_poll(bb, creds, cache, events, gen, wake, prefetch_ok).await
    }
}

async fn live_poll(
    bb: Arc<BlueBubbles>,
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    gen: u64,
    mut wake: watch::Receiver<u64>,
    mut prefetch_ok: bool,
) -> Result<()> {
    let mut sub = Arc::clone(&bb).subscribe();
    let mut retry = tokio::time::interval(std::time::Duration::from_secs(30));
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if *wake.borrow() != gen {
            sub.pump.abort();
            return Ok(());
        }
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
                apply_live(msg, &bb, creds, cache, events).await?;
            }
            _ = retry.tick() => {
                if !prefetch_ok {
                    match prefetch_cache(&bb, creds, cache).await {
                        Ok(()) => prefetch_ok = true,
                        Err(e) => warn!("prefetch retry failed: {e}"),
                    }
                }
            }
            _ = wait_new_gen(&mut wake, gen) => {
                sub.pump.abort();
                return Ok(());
            }
        }
    }
    Ok(())
}

async fn live_webhook(
    bb: Arc<BlueBubbles>,
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    link: &Arc<Link>,
    gen: u64,
    mut wake: watch::Receiver<u64>,
    mut prefetch_ok: bool,
) -> Result<()> {
    let token = crate::link::store::ensure_webhook_token(link.store_ctx())?;
    let listener = match webhook::bind_local(creds.public.webhook_port).await {
        Ok((listener, addr)) => {
            info!("webhook listening on {addr}");
            listener
        }
        Err(e) => {
            warn!("webhook bind failed, falling back to poll: {e}");
            link.set_webhook_listening(false);
            set_link_state(cache, true, true, &format!("webhook bind failed: {e}")).await?;
            emit_sync_link(events, cache, &link.view().await).await;
            return live_poll(bb, creds, cache, events, gen, wake, prefetch_ok).await;
        }
    };
    link.set_webhook_listening(true);
    emit_sync_link(events, cache, &link.view().await).await;

    let (tx, mut rx) = mpsc::channel::<HookEvent>(64);
    let mut server = tokio::spawn(webhook::serve(listener, token.as_str().to_string(), tx));
    let _server_guard = crate::bb::AbortTask(server.abort_handle());
    // Webhooks provide low latency; reconciliation recovers dropped deliveries.
    let mut recovery = Arc::clone(&bb).subscribe_every(std::time::Duration::from_secs(30));
    let mut retry = tokio::time::interval(std::time::Duration::from_secs(30));
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let result = loop {
        if *wake.borrow() != gen {
            server.abort();
            break Ok(());
        }
        tokio::select! {
            joined = &mut server => {
                break joined.unwrap_or_else(|e| Err(e.into()));
            }
            result = &mut recovery.pump => {
                break match result {
                    Ok(result) => result,
                    Err(e) => Err(e.into()),
                };
            }
            msg = recovery.events.recv() => {
                let Some(msg) = msg else { break Ok(()) };
                apply_live(msg, &bb, creds, cache, events).await?;
            }
            ev = rx.recv() => {
                let Some(ev) = ev else { break Ok(()) };
                if let Err(e) = doorbell(&bb, creds, cache, events, ev).await {
                    warn!("webhook doorbell: {e}");
                }
            }
            _ = retry.tick() => {
                if !prefetch_ok {
                    match prefetch_cache(&bb, creds, cache).await {
                        Ok(()) => prefetch_ok = true,
                        Err(e) => warn!("prefetch retry failed: {e}"),
                    }
                }
            }
            _ = wait_new_gen(&mut wake, gen) => {
                server.abort();
                break Ok(());
            }
        }
    };
    link.set_webhook_listening(false);
    result
}

async fn doorbell(
    bb: &BlueBubbles,
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    ev: HookEvent,
) -> Result<()> {
    let Some(guid) = ev.message_guid else {
        return Ok(());
    };
    match bb.message_by_guid(&guid).await {
        Ok(Some(msg)) => apply_live(msg, bb, creds, cache, events).await,
        Ok(None) => {
            warn!("webhook guid not on server");
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
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
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    reason: &str,
) -> Result<()> {
    let mut chats = bb.query_chats(creds.public.prefetch_chats).await?;
    let book = bind_names(bb, cache, &mut chats).await?;
    let mut list = Vec::new();
    {
        let guard = cache.write().await;
        for chat in chats {
            let id = guard.upsert_domain_chat(&chat).await?;
            list.push(chat.to_cache_json(id));
        }
        let _ = guard.apply_contact_book(&book).await;
    }
    let _ = events.send(Envelope::Event {
        topic: "sync.chats".into(),
        payload: json!({"reason": reason, "chats": list}),
    });
    Ok(())
}

async fn apply_live(
    mut msg: Message,
    bb: &BlueBubbles,
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
) -> Result<()> {
    msg.apply_contacts(&bb.contact_book().await);
    let applied = {
        let guard = cache.write().await;
        guard.apply_domain_message(&msg).await?
    };
    let unknown = matches!(applied.chat, ChatRow::Unknown { .. });
    let _ = events.send(sync_message_event(applied));
    if unknown {
        if let Err(e) = reload_chats(bb, creds, cache, events, "unknown_chat").await {
            warn!("reload chats after unknown chat: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_starts_fast_caps_and_resets() {
        let mut retry = RetryBackoff {
            failures: 0,
            entropy: 1,
        };
        let first = retry.next_delay();
        assert!(first >= RETRY_MIN);
        assert!(first <= RETRY_MIN + RETRY_MIN / 4);

        for _ in 0..32 {
            assert!(retry.next_delay() <= RETRY_MAX);
        }

        retry.reset();
        let after_reset = retry.next_delay();
        assert!(after_reset >= RETRY_MIN);
        assert!(after_reset <= RETRY_MIN + RETRY_MIN / 4);
    }
}
