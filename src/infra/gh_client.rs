use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

use crate::{
    domain::events::{EventKind, WatchEvent},
    ports::GhClientPort,
};

const PAGE_SIZE: usize = 100;
const MAX_PAGES_PER_ENDPOINT: usize = 1000;
const GH_EXEC_MAX_ATTEMPTS: usize = 5;
const GH_EXEC_RETRY_BASE_MS: u64 = 20;

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
        let output = self
            .run_gh_with_retry(args)
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

    async fn run_gh_with_retry(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        let mut attempt = 0usize;
        loop {
            attempt += 1;
            match Command::new(&self.gh_bin).args(args).output().await {
                Ok(output) => return Ok(output),
                Err(err) if err.raw_os_error() == Some(26) && attempt < GH_EXEC_MAX_ATTEMPTS => {
                    let wait_ms = GH_EXEC_RETRY_BASE_MS * attempt as u64;
                    sleep(Duration::from_millis(wait_ms)).await;
                }
                Err(err) => return Err(err),
            }
        }
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

    let mut events =
        normalize_events_from_items(repo, since, pulls, issues, issue_comments, review_comments);

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

    async fn viewer_login(&self) -> Result<String> {
        let login = self
            .run_gh(&["api", "user", "--jq", ".login"])
            .await
            .context("failed to load viewer login")?;
        if login.trim().is_empty() {
            return Err(anyhow!("viewer login is empty"));
        }
        Ok(login)
    }

    async fn fetch_repo_events(&self, repo: &str, since: DateTime<Utc>) -> Result<Vec<WatchEvent>> {
        let pulls_created = self
            .fetch_desc_until_since::<GhPull, _, _>(
                repo,
                "pulls",
                since,
                |page| {
                    format!(
                        "repos/{repo}/pulls?state=all&sort=created&direction=desc&per_page={PAGE_SIZE}&page={page}"
                    )
                },
                |pr| pr.created_at,
            )
            .await
            .with_context(|| format!("failed to fetch pulls for {repo}"))?;

        let pulls_updated = self
            .fetch_desc_until_since::<GhPull, _, _>(
                repo,
                "pull updates",
                since,
                |page| {
                    format!(
                        "repos/{repo}/pulls?state=all&sort=updated&direction=desc&per_page={PAGE_SIZE}&page={page}"
                    )
                },
                |pr| pr.updated_at.unwrap_or(pr.created_at),
            )
            .await
            .with_context(|| format!("failed to fetch pull updates for {repo}"))?;

        let pulls = merge_pulls_by_id(pulls_created, pulls_updated);

        let issues = self
            .fetch_desc_until_since::<GhIssue, _, _>(
                repo,
                "issues",
                since,
                |page| {
                    format!(
                        "repos/{repo}/issues?state=all&sort=created&direction=desc&per_page={PAGE_SIZE}&page={page}"
                    )
                },
                |issue| issue.created_at,
            )
            .await
            .with_context(|| format!("failed to fetch issues for {repo}"))?;

        let since_rfc3339 = since.to_rfc3339();
        let issue_comments = self
            .fetch_paginated_comments(
                repo,
                "issue comments",
                &format!("repos/{repo}/issues/comments?since={since_rfc3339}&per_page={PAGE_SIZE}"),
            )
            .await
            .with_context(|| format!("failed to fetch issue comments for {repo}"))?;

        let review_comments = self
            .fetch_paginated_comments(
                repo,
                "review comments",
                &format!("repos/{repo}/pulls/comments?since={since_rfc3339}&per_page={PAGE_SIZE}"),
            )
            .await
            .with_context(|| format!("failed to fetch review comments for {repo}"))?;

        let mut events = normalize_events_from_items(
            repo,
            since,
            pulls,
            issues,
            issue_comments,
            review_comments,
        );
        events.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(events)
    }
}

impl GhCliClient {
    async fn fetch_desc_until_since<T, E, C>(
        &self,
        repo: &str,
        item_label: &str,
        since: DateTime<Utc>,
        endpoint_for_page: E,
        created_at: C,
    ) -> Result<Vec<T>>
    where
        T: for<'de> Deserialize<'de>,
        E: Fn(usize) -> String,
        C: Fn(&T) -> DateTime<Utc>,
    {
        let mut all_items = Vec::new();
        let mut did_break = false;

        for page in 1..=MAX_PAGES_PER_ENDPOINT {
            let endpoint = endpoint_for_page(page);
            let payload = self.run_gh(&["api", &endpoint]).await.with_context(|| {
                format!(
                    "failed to fetch {item_label} for {repo} (endpoint={endpoint}, page={page})"
                )
            })?;

            let mut page_items: Vec<T> = serde_json::from_str(&payload).with_context(|| {
                format!(
                    "invalid {item_label} payload for {repo} (endpoint={endpoint}, page={page})"
                )
            })?;

            if page_items.is_empty() {
                did_break = true;
                break;
            }

            let reached_since = page_items
                .last()
                .map(|item| created_at(item) <= since)
                .unwrap_or(false);
            all_items.append(&mut page_items);

            if reached_since {
                did_break = true;
                break;
            }
        }

        if did_break {
            return Ok(all_items);
        }

        Err(anyhow!(
            "max pages reached while fetching {item_label} for {repo} (limit={MAX_PAGES_PER_ENDPOINT})"
        ))
    }

    async fn fetch_paginated_comments(
        &self,
        repo: &str,
        item_label: &str,
        endpoint: &str,
    ) -> Result<Vec<GhComment>> {
        let payload = self
            .run_gh(&["api", "--paginate", "--slurp", endpoint])
            .await
            .with_context(|| {
                format!(
                    "failed to fetch {item_label} for {repo} (endpoint={endpoint}, paginate=true)"
                )
            })?;

        let pages: Vec<Vec<GhComment>> = serde_json::from_str(&payload).with_context(|| {
            format!("invalid {item_label} payload for {repo} (endpoint={endpoint}, paginate=true)")
        })?;

        Ok(pages.into_iter().flatten().collect())
    }
}

fn normalize_events_from_items(
    repo: &str,
    since: DateTime<Utc>,
    pulls: Vec<GhPull>,
    issues: Vec<GhIssue>,
    issue_comments: Vec<GhComment>,
    review_comments: Vec<GhComment>,
) -> Vec<WatchEvent> {
    let mut events = Vec::new();
    let pull_author_by_number = pulls
        .iter()
        .filter_map(|pr| {
            pr.user
                .as_ref()
                .map(|user| (pr.number_or_id(), user.login.clone()))
        })
        .collect::<HashMap<_, _>>();

    let issue_author_by_number = issues
        .iter()
        .filter_map(|issue| {
            issue
                .user
                .as_ref()
                .map(|user| (issue.number_or_id(), user.login.clone()))
        })
        .collect::<HashMap<_, _>>();

    events.extend(pulls.iter().filter(|pr| pr.created_at > since).map(|pr| {
        let actor = user_login_or_unknown(pr.user.as_ref());
        WatchEvent {
            event_id: format!("pr:{}", pr.id),
            repo: repo.to_string(),
            kind: EventKind::PrCreated,
            actor: actor.clone(),
            title: pr.title.clone(),
            url: pr.html_url.clone(),
            created_at: pr.created_at,
            source_item_id: pr.id.to_string(),
            subject_author: Some(actor),
            requested_reviewer: None,
            mentions: extract_mentions(&pr.title),
        }
    }));

    events.extend(
        pulls
            .iter()
            .filter_map(|pr| {
                let updated_at = pr.updated_at.unwrap_or(pr.created_at);
                if updated_at <= since {
                    return None;
                }
                let author = pr.user.as_ref().map(|u| u.login.clone());
                let actor = user_login_or_unknown(pr.user.as_ref());
                let merged_at = pr.merged_at?;
                if merged_at <= since {
                    return None;
                }
                Some(WatchEvent {
                    event_id: format!("pr-merged:{}", pr.id),
                    repo: repo.to_string(),
                    kind: EventKind::PrMerged,
                    actor: pr
                        .merged_by
                        .as_ref()
                        .map(|u| u.login.clone())
                        .unwrap_or(actor),
                    title: format!("Merged: {}", pr.title),
                    url: pr.html_url.clone(),
                    created_at: merged_at,
                    source_item_id: pr.id.to_string(),
                    subject_author: author,
                    requested_reviewer: None,
                    mentions: Vec::new(),
                })
            })
            .collect::<Vec<_>>(),
    );

    for pr in &pulls {
        let updated_at = pr.updated_at.unwrap_or(pr.created_at);
        if updated_at <= since {
            continue;
        }
        for reviewer in &pr.requested_reviewers {
            let actor = user_login_or_unknown(pr.user.as_ref());
            events.push(WatchEvent {
                event_id: format!("pr-review-requested:{}:{}", pr.id, reviewer.login),
                repo: repo.to_string(),
                kind: EventKind::PrReviewRequested,
                actor,
                title: format!("Review requested: {}", pr.title),
                url: pr.html_url.clone(),
                created_at: updated_at,
                source_item_id: format!("{}:{}", pr.id, reviewer.login),
                subject_author: pr.user.as_ref().map(|u| u.login.clone()),
                requested_reviewer: Some(reviewer.login.clone()),
                mentions: Vec::new(),
            });
        }
    }

    events.extend(
        issues
            .iter()
            .filter(|issue| issue.pull_request.is_none())
            .filter(|issue| issue.created_at > since)
            .map(|issue| {
                let actor = user_login_or_unknown(issue.user.as_ref());
                WatchEvent {
                    event_id: format!("issue:{}", issue.id),
                    repo: repo.to_string(),
                    kind: EventKind::IssueCreated,
                    actor: actor.clone(),
                    title: issue.title.clone(),
                    url: issue.html_url.clone(),
                    created_at: issue.created_at,
                    source_item_id: issue.id.to_string(),
                    subject_author: Some(actor),
                    requested_reviewer: None,
                    mentions: extract_mentions(&issue.title),
                }
            }),
    );

    events.extend(
        issue_comments
            .iter()
            .filter(|comment| comment.created_at > since)
            .map(|comment| {
                let body = comment.body.clone().unwrap_or_default();
                let subject_author = comment
                    .issue_url
                    .as_deref()
                    .and_then(parse_number_from_url)
                    .and_then(|number| {
                        pull_author_by_number
                            .get(&number)
                            .cloned()
                            .or_else(|| issue_author_by_number.get(&number).cloned())
                    });
                WatchEvent {
                    event_id: format!("issue-comment:{}", comment.id),
                    repo: repo.to_string(),
                    kind: EventKind::IssueCommentCreated,
                    actor: user_login_or_unknown(comment.user.as_ref()),
                    title: title_from_comment(comment.body.as_deref(), "New issue/PR comment"),
                    url: comment.html_url.clone(),
                    created_at: comment.created_at,
                    source_item_id: comment.id.to_string(),
                    subject_author,
                    requested_reviewer: None,
                    mentions: extract_mentions(&body),
                }
            }),
    );

    let mut seen_review_submission_ids = HashSet::new();
    for comment in review_comments
        .iter()
        .filter(|comment| comment.created_at > since)
    {
        let body = comment.body.clone().unwrap_or_default();
        let subject_author = comment
            .pull_request_url
            .as_deref()
            .and_then(parse_number_from_url)
            .and_then(|number| pull_author_by_number.get(&number).cloned());
        let actor = user_login_or_unknown(comment.user.as_ref());

        events.push(WatchEvent {
            event_id: format!("review-comment:{}", comment.id),
            repo: repo.to_string(),
            kind: EventKind::PrReviewCommentCreated,
            actor: actor.clone(),
            title: title_from_comment(comment.body.as_deref(), "New PR review comment"),
            url: comment.html_url.clone(),
            created_at: comment.created_at,
            source_item_id: comment.id.to_string(),
            subject_author: subject_author.clone(),
            requested_reviewer: None,
            mentions: extract_mentions(&body),
        });

        if let Some(review_id) = comment.pull_request_review_id {
            if seen_review_submission_ids.insert(review_id) {
                events.push(WatchEvent {
                    event_id: format!("review-submitted:{review_id}"),
                    repo: repo.to_string(),
                    kind: EventKind::PrReviewSubmitted,
                    actor: actor.clone(),
                    title: title_from_comment(comment.body.as_deref(), "PR review submitted"),
                    url: comment.html_url.clone(),
                    created_at: comment.created_at,
                    source_item_id: review_id.to_string(),
                    subject_author: subject_author.clone(),
                    requested_reviewer: None,
                    mentions: extract_mentions(&body),
                });
            }
        }
    }

    events
}

fn merge_pulls_by_id(created: Vec<GhPull>, updated: Vec<GhPull>) -> Vec<GhPull> {
    let mut pulls_by_id = HashMap::new();
    for pull in created {
        pulls_by_id.insert(pull.id, pull);
    }
    for pull in updated {
        pulls_by_id.insert(pull.id, pull);
    }
    pulls_by_id.into_values().collect()
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

fn user_login_or_unknown(user: Option<&GhUser>) -> String {
    user.map(|u| u.login.clone())
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_number_from_url(url: &str) -> Option<i64> {
    let tail = url.rsplit('/').next()?;
    let tail = tail.split('?').next().unwrap_or(tail);
    tail.parse::<i64>().ok()
}

fn extract_mentions(text: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'@' {
            idx += 1;
            continue;
        }
        idx += 1;
        let start = idx;
        while idx < bytes.len() {
            let ch = bytes[idx] as char;
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                idx += 1;
                continue;
            }
            break;
        }
        if idx > start {
            mentions.push(text[start..idx].to_string());
        }
    }
    mentions
}

#[derive(Debug, Clone, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GhPull {
    id: i64,
    number: Option<i64>,
    title: String,
    html_url: String,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
    merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    requested_reviewers: Vec<GhUser>,
    merged_by: Option<GhUser>,
    user: Option<GhUser>,
}

impl GhPull {
    fn number_or_id(&self) -> i64 {
        self.number.unwrap_or(self.id)
    }
}

#[derive(Debug, Deserialize)]
struct GhIssue {
    id: i64,
    number: Option<i64>,
    title: String,
    html_url: String,
    created_at: DateTime<Utc>,
    user: Option<GhUser>,
    pull_request: Option<serde_json::Value>,
}

impl GhIssue {
    fn number_or_id(&self) -> i64 {
        self.number.unwrap_or(self.id)
    }
}

#[derive(Debug, Deserialize)]
struct GhComment {
    id: i64,
    issue_url: Option<String>,
    pull_request_url: Option<String>,
    pull_request_review_id: Option<i64>,
    html_url: String,
    created_at: DateTime<Utc>,
    body: Option<String>,
    user: Option<GhUser>,
}
