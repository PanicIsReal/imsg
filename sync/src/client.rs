use crate::bb::BlueBubbles;
use crate::cache::{Applied, ChatRow, MessageCache};
use crate::domain::Message;
use crate::link::{emit_sync_link, Credentials, Link};
use crate::uplink::UplinkHandle;
use anyhow::Result;
use imsg_proto::Envelope;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, RwLock};
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
        Err(e) => link_error_code(e),
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
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
) -> Result<()> {
    let mut chats = bb.query_chats(creds.public.prefetch_chats).await?;
    match bb.query_contacts().await {
        Ok(book) if !book.is_empty() => {
            for chat in &mut chats {
                chat.apply_contacts(&book);
            }
            cache
                .write()
                .await
                .set_meta("contacts", "granted")
                .await?;
        }
        Ok(_) => {
            cache
                .write()
                .await
                .set_meta("contacts", "unavailable")
                .await?;
        }
        Err(e) => warn!("contacts fetch failed: {e}"),
    }
    for chat in chats {
        let guard = cache.write().await;
        guard.upsert_domain_chat(&chat).await?;
        drop(guard);
        let msgs = bb
            .chat_messages(&chat.guid, creds.public.prefetch_messages)
            .await?;
        let guard = cache.write().await;
        for msg in msgs {
            guard.upsert_domain_message(&msg).await?;
        }
    }
    Ok(())
}

async fn connect_and_sync_inner(
    creds: &Credentials,
    cache: &Arc<RwLock<MessageCache>>,
    events: &broadcast::Sender<Envelope>,
    handle: &UplinkHandle,
    link: &Arc<Link>,
    gen: u64,
    mut wake: watch::Receiver<u64>,
) -> Result<()> {
    if *wake.borrow() != gen {
        return Ok(());
    }
    let bb = BlueBubbles::connect(creds.clone()).await?;
    info!("connected to BlueBubbles");
    handle.attach(Arc::clone(&bb)).await;
    link.set_connecting(false);
    set_link_state(cache, true, false, "").await?;
    emit_sync_link(events, cache, &link.view().await).await;

    match prefetch_cache(&bb, creds, cache).await {
        Ok(()) => {
            set_link_state(cache, true, true, "").await?;
            emit_sync_link(events, cache, &link.view().await).await;
        }
        Err(e) => {
            warn!("prefetch failed, staying connected: {e}");
            set_link_state(cache, true, false, &link_error_code(&e)).await?;
            emit_sync_link(events, cache, &link.view().await).await;
        }
    }

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
                let ready = cache
                    .read()
                    .await
                    .get_meta("database_ready")
                    .await?
                    .is_some_and(|v| v == "true");
                if !ready {
                    match prefetch_cache(&bb, creds, cache).await {
                        Ok(()) => {
                            set_link_state(cache, true, true, "").await?;
                            emit_sync_link(events, cache, &link.view().await).await;
                        }
                        Err(e) => {
                            warn!("prefetch retry failed: {e}");
                            set_link_state(cache, true, false, &link_error_code(&e)).await?;
                            emit_sync_link(events, cache, &link.view().await).await;
                        }
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
    let chats = bb.query_chats(creds.public.prefetch_chats).await?;
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
