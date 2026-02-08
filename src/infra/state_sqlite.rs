use std::{fs, path::Path, sync::Mutex};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::{
    domain::{events::WatchEvent, failure::FailureRecord},
    ports::StateStorePort,
};

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

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        conn.execute_batch(
            "
CREATE TABLE IF NOT EXISTS notified_events (
  event_key TEXT PRIMARY KEY,
  repo TEXT NOT NULL,
  kind TEXT NOT NULL,
  source_id TEXT NOT NULL,
  notified_at TEXT NOT NULL,
  event_created_at TEXT NOT NULL,
  url TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS polling_cursors (
  repo TEXT PRIMARY KEY,
  last_polled_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS timeline_events (
  event_key TEXT PRIMARY KEY,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS failure_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  repo TEXT NOT NULL,
  failed_at TEXT NOT NULL,
  message TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_failure_events_failed_at
ON failure_events (failed_at DESC, id DESC);
",
        )?;
        Ok(())
    }

    pub fn load_timeline_events_since(
        &self,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<WatchEvent>> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let mut stmt = conn.prepare(
            "
SELECT payload_json
FROM timeline_events
WHERE created_at >= ?1
ORDER BY created_at DESC
LIMIT ?2
",
        )?;

        let rows = stmt.query_map(params![since.to_rfc3339(), limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let items = rows
            .map(|row| -> Result<WatchEvent> {
                let payload = row?;
                Ok(serde_json::from_str(&payload)?)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(items)
    }

    pub fn load_failures_since(
        &self,
        since: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<FailureRecord>> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let mut stmt = conn.prepare(
            "
SELECT kind, repo, failed_at, message
FROM failure_events
WHERE failed_at >= ?1
ORDER BY failed_at DESC, id DESC
LIMIT ?2
",
        )?;

        let rows = stmt.query_map(params![since.to_rfc3339(), limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let items = rows
            .map(|row| -> Result<FailureRecord> {
                let (kind, repo, failed_at, message) = row?;
                let failed_at = DateTime::parse_from_rfc3339(&failed_at)?.with_timezone(&Utc);
                Ok(FailureRecord::new(kind, repo, failed_at, message))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(items)
    }
}

impl StateStorePort for SqliteStateStore {
    fn get_cursor(&self, repo: &str) -> Result<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let value: Option<String> = conn
            .query_row(
                "SELECT last_polled_at FROM polling_cursors WHERE repo = ?1",
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
INSERT INTO polling_cursors (repo, last_polled_at)
VALUES (?1, ?2)
ON CONFLICT(repo) DO UPDATE SET last_polled_at = excluded.last_polled_at
",
            params![repo, at.to_rfc3339()],
        )?;
        Ok(())
    }

    fn is_event_notified(&self, event_key: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let exists: Option<String> = conn
            .query_row(
                "SELECT event_key FROM notified_events WHERE event_key = ?1",
                params![event_key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(exists.is_some())
    }

    fn record_notified_event(&self, event: &WatchEvent, notified_at: DateTime<Utc>) -> Result<()> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        conn.execute(
            "
INSERT OR REPLACE INTO notified_events
(event_key, repo, kind, source_id, notified_at, event_created_at, url)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
",
            params![
                event.event_key(),
                event.repo,
                event.kind.as_str(),
                event.source_item_id,
                notified_at.to_rfc3339(),
                event.created_at.to_rfc3339(),
                event.url,
            ],
        )?;
        Ok(())
    }

    fn record_failure(&self, failure: &FailureRecord) -> Result<()> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        conn.execute(
            "
INSERT INTO failure_events (kind, repo, failed_at, message)
VALUES (?1, ?2, ?3, ?4)
",
            params![
                &failure.kind,
                &failure.repo,
                failure.failed_at.to_rfc3339(),
                &failure.message,
            ],
        )?;
        Ok(())
    }

    fn latest_failure(&self) -> Result<Option<FailureRecord>> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let row: Option<(String, String, String, String)> = conn
            .query_row(
                "
SELECT kind, repo, failed_at, message
FROM failure_events
ORDER BY failed_at DESC, id DESC
LIMIT 1
",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;

        row.map(
            |(kind, repo, failed_at, message)| -> Result<FailureRecord> {
                let parsed = DateTime::parse_from_rfc3339(&failed_at)?.with_timezone(&Utc);
                Ok(FailureRecord::new(kind, repo, parsed, message))
            },
        )
        .transpose()
    }

    fn append_timeline_event(&self, event: &WatchEvent) -> Result<()> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let payload = serde_json::to_string(event)?;
        conn.execute(
            "
INSERT OR REPLACE INTO timeline_events (event_key, payload_json, created_at)
VALUES (?1, ?2, ?3)
",
            params![event.event_key(), payload, event.created_at.to_rfc3339()],
        )?;
        Ok(())
    }

    fn load_timeline_events(&self, limit: usize) -> Result<Vec<WatchEvent>> {
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        let mut stmt = conn.prepare(
            "
SELECT payload_json
FROM timeline_events
ORDER BY created_at DESC
LIMIT ?1
",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| row.get::<_, String>(0))?;
        let items = rows
            .map(|row| -> Result<WatchEvent> {
                let payload = row?;
                Ok(serde_json::from_str(&payload)?)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(items)
    }

    fn cleanup_old(
        &self,
        retention_days: u32,
        failure_history_limit: usize,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let cutoff = now - Duration::days(retention_days as i64);
        let conn = self.conn.lock().expect("sqlite mutex poisoned");
        conn.execute(
            "DELETE FROM notified_events WHERE event_created_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        conn.execute(
            "DELETE FROM timeline_events WHERE created_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        conn.execute(
            "DELETE FROM failure_events WHERE failed_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        conn.execute(
            "
DELETE FROM failure_events
WHERE id IN (
  SELECT id
  FROM failure_events
  ORDER BY failed_at DESC, id DESC
  LIMIT -1 OFFSET ?1
)
",
            params![failure_history_limit as i64],
        )?;
        Ok(())
    }
}
