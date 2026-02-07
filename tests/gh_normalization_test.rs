use chrono::{TimeZone, Utc};
use gh_watch::domain::events::EventKind;
use gh_watch::infra::gh_client::normalize_events_from_payloads;

#[test]
fn normalize_events_filters_by_since_and_maps_kinds() {
    let pulls = include_str!("fixtures/pulls.json");
    let issues = include_str!("fixtures/issues.json");
    let issue_comments = include_str!("fixtures/issue_comments.json");
    let review_comments = include_str!("fixtures/review_comments.json");

    let since = Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap();
    let events = normalize_events_from_payloads(
        "acme/api",
        since,
        pulls,
        issues,
        issue_comments,
        review_comments,
    )
    .unwrap();

    assert_eq!(events.len(), 4);
    assert!(events.iter().any(|e| e.kind == EventKind::PrCreated));
    assert!(events.iter().any(|e| e.kind == EventKind::IssueCreated));
    assert!(events
        .iter()
        .any(|e| e.kind == EventKind::IssueCommentCreated));
    assert!(events
        .iter()
        .any(|e| e.kind == EventKind::PrReviewCommentCreated));
    assert!(events.iter().all(|e| e.created_at > since));
}
