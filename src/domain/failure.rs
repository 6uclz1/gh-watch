use chrono::{DateTime, Utc};

pub const FAILURE_KIND_REPO_POLL: &str = "repo_poll";
pub const FAILURE_KIND_NOTIFICATION: &str = "notification";
pub const FAILURE_KIND_POLL_LOOP: &str = "poll_loop";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureRecord {
    pub kind: String,
    pub repo: String,
    pub failed_at: DateTime<Utc>,
    pub message: String,
}

impl FailureRecord {
    pub fn new(
        kind: impl Into<String>,
        repo: impl Into<String>,
        failed_at: DateTime<Utc>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            repo: repo.into(),
            failed_at,
            message: message.into(),
        }
    }
}
