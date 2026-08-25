use anyhow::Result;
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};
use std::path::Path;

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
        Ok(Self { pool })
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
        let chat_id = msg["chat_id"].as_i64().unwrap_or(0);
        let raw = serde_json::to_string(msg)?;
        sqlx::query(
            r#"INSERT INTO messages (id, chat_id, guid, sender, sender_name, text, created_at, is_from_me, raw_json)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET text=excluded.text, raw_json=excluded.raw_json"#,
        )
        .bind(id)
        .bind(chat_id)
        .bind(msg["guid"].as_str())
        .bind(msg["sender"].as_str())
        .bind(msg["sender_name"].as_str())
        .bind(msg["text"].as_str())
        .bind(msg["created_at"].as_str())
        .bind(if msg["is_from_me"].as_bool().unwrap_or(false) { 1 } else { 0 })
        .bind(&raw)
        .execute(&self.pool)
        .await?;
        Ok(())
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

    pub async fn list_messages(&self, chat_id: i64, limit: i64, before: Option<&str>) -> Result<Vec<Value>> {
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
}
