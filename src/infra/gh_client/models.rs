use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct GhUser {
    pub(super) login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct GhPull {
    pub(super) id: i64,
    pub(super) number: Option<i64>,
    pub(super) title: String,
    pub(super) html_url: String,
    pub(super) created_at: DateTime<Utc>,
    pub(super) updated_at: Option<DateTime<Utc>>,
    pub(super) merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(super) requested_reviewers: Vec<GhUser>,
    pub(super) merged_by: Option<GhUser>,
    pub(super) user: Option<GhUser>,
}

impl GhPull {
    pub(super) fn number_or_id(&self) -> i64 {
        self.number.unwrap_or(self.id)
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct GhIssue {
    pub(super) id: i64,
    pub(super) number: Option<i64>,
    pub(super) title: String,
    pub(super) html_url: String,
    pub(super) created_at: DateTime<Utc>,
    pub(super) user: Option<GhUser>,
    pub(super) pull_request: Option<serde_json::Value>,
}

impl GhIssue {
    pub(super) fn number_or_id(&self) -> i64 {
        self.number.unwrap_or(self.id)
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct GhComment {
    pub(super) id: i64,
    pub(super) issue_url: Option<String>,
    pub(super) pull_request_url: Option<String>,
    pub(super) pull_request_review_id: Option<i64>,
    pub(super) html_url: String,
    pub(super) created_at: DateTime<Utc>,
    pub(super) body: Option<String>,
    pub(super) user: Option<GhUser>,
}
