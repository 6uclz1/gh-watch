use std::collections::HashSet;

use chrono::{TimeZone, Utc};
use gh_watch::domain::decision::{decide_notification, sort_timeline_desc, NotificationDecision};
use gh_watch::domain::events::{EventKind, WatchEvent};

fn sample_event(id: &str, created_at: chrono::DateTime<Utc>) -> WatchEvent {
    WatchEvent {
        event_id: id.to_string(),
        repo: "acme/api".to_string(),
        kind: EventKind::PrCreated,
        actor: "alice".to_string(),
        title: "Add endpoint".to_string(),
        url: "https://example.com/pr/1".to_string(),
        created_at,
        source_item_id: id.to_string(),
        subject_author: Some("alice".to_string()),
        requested_reviewer: None,
        mentions: Vec::new(),
    }
}

#[test]
fn bootstrap_skips_notifications() {
    let e = sample_event("1", Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    let notified = HashSet::new();
    let d = decide_notification(&e, true, &notified);
    assert!(matches!(d, NotificationDecision::SkipBootstrap));
}

#[test]
fn already_notified_skips_notification() {
    let e = sample_event("1", Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    let mut notified = HashSet::new();
    notified.insert(e.event_key());

    let d = decide_notification(&e, false, &notified);
    assert!(matches!(d, NotificationDecision::SkipAlreadyNotified));
}

#[test]
fn unseen_event_is_notified() {
    let e = sample_event("1", Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    let notified = HashSet::new();

    let d = decide_notification(&e, false, &notified);
    match d {
        NotificationDecision::Notify(ev) => assert_eq!(ev.event_id, "1"),
        _ => panic!("expected notify"),
    }
}

#[test]
fn timeline_sort_is_descending() {
    let older = sample_event("1", Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    let newer = sample_event("2", Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap());

    let sorted = sort_timeline_desc(vec![older.clone(), newer.clone()]);
    assert_eq!(sorted[0].event_id, newer.event_id);
    assert_eq!(sorted[1].event_id, older.event_id);
}
