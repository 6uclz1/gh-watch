use chrono::{Duration, TimeZone, Utc};
use gh_watch::domain::events::{EventKind, WatchEvent};
use gh_watch::infra::state_sqlite::SqliteStateStore;
use gh_watch::ports::StateStorePort;
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

    store.cleanup_old(90, now).unwrap();
    assert!(!store.is_event_notified(&ev.event_key()).unwrap());
    let timeline = store.load_timeline_events(10).unwrap();
    assert!(timeline.is_empty());
}
