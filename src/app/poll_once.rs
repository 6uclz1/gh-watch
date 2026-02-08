use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use futures_util::stream::{self, StreamExt};
use std::time::Duration as StdDuration;

use crate::{
    config::Config,
    domain::{
        events::WatchEvent,
        failure::{FailureRecord, FAILURE_KIND_NOTIFICATION, FAILURE_KIND_REPO_POLL},
    },
    ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort},
};

#[derive(Debug, Clone, Default)]
pub struct PollOutcome {
    pub notified_count: usize,
    pub bootstrap_repos: usize,
    pub repo_errors: Vec<String>,
    pub failures: Vec<FailureRecord>,
    pub notified_events: Vec<WatchEvent>,
    pub timeline_events: Vec<WatchEvent>,
}

enum RepoFetchResult {
    Fetched {
        repo_name: String,
        since: chrono::DateTime<Utc>,
        events: Vec<WatchEvent>,
    },
    Failed {
        repo_name: String,
        error_message: String,
    },
}

pub async fn poll_once<C, S, N, K>(
    config: &Config,
    gh: &C,
    state: &S,
    notifier: &N,
    clock: &K,
) -> Result<PollOutcome>
where
    C: GhClientPort,
    S: StateStorePort,
    N: NotifierPort,
    K: ClockPort,
{
    let now = clock.now();
    state.cleanup_old(config.retention_days, config.failure_history_limit, now)?;

    let mut outcome = PollOutcome::default();
    let mut repos_to_fetch = Vec::new();

    for repo in config.repositories.iter().filter(|r| r.enabled) {
        let cursor = state
            .get_cursor(&repo.name)
            .with_context(|| format!("failed to load cursor for {}", repo.name))?;

        let Some(since) = cursor else {
            state
                .set_cursor(&repo.name, now)
                .with_context(|| format!("failed to set bootstrap cursor for {}", repo.name))?;
            outcome.bootstrap_repos += 1;
            continue;
        };

        repos_to_fetch.push((repo.name.clone(), since));
    }

    let timeout = StdDuration::from_secs(config.poll.timeout_seconds);
    let max_concurrency = config.poll.max_concurrency;
    let timeout_seconds = config.poll.timeout_seconds;

    let mut fetches = stream::iter(repos_to_fetch.into_iter().map(|(repo_name, since)| async move {
        let result = tokio::time::timeout(timeout, gh.fetch_repo_events(&repo_name, since)).await;
        match result {
            Ok(Ok(events)) => RepoFetchResult::Fetched {
                repo_name,
                since,
                events,
            },
            Ok(Err(err)) => RepoFetchResult::Failed {
                repo_name,
                error_message: err.to_string(),
            },
            Err(_) => RepoFetchResult::Failed {
                repo_name,
                error_message: format!("repo polling timed out after {timeout_seconds}s"),
            },
        }
    }))
    .buffer_unordered(max_concurrency);

    while let Some(fetch_result) = fetches.next().await {
        match fetch_result {
            RepoFetchResult::Fetched {
                repo_name,
                since,
                events,
            } => process_repo_events(
                config,
                state,
                notifier,
                clock,
                &mut outcome,
                repo_name.as_str(),
                since,
                events,
                now,
            )?,
            RepoFetchResult::Failed {
                repo_name,
                error_message,
            } => record_repo_poll_failure(
                state,
                clock,
                &mut outcome,
                repo_name.as_str(),
                error_message.as_str(),
            )?,
        }
    }

    Ok(outcome)
}

fn process_repo_events<S, N, K>(
    config: &Config,
    state: &S,
    notifier: &N,
    clock: &K,
    outcome: &mut PollOutcome,
    repo_name: &str,
    since: chrono::DateTime<Utc>,
    events: Vec<WatchEvent>,
    now: chrono::DateTime<Utc>,
) -> Result<()>
where
    S: StateStorePort,
    N: NotifierPort,
    K: ClockPort,
{
    let mut earliest_notification_failure_at: Option<chrono::DateTime<Utc>> = None;

    for event in events {
        let event_key = event.event_key();
        let already_notified = state.is_event_notified(&event_key)?;
        if already_notified {
            continue;
        }

        if config.notifications.enabled {
            match notifier.notify(&event, config.notifications.include_url) {
                Ok(_) => {}
                Err(err) => {
                    let failure = FailureRecord::new(
                        FAILURE_KIND_NOTIFICATION,
                        event.repo.clone(),
                        clock.now(),
                        format!("{}: {}", event.event_key(), err),
                    );
                    state.record_failure(&failure).with_context(|| {
                        format!(
                            "failed to record notification failure for {}",
                            event.event_key()
                        )
                    })?;
                    outcome.failures.push(failure);
                    outcome.repo_errors.push(format!(
                        "notification failed for {}: {}",
                        event.event_key(),
                        err
                    ));
                    earliest_notification_failure_at = Some(
                        earliest_notification_failure_at
                            .map(|at| at.min(event.created_at))
                            .unwrap_or(event.created_at),
                    );
                    // Keep the event unrecorded if notification delivery failed.
                    continue;
                }
            }
        }

        state.record_notified_event(&event, now)?;
        state.append_timeline_event(&event)?;
        outcome.notified_count += 1;
        outcome.notified_events.push(event.clone());
        outcome.timeline_events.push(event);
    }

    let next_cursor = if let Some(failure_at) = earliest_notification_failure_at {
        // Keep failed events in the next query window (`created_at > cursor`).
        failure_at
            .checked_sub_signed(Duration::nanoseconds(1))
            .unwrap_or(since)
    } else {
        now
    };

    state
        .set_cursor(repo_name, next_cursor)
        .with_context(|| format!("failed to update cursor for {repo_name}"))?;

    Ok(())
}

fn record_repo_poll_failure<S, K>(
    state: &S,
    clock: &K,
    outcome: &mut PollOutcome,
    repo_name: &str,
    error_message: &str,
) -> Result<()>
where
    S: StateStorePort,
    K: ClockPort,
{
    let failure = FailureRecord::new(
        FAILURE_KIND_REPO_POLL,
        repo_name.to_string(),
        clock.now(),
        error_message.to_string(),
    );
    state
        .record_failure(&failure)
        .with_context(|| format!("failed to record repo polling failure for {repo_name}"))?;
    outcome.failures.push(failure);
    outcome
        .repo_errors
        .push(format!("{repo_name}: {error_message}"));
    Ok(())
}
