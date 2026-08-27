use crate::bb::{BbError, BlueBubbles};
use crate::domain::{ChatGuid, ContactBook, Message};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type UplinkError = BbError;

#[derive(Clone, Default)]
pub struct UplinkHandle(Arc<RwLock<Option<Arc<BlueBubbles>>>>);

impl UplinkHandle {
    pub async fn send_text(&self, guid: &ChatGuid, text: &str) -> Result<Message, UplinkError> {
        let bb = self.0.read().await.clone().ok_or(UplinkError::LinkDown)?;
        bb.send_text(guid, text).await
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
