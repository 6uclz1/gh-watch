use chrono::{Duration, TimeZone, Utc};
use gh_watch::domain::events::{EventKind, WatchEvent};
use gh_watch::domain::failure::{FailureRecord, FAILURE_KIND_NOTIFICATION, FAILURE_KIND_REPO_POLL};
use gh_watch::infra::state_sqlite::{SqliteStateStore, StateSchemaMismatchError};
use gh_watch::ports::{RepoPersistBatch, StateStorePort};
use rusqlite::params;
use tempfile::tempdir;

fn sample_event(id: &str, created_at: chrono::DateTime<Utc>) -> WatchEvent {
    WatchEvent {
        event_id: id.to_string(),
        repo: "acme/api".to_string(),
        kind: EventKind::IssueCreated,
        actor: "bob".to_string(),
        title: "Bug report".to_string(),
        url: "https://example.com/issues/1".to_string(),
        created_at,
        source_item_id: id.to_string(),
        subject_author: Some("bob".to_string()),
        requested_reviewer: None,
        mentions: Vec::new(),
    }
}

#[test]
fn cursor_roundtrip() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let ts = Utc.with_ymd_and_hms(2025, 1, 1, 10, 0, 0).unwrap();

    store.set_cursor("acme/api", ts).unwrap();
    let out = store.get_cursor("acme/api").unwrap().unwrap();
    assert_eq!(out, ts);
}

#[test]
fn notified_event_is_persisted() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let ev = sample_event("101", Utc.with_ymd_and_hms(2025, 1, 1, 11, 0, 0).unwrap());

    assert!(!store.is_event_notified(&ev.event_key()).unwrap());
    store
        .record_notified_event(&ev, Utc.with_ymd_and_hms(2025, 1, 1, 11, 1, 0).unwrap())
        .unwrap();
    assert!(store.is_event_notified(&ev.event_key()).unwrap());
}

#[test]
fn cleanup_removes_old_records() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();

    let old = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    let now = old + Duration::days(120);
    let ev = sample_event("old-1", old);
    store.record_notified_event(&ev, old).unwrap();
    store.append_timeline_event(&ev).unwrap();
    store
        .record_failure(&FailureRecord::new(
            FAILURE_KIND_REPO_POLL,
            "acme/api",
            old,
            "request failed",
        ))
        .unwrap();

    store.cleanup_old(90, 200, now).unwrap();
    assert!(!store.is_event_notified(&ev.event_key()).unwrap());
    let timeline = store.load_timeline_events(10).unwrap();
    assert!(timeline.is_empty());
    assert!(store.latest_failure().unwrap().is_none());
}

#[test]
fn latest_failure_roundtrip() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let ts = Utc.with_ymd_and_hms(2025, 1, 2, 12, 0, 0).unwrap();

    store
        .record_failure(&FailureRecord::new(
            FAILURE_KIND_NOTIFICATION,
            "acme/api",
            ts,
            "acme/api:pr_created:123: notify failed",
        ))
        .unwrap();

    let latest = store.latest_failure().unwrap().unwrap();
    assert_eq!(latest.kind, FAILURE_KIND_NOTIFICATION);
    assert_eq!(latest.repo, "acme/api");
    assert_eq!(latest.failed_at, ts);
    assert!(latest.message.contains("notify failed"));
}

#[test]
fn failure_history_limit_keeps_recent_records_only() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let base = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();

    for i in 0..3 {
        store
            .record_failure(&FailureRecord::new(
                FAILURE_KIND_REPO_POLL,
                "acme/api",
                base + Duration::minutes(i),
                format!("boom-{i}"),
            ))
            .unwrap();
    }

    store.cleanup_old(90, 2, base + Duration::days(1)).unwrap();

    let latest = store.latest_failure().unwrap().unwrap();
    assert_eq!(latest.message, "boom-2");
    let conn = rusqlite::Connection::open(db).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM failure_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn opening_legacy_timeline_schema_returns_schema_mismatch_error() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let legacy_event = sample_event(
        "legacy-1",
        Utc.with_ymd_and_hms(2025, 1, 7, 9, 0, 0).unwrap(),
    );

    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "
CREATE TABLE timeline_events (
  event_key TEXT PRIMARY KEY,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
",
    )
    .unwrap();
    conn.execute(
        "
INSERT INTO timeline_events (event_key, payload_json, created_at)
VALUES (?1, ?2, ?3)
",
        params![
            legacy_event.event_key(),
            serde_json::to_string(&legacy_event).unwrap(),
            legacy_event.created_at.to_rfc3339(),
        ],
    )
    .unwrap();
    drop(conn);

    let err = match SqliteStateStore::new(&db) {
        Ok(_) => panic!("legacy schema should be rejected"),
        Err(err) => err,
    };
    assert!(err.downcast_ref::<StateSchemaMismatchError>().is_some());
}

#[test]
fn opening_v2_schema_returns_schema_mismatch_error() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "
CREATE TABLE schema_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE polling_cursors_v2 (
  repo TEXT PRIMARY KEY,
  last_polled_at TEXT NOT NULL
);
CREATE TABLE event_log_v2 (
  event_key TEXT PRIMARY KEY,
  repo TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  observed_at TEXT NOT NULL,
  delivered_at TEXT,
  read_at TEXT
);
CREATE TABLE notification_queue_v2 (
  event_key TEXT PRIMARY KEY,
  repo TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT NOT NULL,
  last_error TEXT,
  enqueued_at TEXT NOT NULL
);
CREATE TABLE failure_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  repo TEXT NOT NULL,
  failed_at TEXT NOT NULL,
  message TEXT NOT NULL
);
INSERT INTO schema_meta (key, value) VALUES ('schema_version', '2');
",
    )
    .unwrap();
    drop(conn);

    let err = match SqliteStateStore::new(&db) {
        Ok(_) => panic!("v2 schema should be rejected"),
        Err(err) => err,
    };
    let rendered = format!("{err:#}");
    assert!(err.downcast_ref::<StateSchemaMismatchError>().is_some());
    assert!(rendered.contains("init --reset-state"));
}

#[test]
fn appended_event_is_unread_until_explicitly_marked_read() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let event = sample_event(
        "unread-1",
        Utc.with_ymd_and_hms(2025, 1, 8, 10, 0, 0).unwrap(),
    );
    let key = event.event_key();

    store.append_timeline_event(&event).unwrap();
    let unread = store
        .load_read_event_keys(std::slice::from_ref(&key))
        .unwrap();
    assert!(!unread.contains(&key));

    let read_at = Utc.with_ymd_and_hms(2025, 1, 8, 10, 5, 0).unwrap();
    store.mark_timeline_event_read(&key, read_at).unwrap();

    let read = store
        .load_read_event_keys(std::slice::from_ref(&key))
        .unwrap();
    assert!(read.contains(&key));
}

#[test]
fn upserting_same_event_key_preserves_read_state() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let created_at = Utc.with_ymd_and_hms(2025, 1, 9, 11, 0, 0).unwrap();
    let key = sample_event("sticky-read", created_at).event_key();

    store
        .append_timeline_event(&sample_event("sticky-read", created_at))
        .unwrap();
    let read_at = Utc.with_ymd_and_hms(2025, 1, 9, 11, 2, 0).unwrap();
    store.mark_timeline_event_read(&key, read_at).unwrap();

    let mut updated = sample_event(
        "sticky-read",
        Utc.with_ymd_and_hms(2025, 1, 9, 11, 30, 0).unwrap(),
    );
    updated.title = "Updated title".to_string();
    store.append_timeline_event(&updated).unwrap();

    let read = store
        .load_read_event_keys(std::slice::from_ref(&key))
        .unwrap();
    assert!(read.contains(&key));

    let conn = rusqlite::Connection::open(&db).unwrap();
    let persisted_read_at: Option<String> = conn
        .query_row(
            "SELECT read_at FROM event_log_v2 WHERE event_key = ?1",
            params![key],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_read_at, Some(read_at.to_rfc3339()));
}

#[test]
fn persist_batch_marks_events_delivered_immediately() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let poll_started_at = Utc.with_ymd_and_hms(2025, 1, 11, 0, 0, 0).unwrap();
    let event = sample_event(
        "bootstrap-1",
        Utc.with_ymd_and_hms(2025, 1, 10, 23, 0, 0).unwrap(),
    );

    let batch = RepoPersistBatch {
        repo: "acme/api".to_string(),
        poll_started_at,
        events: vec![event.clone()],
    };
    let persisted = store.persist_repo_batch(&batch).unwrap();
    assert_eq!(persisted.newly_logged_event_keys, vec![event.event_key()]);
    assert!(store.is_event_notified(&event.event_key()).unwrap());
}

#[test]
fn persist_batch_deduplicates_existing_event_keys() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let poll_started_at = Utc.with_ymd_and_hms(2025, 1, 12, 0, 0, 0).unwrap();
    let event = sample_event(
        "dedupe-1",
        Utc.with_ymd_and_hms(2025, 1, 11, 23, 0, 0).unwrap(),
    );

    let first = RepoPersistBatch {
        repo: "acme/api".to_string(),
        poll_started_at,
        events: vec![event.clone()],
    };
    let second = RepoPersistBatch {
        repo: "acme/api".to_string(),
        poll_started_at: poll_started_at + Duration::minutes(1),
        events: vec![event.clone()],
    };

    let first_result = store.persist_repo_batch(&first).unwrap();
    let second_result = store.persist_repo_batch(&second).unwrap();
    assert_eq!(
        first_result.newly_logged_event_keys,
        vec![event.event_key()]
    );
    assert!(second_result.newly_logged_event_keys.is_empty());
}
