use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::domain::events::{EventKind, WatchEvent};

use super::models::{GhComment, GhIssue, GhPull, GhUser};

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

pub(super) fn normalize_events_from_items(
    repo: &str,
    since: DateTime<Utc>,
    pulls: Vec<GhPull>,
    issues: Vec<GhIssue>,
    issue_comments: Vec<GhComment>,
    review_comments: Vec<GhComment>,
) -> Vec<WatchEvent> {
    let mut events = Vec::new();
    let draft_pull_numbers = pulls
        .iter()
        .filter(|pr| pr.draft)
        .map(GhPull::number_or_id)
        .collect::<HashSet<_>>();

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

    events.extend(
        pulls
            .iter()
            .filter(|pr| !pr.draft && pr.created_at > since)
            .map(|pr| {
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
            }),
    );

    events.extend(
        pulls
            .iter()
            .filter_map(|pr| {
                if pr.draft {
                    return None;
                }
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
        if pr.draft {
            continue;
        }
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
            .filter(|comment| {
                !references_draft_pull(comment.issue_url.as_deref(), &draft_pull_numbers)
            })
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
        .filter(|comment| {
            !references_draft_pull(comment.pull_request_url.as_deref(), &draft_pull_numbers)
        })
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

pub(super) fn merge_pulls_by_id(created: Vec<GhPull>, updated: Vec<GhPull>) -> Vec<GhPull> {
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

fn references_draft_pull(url: Option<&str>, draft_pull_numbers: &HashSet<i64>) -> bool {
    url.and_then(parse_number_from_url)
        .is_some_and(|number| draft_pull_numbers.contains(&number))
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
