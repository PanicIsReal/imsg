use anyhow::Result;
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub message: Value,
    pub chat: ChatRow,
    pub is_new: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatRow {
    Updated(Value),
    Unknown { chat_id: i64 },
}

pub struct MessageCache {
    pool: SqlitePool,
}

impl MessageCache {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let url = format!("sqlite:{}?mode=rwc", path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chats (
                id INTEGER PRIMARY KEY,
                guid TEXT,
                name TEXT,
                identifier TEXT,
                service TEXT,
                last_message_at TEXT,
                unread_count INTEGER DEFAULT 0,
                participants_json TEXT,
                raw_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY,
                chat_id INTEGER NOT NULL,
                guid TEXT,
                sender TEXT,
                sender_name TEXT,
                text TEXT,
                created_at TEXT,
                is_from_me INTEGER NOT NULL,
                raw_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sync_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_chat_created ON messages(chat_id, created_at);
            "#,
        )
        .execute(&pool)
        .await?;
        let cache = Self { pool };
        cache.repair_message_projections().await?;
        Ok(cache)
    }

    async fn repair_message_projections(&self) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE messages
            SET
              chat_id = COALESCE(
                CAST(json_extract(raw_json, '$.chat_id') AS INTEGER),
                chat_id
              ),
              created_at = COALESCE(json_extract(raw_json, '$.created_at'), created_at),
              is_from_me = CASE
                WHEN json_extract(raw_json, '$.is_from_me') = 1 THEN 1
                WHEN json_extract(raw_json, '$.is_from_me') = 0 THEN 0
                ELSE is_from_me
              END
            WHERE (chat_id = 0 AND json_extract(raw_json, '$.chat_id') IS NOT NULL)
               OR (created_at IS NULL AND json_extract(raw_json, '$.created_at') IS NOT NULL)
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_chat(&self, chat: &Value) -> Result<()> {
        let id = chat["id"].as_i64().unwrap_or(0);
        let raw = serde_json::to_string(chat)?;
        sqlx::query(
            r#"INSERT INTO chats (id, guid, name, identifier, service, last_message_at, unread_count, participants_json, raw_json)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
                 name=excluded.name, last_message_at=excluded.last_message_at,
                 unread_count=excluded.unread_count, raw_json=excluded.raw_json"#,
        )
        .bind(id)
        .bind(chat["guid"].as_str())
        .bind(chat["name"].as_str().or(chat["contact_name"].as_str()))
        .bind(chat["identifier"].as_str())
        .bind(chat["service"].as_str())
        .bind(chat["last_message_at"].as_str())
        .bind(chat["unread_count"].as_i64().unwrap_or(0))
        .bind(chat["participants"].to_string())
        .bind(&raw)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert_message(&self, msg: &Value) -> Result<()> {
        let id = msg["id"].as_i64().unwrap_or(0);
        let existing = load_raw_json(&self.pool, id).await?;
        persist_message(&self.pool, &merge_message(existing, msg)).await
    }

    pub async fn apply_live_message(&self, msg: &Value) -> Result<Applied> {
        let id = msg["id"].as_i64().unwrap_or(0);
        let mut tx = self.pool.begin().await?;

        let existing = load_raw_json(&mut *tx, id).await?;
        let is_new = existing.is_none();
        let merged = merge_message(existing, msg);
        persist_message(&mut *tx, &merged).await?;

        let chat_id = merged["chat_id"].as_i64().unwrap_or(0);
        let created_at = merged["created_at"].as_str();
        let is_from_me = merged["is_from_me"].as_bool().unwrap_or(false);

        let chat_row = sqlx::query("SELECT raw_json FROM chats WHERE id = ?")
            .bind(chat_id)
            .fetch_optional(&mut *tx)
            .await?;

        let chat = match chat_row {
            None => ChatRow::Unknown { chat_id },
            Some(row) => {
                let s: String = row.get("raw_json");
                let mut chat: Value = serde_json::from_str(&s)?;
                if let Some(ts) = later_timestamp(chat["last_message_at"].as_str(), created_at) {
                    chat["last_message_at"] = Value::String(ts);
                }
                let unread = chat["unread_count"].as_i64().unwrap_or(0);
                let unread = if is_new && !is_from_me {
                    unread.saturating_add(1)
                } else {
                    unread
                };
                chat["unread_count"] = Value::from(unread);
                let patched = serde_json::to_string(&chat)?;
                sqlx::query(
                    r#"UPDATE chats SET last_message_at = ?, unread_count = ?, name = ?, raw_json = ? WHERE id = ?"#,
                )
                .bind(chat["last_message_at"].as_str())
                .bind(unread)
                .bind(chat["name"].as_str().or(chat["contact_name"].as_str()))
                .bind(&patched)
                .bind(chat_id)
                .execute(&mut *tx)
                .await?;
                ChatRow::Updated(chat)
            }
        };

        tx.commit().await?;
        Ok(Applied {
            message: merged,
            chat,
            is_new,
        })
    }

    pub async fn list_chats(&self, limit: i64) -> Result<Vec<Value>> {
        let rows = sqlx::query("SELECT raw_json FROM chats ORDER BY last_message_at DESC LIMIT ?")
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                let s: String = r.get("raw_json");
                Ok(serde_json::from_str(&s)?)
            })
            .collect()
    }

    pub async fn list_messages(
        &self,
        chat_id: i64,
        limit: i64,
        before: Option<&str>,
    ) -> Result<Vec<Value>> {
        let rows = if let Some(b) = before {
            sqlx::query(
                "SELECT raw_json FROM messages WHERE chat_id = ? AND created_at < ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(chat_id)
            .bind(b)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT raw_json FROM messages WHERE chat_id = ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(chat_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };
        let mut out: Vec<Value> = rows
            .iter()
            .map(|r| {
                let s: String = r.get("raw_json");
                serde_json::from_str(&s)
            })
            .collect::<Result<_, _>>()?;
        out.reverse();
        Ok(out)
    }

    pub async fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query("INSERT INTO sync_meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM sync_meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<String, _>("value")))
    }

    pub async fn link_snapshot(&self) -> Result<Value> {
        let bridge_connected = self
            .get_meta("bridge_connected")
            .await?
            .is_some_and(|v| v == "true");
        let database_ready = self
            .get_meta("database_ready")
            .await?
            .is_some_and(|v| v == "true");
        let last_error = self.get_meta("last_error").await?.unwrap_or_default();
        let contacts = self
            .get_meta("contacts")
            .await?
            .unwrap_or_else(|| "unknown".into());
        Ok(serde_json::json!({
            "bridge_connected": bridge_connected,
            "database_ready": database_ready,
            "last_error": last_error,
            "contacts": contacts,
        }))
    }

    pub async fn chat_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as c FROM chats")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("c"))
    }

    pub async fn message_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as c FROM messages")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("c"))
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

async fn load_raw_json<'e, E>(exec: E, id: i64) -> Result<Option<Value>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query("SELECT raw_json FROM messages WHERE id = ?")
        .bind(id)
        .fetch_optional(exec)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => {
            let s: String = row.get("raw_json");
            Ok(Some(serde_json::from_str(&s)?))
        }
    }
}

async fn persist_message<'e, E>(exec: E, msg: &Value) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let raw = serde_json::to_string(msg)?;
    sqlx::query(
        r#"INSERT INTO messages (id, chat_id, guid, sender, sender_name, text, created_at, is_from_me, raw_json)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(id) DO UPDATE SET
             chat_id=excluded.chat_id,
             guid=excluded.guid,
             sender=excluded.sender,
             sender_name=excluded.sender_name,
             text=excluded.text,
             created_at=excluded.created_at,
             is_from_me=excluded.is_from_me,
             raw_json=excluded.raw_json"#,
    )
    .bind(msg["id"].as_i64().unwrap_or(0))
    .bind(msg["chat_id"].as_i64().unwrap_or(0))
    .bind(msg["guid"].as_str())
    .bind(msg["sender"].as_str())
    .bind(msg["sender_name"].as_str())
    .bind(msg["text"].as_str())
    .bind(msg["created_at"].as_str())
    .bind(if msg["is_from_me"].as_bool().unwrap_or(false) {
        1
    } else {
        0
    })
    .bind(&raw)
    .execute(exec)
    .await?;
    Ok(())
}

fn merge_message(existing: Option<Value>, incoming: &Value) -> Value {
    let mut out = existing
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(incoming_obj) = incoming.as_object() else {
        return incoming.clone();
    };
    let obj = out.as_object_mut().expect("merged message is an object");
    for (k, v) in incoming_obj {
        if v.is_null() {
            continue;
        }
        if k == "chat_id" && v.as_i64().unwrap_or(0) == 0 {
            continue;
        }
        if k == "created_at" && v.as_str().is_none_or(|s| s.is_empty()) {
            continue;
        }
        obj.insert(k.clone(), v.clone());
    }
    out
}

fn later_timestamp(existing: Option<&str>, incoming: Option<&str>) -> Option<String> {
    match (existing, incoming) {
        (Some(a), Some(b)) => Some(if b > a { b } else { a }.to_string()),
        (Some(a), None) => Some(a.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn cache_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let cache = MessageCache::open(&path).await.unwrap();
        let chat = serde_json::json!({"id": 1, "name": "Test", "identifier": "+1", "service": "iMessage", "last_message_at": "2026-01-01T00:00:00Z"});
        cache.upsert_chat(&chat).await.unwrap();
        let chats = cache.list_chats(10).await.unwrap();
        assert_eq!(chats.len(), 1);
    }

    #[tokio::test]
    async fn apply_live_message_advances_list_chats_last_message_at() {
        let dir = tempdir().unwrap();
        let cache = MessageCache::open(&dir.path().join("cache.db"))
            .await
            .unwrap();
        cache
            .upsert_chat(&serde_json::json!({
                "id": 1,
                "name": "Test",
                "identifier": "+1",
                "service": "iMessage",
                "last_message_at": "2026-01-01T00:00:00Z",
                "unread_count": 0
            }))
            .await
            .unwrap();

        let created_at = "2026-01-02T12:00:00Z";
        cache
            .apply_live_message(&serde_json::json!({
                "id": 10,
                "chat_id": 1,
                "text": "hello",
                "created_at": created_at,
                "is_from_me": false
            }))
            .await
            .unwrap();

        let chats = cache.list_chats(10).await.unwrap();
        assert_eq!(chats[0]["last_message_at"], created_at);
    }

    #[tokio::test]
    async fn apply_live_message_same_id_is_not_new_and_unread_increments_once() {
        let dir = tempdir().unwrap();
        let cache = MessageCache::open(&dir.path().join("cache.db"))
            .await
            .unwrap();
        cache
            .upsert_chat(&serde_json::json!({
                "id": 1,
                "name": "Test",
                "identifier": "+1",
                "service": "iMessage",
                "last_message_at": "2026-01-01T00:00:00Z",
                "unread_count": 0
            }))
            .await
            .unwrap();
        let msg = serde_json::json!({
            "id": 10,
            "chat_id": 1,
            "text": "hello",
            "created_at": "2026-01-02T12:00:00Z",
            "is_from_me": false
        });

        let first = cache.apply_live_message(&msg).await.unwrap();
        assert!(first.is_new);
        let ChatRow::Updated(after_first) = &first.chat else {
            panic!("expected updated chat");
        };
        assert_eq!(after_first["unread_count"], 1);

        let second = cache.apply_live_message(&msg).await.unwrap();
        assert!(!second.is_new);
        let ChatRow::Updated(after_second) = &second.chat else {
            panic!("expected updated chat");
        };
        assert_eq!(after_second["unread_count"], 1);
        assert_eq!(cache.list_chats(10).await.unwrap()[0]["unread_count"], 1);
    }

    #[tokio::test]
    async fn apply_live_message_unknown_chat_still_stores_message() {
        let dir = tempdir().unwrap();
        let cache = MessageCache::open(&dir.path().join("cache.db"))
            .await
            .unwrap();
        let applied = cache
            .apply_live_message(&serde_json::json!({
                "id": 10,
                "chat_id": 99,
                "text": "orphan",
                "created_at": "2026-01-02T12:00:00Z",
                "is_from_me": false
            }))
            .await
            .unwrap();

        assert_eq!(applied.chat, ChatRow::Unknown { chat_id: 99 });
        let messages = cache.list_messages(99, 10, None).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["id"], 10);
        assert_eq!(messages[0]["text"], "orphan");
    }

    #[tokio::test]
    async fn apply_live_message_repairs_stub_columns_from_later_full_event() {
        let dir = tempdir().unwrap();
        let cache = MessageCache::open(&dir.path().join("cache.db"))
            .await
            .unwrap();
        cache
            .upsert_chat(&serde_json::json!({
                "id": 2,
                "name": "Test",
                "identifier": "+1",
                "service": "iMessage",
                "last_message_at": "2026-01-01T00:00:00Z",
                "unread_count": 0
            }))
            .await
            .unwrap();

        cache
            .apply_live_message(&serde_json::json!({
                "id": 163692,
                "guid": "67FD80FB-C6AF-46DF-86E3-85D10B796911",
                "text": "Testing"
            }))
            .await
            .unwrap();

        assert!(
            cache.list_messages(2, 10, None).await.unwrap().is_empty(),
            "thin send result must not be listed under the real chat yet"
        );

        cache
            .apply_live_message(&serde_json::json!({
                "id": 163692,
                "chat_id": 2,
                "guid": "67FD80FB-C6AF-46DF-86E3-85D10B796911",
                "text": "Testing",
                "created_at": "2026-08-26T04:57:12.799Z",
                "is_from_me": true
            }))
            .await
            .unwrap();

        let messages = cache.list_messages(2, 10, None).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], "Testing");
        assert_eq!(messages[0]["chat_id"], 2);
        assert_eq!(messages[0]["is_from_me"], true);
        assert_eq!(messages[0]["created_at"], "2026-08-26T04:57:12.799Z");
    }

    #[tokio::test]
    async fn open_repairs_projected_columns_from_raw_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let cache = MessageCache::open(&path).await.unwrap();
        cache
            .apply_live_message(&serde_json::json!({
                "id": 163692,
                "guid": "abc",
                "text": "Testing"
            }))
            .await
            .unwrap();
        sqlx::query(
            "UPDATE messages SET raw_json = ? WHERE id = 163692",
        )
        .bind(r#"{"id":163692,"chat_id":2,"guid":"abc","text":"Testing","created_at":"2026-08-26T04:57:12.799Z","is_from_me":true}"#)
        .execute(cache.pool())
        .await
        .unwrap();
        drop(cache);

        let reopened = MessageCache::open(&path).await.unwrap();
        let messages = reopened.list_messages(2, 10, None).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], "Testing");
        assert_eq!(messages[0]["is_from_me"], true);
    }

    #[tokio::test]
    async fn apply_live_message_thin_replay_does_not_clear_chat_id() {
        let dir = tempdir().unwrap();
        let cache = MessageCache::open(&dir.path().join("cache.db"))
            .await
            .unwrap();
        cache
            .upsert_chat(&serde_json::json!({
                "id": 2,
                "name": "Test",
                "identifier": "+1",
                "service": "iMessage",
                "last_message_at": "2026-01-01T00:00:00Z",
                "unread_count": 0
            }))
            .await
            .unwrap();
        cache
            .apply_live_message(&serde_json::json!({
                "id": 10,
                "chat_id": 2,
                "text": "Testing",
                "created_at": "2026-08-26T04:57:12.799Z",
                "is_from_me": true
            }))
            .await
            .unwrap();
        cache
            .apply_live_message(&serde_json::json!({
                "id": 10,
                "text": "Testing"
            }))
            .await
            .unwrap();
        let messages = cache.list_messages(2, 10, None).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["chat_id"], 2);
        assert_eq!(messages[0]["is_from_me"], true);
    }
}
