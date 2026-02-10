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

#[test]
fn normalize_events_maps_review_requested_review_submitted_and_merged() {
    let pulls = r#"
[
  {
    "id": 10,
    "number": 10,
    "title": "New pull",
    "html_url": "https://example.com/pr/10",
    "created_at": "2025-01-03T00:00:00Z",
    "updated_at": "2025-01-06T00:00:00Z",
    "merged_at": "2025-01-06T00:00:00Z",
    "user": {"login": "bob"},
    "requested_reviewers": [{"login": "alice"}]
  }
]
"#;
    let issues = "[]";
    let issue_comments = "[]";
    let review_comments = r#"
[
  {
    "id": 41,
    "pull_request_review_id": 9001,
    "pull_request_url": "https://api.github.com/repos/acme/api/pulls/10",
    "html_url": "https://example.com/pr/10#pullrequestreview-9001",
    "created_at": "2025-01-05T00:00:00Z",
    "body": "@alice looks good",
    "user": {"login": "frank"}
  }
]
"#;

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

    assert!(events
        .iter()
        .any(|e| e.kind == EventKind::PrReviewRequested));
    assert!(events
        .iter()
        .any(|e| e.kind == EventKind::PrReviewSubmitted));
    assert!(events.iter().any(|e| e.kind == EventKind::PrMerged));
}

#[test]
fn normalize_events_deduplicates_review_submitted_by_review_id() {
    let pulls = r#"
[
  {
    "id": 10,
    "number": 10,
    "title": "New pull",
    "html_url": "https://example.com/pr/10",
    "created_at": "2025-01-03T00:00:00Z",
    "updated_at": "2025-01-06T00:00:00Z",
    "user": {"login": "bob"},
    "requested_reviewers": []
  }
]
"#;
    let issues = "[]";
    let issue_comments = "[]";
    let review_comments = r#"
[
  {
    "id": 41,
    "pull_request_review_id": 9001,
    "pull_request_url": "https://api.github.com/repos/acme/api/pulls/10",
    "html_url": "https://example.com/pr/10#discussion_r1",
    "created_at": "2025-01-05T00:00:00Z",
    "body": "first",
    "user": {"login": "frank"}
  },
  {
    "id": 42,
    "pull_request_review_id": 9001,
    "pull_request_url": "https://api.github.com/repos/acme/api/pulls/10",
    "html_url": "https://example.com/pr/10#discussion_r2",
    "created_at": "2025-01-05T00:01:00Z",
    "body": "second",
    "user": {"login": "frank"}
  }
]
"#;

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

    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == EventKind::PrReviewSubmitted)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == EventKind::PrReviewCommentCreated)
            .count(),
        2
    );
}
