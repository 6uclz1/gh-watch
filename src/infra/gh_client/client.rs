use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

use crate::{domain::events::WatchEvent, ports::GhClientPort};

use super::{
    models::{GhComment, GhIssue, GhPull},
    normalize::{merge_pulls_by_id, normalize_events_from_items},
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
