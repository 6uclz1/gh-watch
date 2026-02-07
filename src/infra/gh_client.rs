use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::process::Command;

use crate::{
    domain::events::{EventKind, WatchEvent},
    ports::GhClientPort,
};

#[derive(Debug, Clone)]
pub struct GhCliClient {
    gh_bin: PathBuf,
}

impl Default for GhCliClient {
    fn default() -> Self {
        let gh_bin = std::env::var_os("GH_WATCH_GH_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("gh"));
        Self { gh_bin }
    }
}

impl GhCliClient {
    pub fn new_with_bin<P: Into<PathBuf>>(path: P) -> Self {
        Self {
            gh_bin: path.into(),
        }
    }

    async fn run_gh(&self, args: &[&str]) -> Result<String> {
        let output = Command::new(&self.gh_bin)
            .args(args)
            .output()
            .await
            .with_context(|| format!("failed to execute gh command: {:?}", args))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "gh command failed (status={}): {}",
                output.status,
                stderr.trim()
            ));
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }
}

pub fn normalize_events_from_payloads(
    repo: &str,
    since: DateTime<Utc>,
    pulls_json: &str,
    issues_json: &str,
    issue_comments_json: &str,
    review_comments_json: &str,
) -> Result<Vec<WatchEvent>> {
    let pulls: Vec<GhPull> = serde_json::from_str(pulls_json).context("invalid pulls payload")?;
    let issues: Vec<GhIssue> =
        serde_json::from_str(issues_json).context("invalid issues payload")?;
    let issue_comments: Vec<GhComment> =
        serde_json::from_str(issue_comments_json).context("invalid issue comments payload")?;
    let review_comments: Vec<GhComment> =
        serde_json::from_str(review_comments_json).context("invalid review comments payload")?;

    let mut events = Vec::new();

    events.extend(
        pulls
            .into_iter()
            .filter(|pr| pr.created_at > since)
            .map(|pr| WatchEvent {
                event_id: format!("pr:{}", pr.id),
                repo: repo.to_string(),
                kind: EventKind::PrCreated,
                actor: pr
                    .user
                    .map(|u| u.login)
                    .unwrap_or_else(|| "unknown".to_string()),
                title: pr.title,
                url: pr.html_url,
                created_at: pr.created_at,
                source_item_id: pr.id.to_string(),
            }),
    );

    events.extend(
        issues
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
            .filter(|issue| issue.created_at > since)
            .map(|issue| WatchEvent {
                event_id: format!("issue:{}", issue.id),
                repo: repo.to_string(),
                kind: EventKind::IssueCreated,
                actor: issue
                    .user
                    .map(|u| u.login)
                    .unwrap_or_else(|| "unknown".to_string()),
                title: issue.title,
                url: issue.html_url,
                created_at: issue.created_at,
                source_item_id: issue.id.to_string(),
            }),
    );

    events.extend(
        issue_comments
            .into_iter()
            .filter(|comment| comment.created_at > since)
            .map(|comment| WatchEvent {
                event_id: format!("issue-comment:{}", comment.id),
                repo: repo.to_string(),
                kind: EventKind::IssueCommentCreated,
                actor: comment
                    .user
                    .map(|u| u.login)
                    .unwrap_or_else(|| "unknown".to_string()),
                title: title_from_comment(comment.body.as_deref(), "New issue/PR comment"),
                url: comment.html_url,
                created_at: comment.created_at,
                source_item_id: comment.id.to_string(),
            }),
    );

    events.extend(
        review_comments
            .into_iter()
            .filter(|comment| comment.created_at > since)
            .map(|comment| WatchEvent {
                event_id: format!("review-comment:{}", comment.id),
                repo: repo.to_string(),
                kind: EventKind::PrReviewCommentCreated,
                actor: comment
                    .user
                    .map(|u| u.login)
                    .unwrap_or_else(|| "unknown".to_string()),
                title: title_from_comment(comment.body.as_deref(), "New PR review comment"),
                url: comment.html_url,
                created_at: comment.created_at,
                source_item_id: comment.id.to_string(),
            }),
    );

    events.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(events)
}

#[async_trait]
impl GhClientPort for GhCliClient {
    async fn check_auth(&self) -> Result<()> {
        self.run_gh(&["auth", "status"])
            .await
            .context("gh auth status failed")?;
        Ok(())
    }

    async fn fetch_repo_events(&self, repo: &str, since: DateTime<Utc>) -> Result<Vec<WatchEvent>> {
        let pulls = self
            .run_gh(&[
                "api",
                &format!("repos/{repo}/pulls?state=all&sort=created&direction=desc&per_page=100"),
            ])
            .await
            .with_context(|| format!("failed to fetch pulls for {repo}"))?;

        let issues = self
            .run_gh(&[
                "api",
                &format!("repos/{repo}/issues?state=all&sort=created&direction=desc&per_page=100"),
            ])
            .await
            .with_context(|| format!("failed to fetch issues for {repo}"))?;

        let since_rfc3339 = since.to_rfc3339();
        let issue_comments = self
            .run_gh(&[
                "api",
                &format!("repos/{repo}/issues/comments?since={since_rfc3339}&per_page=100"),
            ])
            .await
            .with_context(|| format!("failed to fetch issue comments for {repo}"))?;

        let review_comments = self
            .run_gh(&[
                "api",
                &format!("repos/{repo}/pulls/comments?since={since_rfc3339}&per_page=100"),
            ])
            .await
            .with_context(|| format!("failed to fetch review comments for {repo}"))?;

        normalize_events_from_payloads(
            repo,
            since,
            &pulls,
            &issues,
            &issue_comments,
            &review_comments,
        )
    }
}

fn title_from_comment(body: Option<&str>, fallback: &str) -> String {
    body.and_then(|b| b.lines().next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate(s, 120))
        .unwrap_or_else(|| fallback.to_string())
}

fn truncate(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut s = input
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    s.push_str("...");
    s
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhPull {
    id: i64,
    title: String,
    html_url: String,
    created_at: DateTime<Utc>,
    user: Option<GhUser>,
}

#[derive(Debug, Deserialize)]
struct GhIssue {
    id: i64,
    title: String,
    html_url: String,
    created_at: DateTime<Utc>,
    user: Option<GhUser>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GhComment {
    id: i64,
    html_url: String,
    created_at: DateTime<Utc>,
    body: Option<String>,
    user: Option<GhUser>,
}
