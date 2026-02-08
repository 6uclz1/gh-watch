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
    PrReviewRequested,
    PrReviewSubmitted,
    PrMerged,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PrCreated => "pr_created",
            Self::IssueCreated => "issue_created",
            Self::IssueCommentCreated => "issue_comment_created",
            Self::PrReviewCommentCreated => "pr_review_comment_created",
            Self::PrReviewRequested => "pr_review_requested",
            Self::PrReviewSubmitted => "pr_review_submitted",
            Self::PrMerged => "pr_merged",
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
    #[serde(default)]
    pub subject_author: Option<String>,
    #[serde(default)]
    pub requested_reviewer: Option<String>,
    #[serde(default)]
    pub mentions: Vec<String>,
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
    only_involving_me: bool,
    viewer_login: Option<&str>,
) -> bool {
    let kind_allowed = allowed_event_kinds.is_empty()
        || allowed_event_kinds.iter().any(|kind| kind == &event.kind);
    if !kind_allowed {
        return false;
    }

    if ignore_actors.iter().any(|actor| actor == &event.actor) {
        return false;
    }

    if !only_involving_me {
        return true;
    }

    let Some(viewer_login) = viewer_login else {
        return false;
    };
    event_involves_viewer(event, viewer_login)
}

fn event_involves_viewer(event: &WatchEvent, viewer_login: &str) -> bool {
    if event
        .requested_reviewer
        .as_deref()
        .is_some_and(|reviewer| reviewer.eq_ignore_ascii_case(viewer_login))
    {
        return true;
    }

    if event
        .mentions
        .iter()
        .any(|mention| mention.eq_ignore_ascii_case(viewer_login))
    {
        return true;
    }

    let is_update_event = !matches!(event.kind, EventKind::PrCreated | EventKind::IssueCreated);
    is_update_event
        && event
            .subject_author
            .as_deref()
            .is_some_and(|author| author.eq_ignore_ascii_case(viewer_login))
}
