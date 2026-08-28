use crate::bb::{BbError, BlueBubbles};
use crate::domain::{ChatGuid, ContactBook, Message};
use std::path::Path;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type UplinkError = BbError;

#[derive(Clone, Default)]
pub struct UplinkHandle(Arc<RwLock<Option<Arc<BlueBubbles>>>>);

impl UplinkHandle {
    pub async fn mark_read(&self, guid: &ChatGuid) -> Result<(), UplinkError> {
        let Some(bb) = self.0.read().await.clone() else {
            return Ok(());
        };
        bb.mark_read(guid).await
    }

    pub async fn send_text(&self, guid: &ChatGuid, text: &str) -> Result<Message, UplinkError> {
        let bb = self.0.read().await.clone().ok_or(UplinkError::LinkDown)?;
        bb.send_text(guid, text).await
    }

    pub async fn send_attachment(
        &self,
        guid: &ChatGuid,
        identifier: &str,
        path: &Path,
    ) -> Result<Message, UplinkError> {
        let bb = self.0.read().await.clone().ok_or(UplinkError::LinkDown)?;
        bb.send_attachment(guid, identifier, path).await
    }

    pub async fn contact_book(&self) -> Result<ContactBook, UplinkError> {
        let bb = self.0.read().await.clone().ok_or(UplinkError::LinkDown)?;
        bb.query_contacts().await
    }

    pub async fn attach(&self, bb: Arc<BlueBubbles>) {
        *self.0.write().await = Some(bb);
    }

    pub async fn detach(&self) {
        *self.0.write().await = None;
    }

    pub async fn is_up(&self) -> bool {
        self.0.read().await.is_some()
    }

    pub async fn webhook_replace(&self, url: &str) -> Result<(), UplinkError> {
        let bb = self.0.read().await.clone().ok_or(UplinkError::LinkDown)?;
        bb.webhook_replace(url).await
    }

    pub async fn webhook_clear_ours(&self) -> Result<(), UplinkError> {
        let Some(bb) = self.0.read().await.clone() else {
            return Ok(());
        };
        let listed = bb.webhook_list().await.unwrap_or_default();
        for row in listed {
            let existing = row.get("url").and_then(|u| u.as_str()).unwrap_or("");
            if !existing.contains("/imsg/hook") {
                continue;
            }
            if let Some(id) = row
                .get("guid")
                .and_then(|g| g.as_str())
                .or_else(|| row.get("id").and_then(|g| g.as_str()))
            {
                let _ = bb.webhook_delete(id).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detached_handle_fails_fast_with_link_down() {
        let handle = UplinkHandle::default();
        let guid = ChatGuid::parse("iMessage;-;+1").unwrap();
        let err = handle.send_text(&guid, "hi").await.unwrap_err();
        assert!(matches!(err, UplinkError::LinkDown));
        assert_eq!(err.code(), "link_down");
    }
}
