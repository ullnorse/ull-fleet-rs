use std::path::PathBuf;

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::ota::PendingUpdate;

#[derive(Clone)]
pub struct PendingUpdateRepository {
    database_path: PathBuf,
}

impl PendingUpdateRepository {
    pub fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }

    pub fn initialize(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.database_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create SQLite parent directory {}",
                    parent.display()
                )
            })?;
        }

        let conn = self.connection()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pending_update (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                filename TEXT NOT NULL,
                stored_path TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                sha256_hex TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
            [],
        )
        .context("failed to create pending_update table")?;

        Ok(())
    }

    pub fn load_pending_update(&self) -> anyhow::Result<Option<PendingUpdate>> {
        let conn = self.connection()?;
        let pending = conn
            .query_row(
                "SELECT filename, stored_path, size_bytes, sha256_hex, created_at
                 FROM pending_update
                 WHERE id = 1",
                [],
                |row| {
                    Ok(PendingUpdate {
                        filename: row.get(0)?,
                        stored_path: row.get(1)?,
                        size_bytes: row.get(2)?,
                        sha256_hex: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .context("failed to query pending OTA row")?;

        Ok(pending)
    }

    pub fn replace_pending_update(
        &self,
        pending: &PendingUpdate,
    ) -> anyhow::Result<Option<String>> {
        let mut conn = self.connection()?;
        let tx = conn
            .transaction()
            .context("failed to start pending update transaction")?;

        let previous_path = tx
            .query_row(
                "SELECT stored_path FROM pending_update WHERE id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to load previous pending OTA path")?;

        tx.execute(
            "INSERT OR REPLACE INTO pending_update (
                id,
                filename,
                stored_path,
                size_bytes,
                sha256_hex,
                created_at
            ) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![
                &pending.filename,
                &pending.stored_path,
                pending.size_bytes,
                &pending.sha256_hex,
                &pending.created_at,
            ],
        )
        .context("failed to upsert pending OTA row")?;
        tx.commit()
            .context("failed to commit pending OTA transaction")?;

        Ok(previous_path)
    }

    pub fn clear_pending_update(&self, stored_path: &str) -> anyhow::Result<()> {
        let conn = self.connection()?;
        conn.execute(
            "DELETE FROM pending_update WHERE id = 1 AND stored_path = ?1",
            [stored_path],
        )
        .with_context(|| format!("failed to delete pending OTA row for {stored_path}"))?;

        Ok(())
    }

    fn connection(&self) -> anyhow::Result<Connection> {
        Connection::open(&self.database_path).with_context(|| {
            format!(
                "failed to open SQLite database at {}",
                self.database_path.display()
            )
        })
    }
}
