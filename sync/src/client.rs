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

pub(crate) async fn run_generation(
    link: Arc<Link>,
    creds: Credentials,
    cache: Arc<RwLock<MessageCache>>,
    events: broadcast::Sender<Envelope>,
    gen: u64,
    mut wake: watch::Receiver<u64>,
) {
    let handle = link.uplink();
    loop {
        if *wake.borrow() != gen {
            return;
        }
        link.set_connecting(true);
        emit_sync_link(&events, &cache, &link.view().await).await;
        tokio::select! {
            result = connect_and_sync(&creds, &cache, &events, &handle, &link, gen, wake.clone()) => {
                match result {
                    Ok(()) => warn!("bluebubbles connection closed, reconnecting"),
                    Err(e) => warn!("bluebubbles error: {e}, retry in 5s"),
                }
            }
            _ = wait_new_gen(&mut wake, gen) => {
                handle.detach().await;
                return;
            }
        }
        if *wake.borrow() != gen {
            return;
        }
        link.set_connecting(false);
        emit_sync_link(&events, &cache, &link.view().await).await;
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
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
) -> Result<()> {
    let result = tokio::select! {
        r = connect_and_sync_inner(creds, cache, events, handle, link, gen, wake.clone()) => r,
        _ = wait_new_gen(&mut wake, gen) => Ok(()),
    };
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
) -> Result<()> {
    if *wake.borrow() != gen {
        return Ok(());
    }
    let bb = BlueBubbles::connect(creds.clone()).await?;
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

    if let Ok(msgs) = bb.recent_messages(40).await {
        for msg in msgs {
            apply_live(msg, &bb, creds, cache, events).await?;
        }
    }

    let (tx, mut rx) = mpsc::channel::<HookEvent>(64);
    let mut server = tokio::spawn(webhook::serve(
        listener,
        token.as_str().to_string(),
        tx,
    ));
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
        Err(e) => {
            warn!("webhook fetch failed: {e}");
            Ok(())
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
