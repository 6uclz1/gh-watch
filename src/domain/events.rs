use std::fmt::{Display, Formatter};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    PrCreated,
    IssueCreated,
    IssueCommentCreated,
    PrReviewCommentCreated,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PrCreated => "pr_created",
            Self::IssueCreated => "issue_created",
            Self::IssueCommentCreated => "issue_comment_created",
            Self::PrReviewCommentCreated => "pr_review_comment_created",
        }
    }
}

impl Display for EventKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchEvent {
    pub event_id: String,
    pub repo: String,
    pub kind: EventKind,
    pub actor: String,
    pub title: String,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub source_item_id: String,
}

impl WatchEvent {
    pub fn event_key(&self) -> String {
        format!("{}:{}:{}", self.repo, self.kind, self.source_item_id)
    }
}

pub fn event_matches_notification_filters(
    event: &WatchEvent,
    allowed_event_kinds: &[EventKind],
    ignore_actors: &[String],
) -> bool {
    let kind_allowed =
        allowed_event_kinds.is_empty() || allowed_event_kinds.iter().any(|kind| kind == &event.kind);
    if !kind_allowed {
        return false;
    }

    !ignore_actors.iter().any(|actor| actor == &event.actor)
}
