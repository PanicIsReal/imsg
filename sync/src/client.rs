use crate::cache::{Applied, ChatRow, MessageCache};
use crate::config::SyncConfig;
use crate::uplink::{Uplink, UplinkHandle, UplinkSession};
use anyhow::{Context, Result};
use imsg_proto::event::{BridgeEvent, ContactsState};
use imsg_proto::Envelope;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};

struct AbortPump(tokio::task::AbortHandle);

impl Drop for AbortPump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn bridge_loop(
    config: SyncConfig,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    handle: UplinkHandle,
) -> Result<()> {
    let mut backoff = ReconnectBackoff::default();
    loop {
        let started = Instant::now();
        match connect_and_sync(&config, &cache, &events, &handle).await {
            Ok(()) => warn!("bridge connection closed, reconnecting"),
            Err(e) => warn!("bridge error: {e}"),
        }
        if started.elapsed() >= Duration::from_secs(30) {
            backoff.reset();
        }
        let delay = backoff.next_delay();
        warn!(
            delay_ms = delay.as_millis() as u64,
            "retrying bridge connection"
        );
        tokio::time::sleep(delay).await;
    }
}

struct ReconnectBackoff {
    attempts: u32,
    jitter_seed: u64,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        let bytes = *uuid::Uuid::new_v4().as_bytes();
        Self {
            attempts: 0,
            jitter_seed: u64::from_le_bytes(bytes[..8].try_into().expect("eight uuid bytes")),
        }
    }
}

impl ReconnectBackoff {
    fn next_delay(&mut self) -> Duration {
        let exponent = self.attempts.min(4);
        self.attempts = self.attempts.saturating_add(1);
        let base_ms = 250_u64.saturating_mul(1_u64 << exponent);
        self.jitter_seed ^= self.jitter_seed << 13;
        self.jitter_seed ^= self.jitter_seed >> 7;
        self.jitter_seed ^= self.jitter_seed << 17;
        let jitter_ms = self.jitter_seed % (base_ms / 2 + 1);
        Duration::from_millis((base_ms + jitter_ms).min(5_000))
    }

    fn reset(&mut self) {
        self.attempts = 0;
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

fn contacts_meta(state: ContactsState) -> &'static str {
    match state {
        ContactsState::Unknown => "unknown",
        ContactsState::Unavailable => "unavailable",
        ContactsState::Prompting => "prompting",
        ContactsState::Granted => "granted",
    }
}

async fn persist_contacts(cache: &Arc<RwLock<MessageCache>>, state: ContactsState) -> Result<()> {
    cache
        .write()
        .await
        .set_meta("contacts", contacts_meta(state))
        .await
}

async fn publish_sync_link(
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
) -> Result<()> {
    let payload = cache.read().await.link_snapshot().await?;
    let _ = events.send(Envelope::Event {
        topic: "sync.link".into(),
        payload,
    });
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
) -> Result<Option<i64>> {
    let mut latest_id = None;
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
                if let Some(id) = msg.get("id").and_then(Value::as_i64) {
                    latest_id = Some(latest_id.map_or(id, |current: i64| current.max(id)));
                }
            }
        }
    }
    Ok(latest_id)
}

async fn recover_and_publish(
    uplink: &Uplink,
    config: &SyncConfig,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    reason: &str,
) -> Result<()> {
    if recovery_cursor(cache).await?.is_some() {
        catch_up_after(uplink, cache, config.prefetch_messages).await?;
        prefetch_cache(uplink, config, cache).await?;
    } else {
        let cursor = prefetch_cache(uplink, config, cache).await?.unwrap_or(0);
        set_recovery_cursor(cache, cursor).await?;
    }
    let chats = cache
        .read()
        .await
        .list_chats(config.prefetch_chats.into())
        .await?;
    publish_resync(cache, events, reason, chats).await?;
    Ok(())
}

async fn publish_resync(
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    reason: &str,
    chats: Vec<Value>,
) -> Result<()> {
    let generation = cache
        .read()
        .await
        .get_meta("db_generation")
        .await?
        .unwrap_or_default();
    let _ = events.send(Envelope::Event {
        topic: "sync.resync".into(),
        payload: json!({"reason": reason, "chats": chats, "db_generation": generation}),
    });
    Ok(())
}

async fn catch_up_after(
    uplink: &Uplink,
    cache: &Arc<RwLock<MessageCache>>,
    limit: u32,
) -> Result<()> {
    let limit = limit.max(1);
    for _ in 0..10 {
        let cursor = recovery_cursor(cache).await?.unwrap_or(0);
        let result = uplink
            .call(
                "messages.after",
                json!({"since_rowid": cursor, "limit": limit}),
            )
            .await?;
        let count = apply_catchup_page(cache, cursor, &result).await?;
        if count < limit as usize {
            return Ok(());
        }
    }
    anyhow::bail!("messages.after catch-up exceeded 10 batches; reconnect to continue")
}

async fn apply_catchup_page(
    cache: &Arc<RwLock<MessageCache>>,
    cursor: i64,
    result: &Value,
) -> Result<usize> {
    let messages = result
        .get("messages")
        .and_then(Value::as_array)
        .context("messages.after response missing messages")?
        .clone();
    let count = messages.len();
    let mut advanced_cursor = cursor;
    {
        let guard = cache.write().await;
        for message in messages {
            guard.apply_live_message(&message).await?;
            if let Some(id) = message.get("id").and_then(Value::as_i64) {
                advanced_cursor = advanced_cursor.max(id);
            }
        }
        guard
            .set_meta("watch_rowid", &advanced_cursor.to_string())
            .await?;
    }
    Ok(count)
}

async fn recovery_cursor(cache: &Arc<RwLock<MessageCache>>) -> Result<Option<i64>> {
    let guard = cache.read().await;
    Ok(guard
        .get_meta("watch_rowid")
        .await?
        .and_then(|value| value.parse().ok()))
}

async fn set_recovery_cursor(cache: &Arc<RwLock<MessageCache>>, cursor: i64) -> Result<()> {
    cache
        .read()
        .await
        .set_meta("watch_rowid", &cursor.to_string())
        .await
}

async fn clear_recovery_cursor(cache: &Arc<RwLock<MessageCache>>) -> Result<()> {
    cache.read().await.set_meta("watch_rowid", "").await
}

async fn advance_recovery_cursor(cache: &Arc<RwLock<MessageCache>>, message: &Value) -> Result<()> {
    let Some(id) = message.get("id").and_then(Value::as_i64) else {
        return Ok(());
    };
    let current = recovery_cursor(cache).await?.unwrap_or(0);
    if id > current {
        set_recovery_cursor(cache, id).await?;
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
    let _pump_guard = AbortPump(pump.abort_handle());

    let (initial_generation, buffered_events) =
        await_generation(&mut bridge_events, &mut pump).await?;
    let generation_changed = prepare_generation(cache, &initial_generation).await?;
    if generation_changed {
        publish_resync(cache, events, "db_generation", Vec::new()).await?;
    }

    let recovery_reason = if generation_changed {
        "db_generation"
    } else {
        "reconnect"
    };
    if let Err(error) = recover_and_publish(&uplink, config, cache, events, recovery_reason).await {
        set_link_state(cache, true, false, &link_error_code(&error)).await?;
        return Err(error);
    }
    set_link_state(cache, true, true, "").await?;

    let mut contacts = ContactsLatch::default();
    let mut generation = GenerationLatch {
        last: Some(initial_generation),
    };
    for event in buffered_events {
        apply_bridge_event(
            event,
            &uplink,
            config,
            cache,
            events,
            &mut contacts,
            &mut generation,
        )
        .await?;
    }
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
                        Ok(_) => set_link_state(cache, true, true, "").await?,
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

async fn prepare_generation(cache: &Arc<RwLock<MessageCache>>, generation: &str) -> Result<bool> {
    let previous = cache.read().await.get_meta("db_generation").await?;
    let changed = previous.as_deref().is_some_and(|value| value != generation);
    if changed {
        clear_recovery_cursor(cache).await?;
        cache.write().await.clear_content().await?;
    }
    cache
        .read()
        .await
        .set_meta("db_generation", generation)
        .await?;
    Ok(changed)
}

async fn await_generation(
    bridge_events: &mut tokio::sync::mpsc::Receiver<BridgeEvent>,
    pump: &mut tokio::task::JoinHandle<Result<()>>,
) -> Result<(String, Vec<BridgeEvent>)> {
    let mut buffered = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            result = &mut *pump => {
                return match result {
                    Ok(Ok(())) => anyhow::bail!("bridge closed before generation greeting"),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(error.into()),
                };
            }
            event = bridge_events.recv() => {
                match event.context("bridge events closed before generation greeting")? {
                    BridgeEvent::DbGeneration { generation } => return Ok((generation, buffered)),
                    event => buffered.push(event),
                }
            }
            _ = &mut deadline => anyhow::bail!("timed out awaiting bridge generation greeting"),
        }
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
            let started = Instant::now();
            let applied = {
                let guard = cache.write().await;
                guard.apply_live_message(&payload).await?
            };
            advance_recovery_cursor(cache, &payload).await?;
            let request_id = payload
                .get("id")
                .and_then(|value| value.as_i64())
                .map(|value| value.to_string())
                .unwrap_or_else(|| "bridge_event".into());
            tracing::debug!(
                metric = "cache_commit",
                elapsed_ms = started.elapsed().as_millis() as u64,
                request_id = %request_id,
                "realtime latency"
            );
            let unknown = matches!(applied.chat, ChatRow::Unknown { .. });
            let _ = events.send(sync_message_event(applied));
            if unknown {
                if let Err(e) = reload_chats(uplink, config, cache, events, "unknown_chat").await {
                    warn!("reload chats after unknown chat: {e}");
                }
            }
        }
        BridgeEvent::Contacts(state) => {
            persist_contacts(cache, state).await?;
            if let Err(e) = publish_sync_link(cache, events).await {
                warn!("publish sync.link after contacts: {e}");
            }
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
                clear_recovery_cursor(cache).await?;
                cache.write().await.clear_content().await?;
                let cursor = prefetch_cache(uplink, config, cache).await?.unwrap_or(0);
                set_recovery_cursor(cache, cursor).await?;
                cache.read().await.set_meta("db_generation", &gen).await?;
                set_link_state(cache, true, true, "").await?;
                let chats = cache
                    .read()
                    .await
                    .list_chats(config.prefetch_chats.into())
                    .await?;
                publish_resync(cache, events, "db_generation", chats).await?;
            }
        }
        BridgeEvent::WatchGap { reason } => {
            warn!(
                reason,
                "bridge reported an event gap; refreshing bounded history"
            );
            recover_and_publish(uplink, config, cache, events, "watch_gap").await?;
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

    #[test]
    fn reconnect_backoff_starts_fast_and_is_bounded() {
        let mut backoff = ReconnectBackoff::default();
        let first = backoff.next_delay();
        assert!(first >= Duration::from_millis(250));
        assert!(first < Duration::from_millis(500));
        for _ in 0..20 {
            assert!(backoff.next_delay() <= Duration::from_secs(5));
        }
        backoff.reset();
        assert!(backoff.next_delay() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn recovery_cursor_does_not_jump_to_a_local_send_result() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(RwLock::new(
            MessageCache::open(&dir.path().join("cache.db"))
                .await
                .unwrap(),
        ));
        cache
            .read()
            .await
            .set_meta("watch_rowid", "40")
            .await
            .unwrap();
        cache
            .write()
            .await
            .apply_live_message(&json!({"id": 60, "chat_id": 1}))
            .await
            .unwrap();
        assert_eq!(recovery_cursor(&cache).await.unwrap(), Some(40));
    }

    #[tokio::test]
    async fn recovery_cursor_has_explicit_initial_and_generation_reset_states() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(RwLock::new(
            MessageCache::open(&dir.path().join("cache.db"))
                .await
                .unwrap(),
        ));
        assert_eq!(recovery_cursor(&cache).await.unwrap(), None);
        set_recovery_cursor(&cache, 81).await.unwrap();
        assert_eq!(recovery_cursor(&cache).await.unwrap(), Some(81));
        clear_recovery_cursor(&cache).await.unwrap();
        assert_eq!(recovery_cursor(&cache).await.unwrap(), None);
    }

    #[tokio::test]
    async fn catchup_pages_advance_cursor_and_preserve_every_message() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(RwLock::new(
            MessageCache::open(&dir.path().join("cache.db"))
                .await
                .unwrap(),
        ));
        set_recovery_cursor(&cache, 40).await.unwrap();
        let first = json!({"messages": [
            {"id": 41, "chat_id": 1},
            {"id": 42, "chat_id": 1}
        ]});
        let second = json!({"messages": [{"id": 43, "chat_id": 1}]});
        assert_eq!(apply_catchup_page(&cache, 40, &first).await.unwrap(), 2);
        assert_eq!(recovery_cursor(&cache).await.unwrap(), Some(42));
        assert_eq!(apply_catchup_page(&cache, 42, &second).await.unwrap(), 1);
        assert_eq!(recovery_cursor(&cache).await.unwrap(), Some(43));
        assert_eq!(cache.read().await.message_count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn reconnect_generation_only_clears_cache_when_greeting_changes() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(RwLock::new(
            MessageCache::open(&dir.path().join("cache.db"))
                .await
                .unwrap(),
        ));
        cache
            .write()
            .await
            .apply_live_message(&json!({"id": 9, "chat_id": 1}))
            .await
            .unwrap();
        set_recovery_cursor(&cache, 9).await.unwrap();
        assert!(!prepare_generation(&cache, "first").await.unwrap());
        assert!(!prepare_generation(&cache, "first").await.unwrap());
        assert_eq!(cache.read().await.message_count().await.unwrap(), 1);
        assert!(prepare_generation(&cache, "second").await.unwrap());
        assert_eq!(cache.read().await.message_count().await.unwrap(), 0);
        assert_eq!(recovery_cursor(&cache).await.unwrap(), None);
    }
}
