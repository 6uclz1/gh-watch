use std::{collections::HashSet, fs, path::Path, sync::Mutex};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use crate::{
    domain::events::WatchEvent,
    ports::{
        CursorPort, PersistBatchResult, RepoBatchPort, RepoPersistBatch, RetentionPort,
        TimelineQueryPort, TimelineReadMarkPort,
    },
};

const SCHEMA_VERSION: &str = "3";

#[derive(Debug)]
pub struct StateSchemaMismatchError {
    path: String,
}

impl StateSchemaMismatchError {
    fn new(path: &Path) -> Self {
        Self {
            path: path.display().to_string(),
        }
    }
}

impl std::fmt::Display for StateSchemaMismatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "state db schema is incompatible: {} (run `gh-watch init --reset-state`)",
            self.path
        )
    }
}

impl std::error::Error for StateSchemaMismatchError {}

pub struct SqliteStateStore {
    conn: Mutex<Connection>,
}

impl SqliteStateStore {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create state dir: {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("failed to open sqlite db: {}", path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;

        Self::ensure_schema(path, &conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn ensure_schema(path: &Path, conn: &Connection) -> Result<()> {
        if !Self::has_non_internal_tables(conn)? {
            Self::init_schema_v3(conn)?;
            return Ok(());
        }

        if !Self::has_compatible_schema(conn)? {
            return Err(StateSchemaMismatchError::new(path).into());
        }

        Ok(())
    }

    fn has_non_internal_tables(conn: &Connection) -> Result<bool> {
        let has_any: i64 = conn.query_row(
            "
SELECT EXISTS(
  SELECT 1
  FROM sqlite_master
  WHERE type = 'table'
    AND name NOT LIKE 'sqlite_%'
)
",
            [],
            |row| row.get(0),
        )?;
        Ok(has_any == 1)
    }

    fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
        let exists: i64 = conn.query_row(
            "
SELECT EXISTS(
  SELECT 1
  FROM sqlite_master
  WHERE type = 'table' AND name = ?1
)
",
            params![table],
            |row| row.get(0),
        )?;
        Ok(exists == 1)
    }

    fn has_compatible_schema(conn: &Connection) -> Result<bool> {
        if !Self::table_exists(conn, "schema_meta")? {
            return Ok(false);
        }

        let version: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if version.as_deref() != Some(SCHEMA_VERSION) {
            return Ok(false);
        }

        for table in ["polling_cursors_v2", "event_log_v2"] {
            if !Self::table_exists(conn, table)? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn init_schema_v3(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "
CREATE TABLE IF NOT EXISTS schema_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS polling_cursors_v2 (
  repo TEXT PRIMARY KEY,
  last_polled_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS event_log_v2 (
  event_key TEXT PRIMARY KEY,
  repo TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  observed_at TEXT NOT NULL,
  delivered_at TEXT,
  read_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_event_log_v2_created_at
ON event_log_v2 (created_at DESC);
",
        )?;

        conn.execute(
            "
INSERT INTO schema_meta (key, value)
VALUES ('schema_version', ?1)
ON CONFLICT(key) DO UPDATE SET value = excluded.value
",
            params![SCHEMA_VERSION],
        )?;

        Ok(())
    }

    fn parse_watch_event_payload(payload: String) -> Result<WatchEvent> {
        Ok(serde_json::from_str(&payload)?)
    }
}

impl CursorPort for SqliteStateStore {
    fn get_cursor(&self, repo: &str) -> Result<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let value: Option<String> = conn
            .query_row(
                "SELECT last_polled_at FROM polling_cursors_v2 WHERE repo = ?1",
                params![repo],
                |row| row.get(0),
            )
            .optional()?;

        value
            .map(|v| DateTime::parse_from_rfc3339(&v).map(|dt| dt.with_timezone(&Utc)))
            .transpose()
            .map_err(Into::into)
    }

    fn set_cursor(&self, repo: &str, at: DateTime<Utc>) -> Result<()> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        conn.execute(
            "
INSERT INTO polling_cursors_v2 (repo, last_polled_at)
VALUES (?1, ?2)
ON CONFLICT(repo) DO UPDATE SET last_polled_at = excluded.last_polled_at
",
            params![repo, at.to_rfc3339()],
        )?;
        Ok(())
    }
}

impl TimelineQueryPort for SqliteStateStore {
    fn load_timeline_events(&self, limit: usize) -> Result<Vec<WatchEvent>> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let mut stmt = conn.prepare(
            "
SELECT payload_json
FROM event_log_v2
ORDER BY created_at DESC
LIMIT ?1
",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| row.get::<_, String>(0))?;
        rows.map(|row| Self::parse_watch_event_payload(row?))
            .collect::<Result<Vec<_>>>()
    }

    fn load_read_event_keys(&self, event_keys: &[String]) -> Result<HashSet<String>> {
        if event_keys.is_empty() {
            return Ok(HashSet::new());
        }

        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let mut read_keys = HashSet::new();

        for keys in event_keys.chunks(900) {
            let placeholders = vec!["?"; keys.len()].join(", ");
            let sql = format!(
                "
SELECT event_key
FROM event_log_v2
WHERE read_at IS NOT NULL
  AND event_key IN ({placeholders})
"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(keys.iter()), |row| row.get(0))?;
            for row in rows {
                read_keys.insert(row?);
            }
        }
        Ok(read_keys)
    }
}

impl TimelineReadMarkPort for SqliteStateStore {
    fn mark_timeline_event_read(&self, event_key: &str, read_at: DateTime<Utc>) -> Result<()> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        conn.execute(
            "
UPDATE event_log_v2
SET read_at = COALESCE(read_at, ?2)
WHERE event_key = ?1
",
            params![event_key, read_at.to_rfc3339()],
        )?;
        Ok(())
    }
}

impl RetentionPort for SqliteStateStore {
    fn cleanup_old(&self, retention_days: u32, now: DateTime<Utc>) -> Result<()> {
        let cutoff = now - Duration::days(retention_days as i64);
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        conn.execute(
            "DELETE FROM event_log_v2 WHERE created_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(())
    }
}

impl RepoBatchPort for SqliteStateStore {
    fn persist_repo_batch(&self, batch: &RepoPersistBatch) -> Result<PersistBatchResult> {
        let mut conn = self.conn.lock().expect("sqlite mutex poisoned");
        let tx = conn.transaction()?;
        tx.execute(
            "
INSERT INTO polling_cursors_v2 (repo, last_polled_at)
VALUES (?1, ?2)
ON CONFLICT(repo) DO UPDATE SET last_polled_at = excluded.last_polled_at
",
            params![batch.repo, batch.poll_started_at.to_rfc3339()],
        )?;

        let mut result = PersistBatchResult::default();
        for event in &batch.events {
            if event.repo != batch.repo {
                return Err(anyhow!(
                    "repo batch mismatch: batch repo={} event repo={}",
                    batch.repo,
                    event.repo
                ));
            }

            let event_key = event.event_key();
            let payload = serde_json::to_string(event)?;
            let inserted = tx.execute(
                "
INSERT OR IGNORE INTO event_log_v2
  (event_key, repo, payload_json, created_at, observed_at, delivered_at, read_at)
VALUES
  (?1, ?2, ?3, ?4, ?5, ?6, NULL)
",
                params![
                    event_key,
                    event.repo,
                    payload,
                    event.created_at.to_rfc3339(),
                    batch.poll_started_at.to_rfc3339(),
                    batch.poll_started_at.to_rfc3339(),
                ],
            )?;

            if inserted == 1 {
                result.newly_logged_event_keys.push(event_key.clone());
            }
        }

        tx.commit()?;
        Ok(result)
    }
}
