//! SQLite access. One connection behind a mutex; queries run on the blocking pool.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rusqlite::Connection;

use crate::error::{AppError, AppResult};

const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE releases (
    id INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    version TEXT NOT NULL,
    secure_version INTEGER NOT NULL,
    size INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    signature TEXT NOT NULL,
    file_name TEXT NOT NULL,
    build_date TEXT NOT NULL,
    notes TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    UNIQUE (project, version)
);

CREATE TABLE devices (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL DEFAULT '',
    project TEXT NOT NULL,
    version TEXT NOT NULL,
    secure_version INTEGER NOT NULL,
    partition TEXT NOT NULL DEFAULT '',
    ota_state TEXT NOT NULL DEFAULT '',
    ip TEXT NOT NULL DEFAULT '',
    rssi_dbm INTEGER,
    uptime_s INTEGER NOT NULL DEFAULT 0,
    last_ota_error TEXT,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    target_release_id INTEGER REFERENCES releases (id) ON DELETE SET NULL,
    update_state TEXT NOT NULL DEFAULT 'none',
    update_attempts INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE metrics (
    device_id TEXT NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    ts INTEGER NOT NULL,
    value REAL NOT NULL
);
CREATE INDEX metrics_by_series ON metrics (device_id, name, ts);
CREATE INDEX metrics_by_ts ON metrics (ts);

CREATE TABLE latest_metrics (
    device_id TEXT NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    ts INTEGER NOT NULL,
    value REAL NOT NULL,
    PRIMARY KEY (device_id, name)
);

CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    device_id TEXT NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    ts INTEGER NOT NULL,
    kind TEXT NOT NULL,
    message TEXT NOT NULL
);
CREATE INDEX events_by_device ON events (device_id, ts);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    expires_at INTEGER NOT NULL
);
"#,
    r#"
ALTER TABLE devices ADD COLUMN location TEXT NOT NULL DEFAULT '';
ALTER TABLE devices ADD COLUMN poll_interval_s INTEGER NOT NULL DEFAULT 30;

CREATE TABLE alert_rules (
    device_id TEXT NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    metric TEXT NOT NULL,
    min REAL,
    max REAL,
    PRIMARY KEY (device_id, metric)
);

CREATE TABLE active_alerts (
    device_id TEXT NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    metric TEXT NOT NULL,
    value REAL NOT NULL,
    direction TEXT NOT NULL,
    limit_value REAL NOT NULL,
    since INTEGER NOT NULL,
    PRIMARY KEY (device_id, metric)
);
"#,
    r#"
ALTER TABLE devices ADD COLUMN reset_reason TEXT NOT NULL DEFAULT '';
ALTER TABLE devices ADD COLUMN reset_detail TEXT;
"#,
];

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` with the connection on the blocking thread pool.
    pub async fn call<T, F>(&self, f: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> AppResult<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            f(&mut conn)
        })
        .await
        .map_err(|err| AppError::Internal(err.into()))?
    }
}

fn migrate(conn: &Connection) -> anyhow::Result<()> {
    let version: usize = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > MIGRATIONS.len() {
        anyhow::bail!(
            "database schema version {version} is newer than this server ({})",
            MIGRATIONS.len()
        );
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version) {
        conn.execute_batch(&format!(
            "BEGIN; {migration}; PRAGMA user_version = {}; COMMIT;",
            index + 1
        ))
        .with_context(|| format!("database migration {} failed", index + 1))?;
    }
    Ok(())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}
