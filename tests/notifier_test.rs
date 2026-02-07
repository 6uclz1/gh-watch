use chrono::{TimeZone, Utc};
use gh_watch::domain::events::{EventKind, WatchEvent};
use gh_watch::infra::notifier::build_notification_body;

#[test]
fn notification_body_contains_url_when_enabled() {
    let event = WatchEvent {
        event_id: "1".to_string(),
        repo: "acme/api".to_string(),
        kind: EventKind::PrCreated,
        actor: "alice".to_string(),
        title: "Add API".to_string(),
        url: "https://example.com/pr/1".to_string(),
        created_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        source_item_id: "1".to_string(),
    };

    let body = build_notification_body(&event, true);
    assert!(body.contains("Add API"));
    assert!(body.contains("https://example.com/pr/1"));
}
