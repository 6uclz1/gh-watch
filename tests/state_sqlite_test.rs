use chrono::{Duration, TimeZone, Utc};
use gh_watch::domain::events::{EventKind, WatchEvent};
use gh_watch::infra::state_sqlite::{SqliteStateStore, StateSchemaMismatchError};
use gh_watch::ports::{
    CursorPort, RepoBatchPort, RepoPersistBatch, RetentionPort, TimelineQueryPort,
    TimelineReadMarkPort,
};
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
fn cleanup_removes_old_events() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();

    let old = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    let now = old + Duration::days(120);
    let ev = sample_event("old-1", old);

    let batch = RepoPersistBatch {
        repo: "acme/api".to_string(),
        poll_started_at: old,
        events: vec![ev.clone()],
    };
    store.persist_repo_batch(&batch).unwrap();

    store.cleanup_old(90, now).unwrap();
    let timeline = store.load_timeline_events(10).unwrap();
    assert!(timeline.is_empty());
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
INSERT INTO schema_meta (key, value) VALUES ('schema_version', '2');
",
    )
    .unwrap();
    drop(conn);

    let err = match SqliteStateStore::new(&db) {
        Ok(_) => panic!("v2 schema should be rejected"),
        Err(err) => err,
    };
    assert!(err.downcast_ref::<StateSchemaMismatchError>().is_some());
}

#[test]
fn persisted_event_is_unread_until_explicitly_marked_read() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = SqliteStateStore::new(&db).unwrap();
    let event = sample_event(
        "unread-1",
        Utc.with_ymd_and_hms(2025, 1, 8, 10, 0, 0).unwrap(),
    );
    let key = event.event_key();

    let batch = RepoPersistBatch {
        repo: "acme/api".to_string(),
        poll_started_at: event.created_at,
        events: vec![event],
    };
    store.persist_repo_batch(&batch).unwrap();

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
