use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use {
    anyhow::Result,
    serde::{Deserialize, Serialize},
};

/// A single session entry in the metadata index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: String,
    pub key: String,
    pub label: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_enabled: Option<bool>,
}

/// JSON file-backed index mapping session key → SessionEntry.
pub struct SessionMetadata {
    path: PathBuf,
    entries: HashMap<String, SessionEntry>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl SessionMetadata {
    /// Load metadata from disk, or create an empty index.
    pub fn load(path: PathBuf) -> Result<Self> {
        let entries = if path.exists() {
            let data = fs::read_to_string(&path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self { path, entries })
    }

    /// Persist metadata to disk.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(&self.entries)?;
        fs::write(&self.path, data)?;
        Ok(())
    }

    /// Get an entry by key.
    pub fn get(&self, key: &str) -> Option<&SessionEntry> {
        self.entries.get(key)
    }

    /// Insert or update an entry. If key doesn't exist, creates a new entry.
    pub fn upsert(&mut self, key: &str, label: Option<String>) -> &SessionEntry {
        let now = now_ms();
        self.entries
            .entry(key.to_string())
            .and_modify(|e| {
                if label.is_some() {
                    e.label = label.clone();
                }
                e.updated_at = now;
            })
            .or_insert_with(|| SessionEntry {
                id: uuid::Uuid::new_v4().to_string(),
                key: key.to_string(),
                label,
                model: None,
                created_at: now,
                updated_at: now,
                message_count: 0,
                project_id: None,
                archived: false,
                worktree_branch: None,
                sandbox_enabled: None,
            })
    }

    /// Update the model associated with a session.
    pub fn set_model(&mut self, key: &str, model: Option<String>) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.model = model;
            entry.updated_at = now_ms();
        }
    }

    /// Update message count and updated_at timestamp.
    pub fn touch(&mut self, key: &str, message_count: u32) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.message_count = message_count;
            entry.updated_at = now_ms();
        }
    }

    /// Set the project_id for a session.
    pub fn set_project_id(&mut self, key: &str, project_id: Option<String>) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.project_id = project_id;
            entry.updated_at = now_ms();
        }
    }

    /// Set the sandbox_enabled override for a session.
    pub fn set_sandbox_enabled(&mut self, key: &str, enabled: Option<bool>) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.sandbox_enabled = enabled;
            entry.updated_at = now_ms();
        }
    }

    /// Remove an entry by key. Returns the removed entry if found.
    pub fn remove(&mut self, key: &str) -> Option<SessionEntry> {
        self.entries.remove(key)
    }

    /// List all entries sorted by updated_at descending.
    pub fn list(&self) -> Vec<SessionEntry> {
        let mut entries: Vec<_> = self.entries.values().cloned().collect();
        entries.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        entries
    }
}

// ── SQLite-backed session metadata ──────────────────────────────────

/// SQLite-backed session metadata store.
pub struct SqliteSessionMetadata {
    pool: sqlx::SqlitePool,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    key: String,
    id: String,
    label: Option<String>,
    model: Option<String>,
    created_at: i64,
    updated_at: i64,
    message_count: i32,
    project_id: Option<String>,
    archived: i32,
    worktree_branch: Option<String>,
    sandbox_enabled: Option<i32>,
}

impl From<SessionRow> for SessionEntry {
    fn from(r: SessionRow) -> Self {
        Self {
            key: r.key,
            id: r.id,
            label: r.label,
            model: r.model,
            created_at: r.created_at as u64,
            updated_at: r.updated_at as u64,
            message_count: r.message_count as u32,
            project_id: r.project_id,
            archived: r.archived != 0,
            worktree_branch: r.worktree_branch,
            sandbox_enabled: r.sandbox_enabled.map(|v| v != 0),
        }
    }
}

impl SqliteSessionMetadata {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// Create the `sessions` table if it doesn't exist.
    pub async fn init(pool: &sqlx::SqlitePool) -> Result<()> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS sessions (
                key             TEXT PRIMARY KEY,
                id              TEXT NOT NULL,
                label           TEXT,
                model           TEXT,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL,
                message_count   INTEGER NOT NULL DEFAULT 0,
                project_id      TEXT REFERENCES projects(id) ON DELETE SET NULL,
                archived        INTEGER NOT NULL DEFAULT 0,
                worktree_branch TEXT,
                sandbox_enabled INTEGER
            )"#,
        )
        .execute(pool)
        .await?;

        // Migrations for columns added after initial release.
        sqlx::query("ALTER TABLE sessions ADD COLUMN sandbox_enabled INTEGER")
            .execute(pool)
            .await
            .ok(); // ignore if column already exists

        Ok(())
    }

    pub async fn get(&self, key: &str) -> Option<SessionEntry> {
        match sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
        {
            Ok(row) => row.map(Into::into),
            Err(e) => {
                tracing::error!("sessions.get failed: {e}");
                None
            },
        }
    }

    /// Insert or update an entry. Returns the entry.
    pub async fn upsert(
        &self,
        key: &str,
        label: Option<String>,
    ) -> Result<SessionEntry, sqlx::Error> {
        let now = now_ms() as i64;
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"INSERT INTO sessions (key, id, label, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(key) DO UPDATE SET
                 label = COALESCE(excluded.label, sessions.label),
                 updated_at = excluded.updated_at"#,
        )
        .bind(key)
        .bind(&id)
        .bind(&label)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get(key).await.ok_or_else(|| sqlx::Error::RowNotFound)
    }

    pub async fn set_model(&self, key: &str, model: Option<String>) {
        let now = now_ms() as i64;
        sqlx::query("UPDATE sessions SET model = ?, updated_at = ? WHERE key = ?")
            .bind(&model)
            .bind(now)
            .bind(key)
            .execute(&self.pool)
            .await
            .ok();
    }

    pub async fn touch(&self, key: &str, message_count: u32) {
        let now = now_ms() as i64;
        sqlx::query("UPDATE sessions SET message_count = ?, updated_at = ? WHERE key = ?")
            .bind(message_count as i32)
            .bind(now)
            .bind(key)
            .execute(&self.pool)
            .await
            .ok();
    }

    pub async fn set_project_id(&self, key: &str, project_id: Option<String>) {
        let now = now_ms() as i64;
        sqlx::query("UPDATE sessions SET project_id = ?, updated_at = ? WHERE key = ?")
            .bind(&project_id)
            .bind(now)
            .bind(key)
            .execute(&self.pool)
            .await
            .ok();
    }

    pub async fn set_sandbox_enabled(&self, key: &str, enabled: Option<bool>) {
        let now = now_ms() as i64;
        let val = enabled.map(|b| b as i32);
        sqlx::query("UPDATE sessions SET sandbox_enabled = ?, updated_at = ? WHERE key = ?")
            .bind(val)
            .bind(now)
            .bind(key)
            .execute(&self.pool)
            .await
            .ok();
    }

    pub async fn remove(&self, key: &str) -> Option<SessionEntry> {
        let entry = self.get(key).await;
        sqlx::query("DELETE FROM sessions WHERE key = ?")
            .bind(key)
            .execute(&self.pool)
            .await
            .ok();
        entry
    }

    pub async fn list(&self) -> Vec<SessionEntry> {
        sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions ORDER BY created_at ASC")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// No-op — SQLite auto-persists.
    pub fn save(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        let mut meta = SessionMetadata::load(path.clone()).unwrap();

        meta.upsert("main", None);
        meta.upsert("session:abc", Some("My Chat".to_string()));

        let list = meta.list();
        assert_eq!(list.len(), 2);
        let keys: Vec<&str> = list.iter().map(|e| e.key.as_str()).collect();
        assert!(keys.contains(&"main"));
        assert!(keys.contains(&"session:abc"));
        let abc = list.iter().find(|e| e.key == "session:abc").unwrap();
        assert_eq!(abc.label.as_deref(), Some("My Chat"));
    }

    #[test]
    fn test_save_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");

        {
            let mut meta = SessionMetadata::load(path.clone()).unwrap();
            meta.upsert("main", Some("Main".to_string()));
            meta.save().unwrap();
        }

        let meta = SessionMetadata::load(path).unwrap();
        let entry = meta.get("main").unwrap();
        assert_eq!(entry.label.as_deref(), Some("Main"));
    }

    #[test]
    fn test_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        let mut meta = SessionMetadata::load(path).unwrap();

        meta.upsert("main", None);
        assert!(meta.get("main").is_some());
        meta.remove("main");
        assert!(meta.get("main").is_none());
    }

    async fn sqlite_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        // sessions table references projects, so create a stub projects table.
        sqlx::query("CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        SqliteSessionMetadata::init(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_sqlite_upsert_and_list() {
        let pool = sqlite_pool().await;
        let meta = SqliteSessionMetadata::new(pool);

        meta.upsert("main", None).await.unwrap();
        meta.upsert("session:abc", Some("My Chat".to_string()))
            .await
            .unwrap();

        let list = meta.list().await;
        assert_eq!(list.len(), 2);
        let abc = list.iter().find(|e| e.key == "session:abc").unwrap();
        assert_eq!(abc.label.as_deref(), Some("My Chat"));
    }

    #[tokio::test]
    async fn test_sqlite_remove() {
        let pool = sqlite_pool().await;
        let meta = SqliteSessionMetadata::new(pool);

        meta.upsert("main", None).await.unwrap();
        assert!(meta.get("main").await.is_some());
        meta.remove("main").await;
        assert!(meta.get("main").await.is_none());
    }

    #[tokio::test]
    async fn test_sqlite_touch() {
        let pool = sqlite_pool().await;
        let meta = SqliteSessionMetadata::new(pool);

        meta.upsert("main", None).await.unwrap();
        meta.touch("main", 5).await;
        assert_eq!(meta.get("main").await.unwrap().message_count, 5);
    }

    #[test]
    fn test_touch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        let mut meta = SessionMetadata::load(path).unwrap();

        meta.upsert("main", None);
        meta.touch("main", 5);
        assert_eq!(meta.get("main").unwrap().message_count, 5);
    }

    #[test]
    fn test_sandbox_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        let mut meta = SessionMetadata::load(path.clone()).unwrap();

        meta.upsert("main", None);
        assert!(meta.get("main").unwrap().sandbox_enabled.is_none());

        meta.set_sandbox_enabled("main", Some(true));
        assert_eq!(meta.get("main").unwrap().sandbox_enabled, Some(true));

        meta.set_sandbox_enabled("main", None);
        assert!(meta.get("main").unwrap().sandbox_enabled.is_none());

        // Verify it round-trips through save/load.
        meta.set_sandbox_enabled("main", Some(false));
        meta.save().unwrap();
        let reloaded = SessionMetadata::load(path).unwrap();
        assert_eq!(reloaded.get("main").unwrap().sandbox_enabled, Some(false));
    }

    #[test]
    fn test_sandbox_enabled_serde_compat() {
        // Existing metadata without sandbox_enabled should deserialize fine.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        std::fs::write(
            &path,
            r#"{"main":{"id":"1","key":"main","label":null,"created_at":0,"updated_at":0,"message_count":0}}"#,
        )
        .unwrap();
        let meta = SessionMetadata::load(path).unwrap();
        assert!(meta.get("main").unwrap().sandbox_enabled.is_none());
    }
}
