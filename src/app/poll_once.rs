use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use std::{collections::HashSet, time::Duration as StdDuration};

use crate::{
    config::Config,
    domain::{
        events::{event_matches_notification_filters, EventKind, WatchEvent},
        failure::{FailureRecord, FAILURE_KIND_NOTIFICATION, FAILURE_KIND_REPO_POLL},
    },
    ports::{ClockPort, GhClientPort, NotifierPort, RepoPersistBatch, StateStorePort},
};

const POLL_OVERLAP_SECONDS: i64 = 300;
const NOTIFICATION_QUEUE_DRAIN_LIMIT: usize = 256;

#[derive(Debug, Clone, Default, Serialize)]
pub struct PollOutcome {
    pub notified_count: usize,
    pub bootstrap_repos: usize,
    pub repo_errors: Vec<String>,
    pub failures: Vec<FailureRecord>,
    pub notified_events: Vec<WatchEvent>,
    pub timeline_events: Vec<WatchEvent>,
    pub pending_notification_count: usize,
}

#[derive(Debug, Clone)]
struct RepoPollPlan {
    repo_name: String,
    since: chrono::DateTime<Utc>,
    poll_started_at: chrono::DateTime<Utc>,
    is_bootstrap: bool,
    allowed_event_kinds: Vec<EventKind>,
}

enum RepoFetchResult {
    Fetched {
        plan: RepoPollPlan,
        events: Vec<WatchEvent>,
    },
    Failed {
        repo_name: String,
        error_message: String,
    },
}

struct RepoEventProcessingContext<'a, S, N, K> {
    config: &'a Config,
    state: &'a S,
    notifier: &'a N,
    clock: &'a K,
    viewer_login: Option<String>,
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
    let viewer_login = if config.filters.only_involving_me {
        Some(
            gh.viewer_login()
                .await
                .context("failed to resolve viewer login for only_involving_me filter")?,
        )
    } else {
        None
    };

    let mut outcome = PollOutcome::default();
    let plans = plan_window(config, state, clock, &mut outcome)?;
    let fetch_results = collect_events(config, gh, plans).await;

    let processing_context = RepoEventProcessingContext {
        config,
        state,
        notifier,
        clock,
        viewer_login,
    };

    for fetch_result in fetch_results {
        match fetch_result {
            RepoFetchResult::Fetched { plan, events } => {
                let repo_name = plan.repo_name.clone();
                if let Err(err) = persist_batch(&processing_context, &mut outcome, plan, events) {
                    record_repo_poll_failure(
                        state,
                        clock,
                        &mut outcome,
                        repo_name.as_str(),
                        format!("failed to persist repo batch: {err:#}").as_str(),
                    );
                }
            }
            RepoFetchResult::Failed {
                repo_name,
                error_message,
            } => record_repo_poll_failure(
                state,
                clock,
                &mut outcome,
                repo_name.as_str(),
                error_message.as_str(),
            ),
        }
    }

    drain_notification_queue(&processing_context, &mut outcome);
    outcome.pending_notification_count = state.pending_notification_count().with_context(|| {
        "failed to load pending notification count after queue drain".to_string()
    })?;

    Ok(outcome)
}

fn plan_window<S, K>(
    config: &Config,
    state: &S,
    clock: &K,
    outcome: &mut PollOutcome,
) -> Result<Vec<RepoPollPlan>>
where
    S: StateStorePort,
    K: ClockPort,
{
    let mut plans = Vec::new();
    for repo in config.repositories.iter().filter(|r| r.enabled) {
        let cursor = match state
            .get_cursor(&repo.name)
            .with_context(|| format!("failed to load cursor for {}", repo.name))
        {
            Ok(cursor) => cursor,
            Err(err) => {
                record_repo_poll_failure(
                    state,
                    clock,
                    outcome,
                    repo.name.as_str(),
                    format!("{err:#}").as_str(),
                );
                continue;
            }
        };

        let poll_started_at = clock.now();
        let allowed_event_kinds = repo
            .event_kinds
            .clone()
            .unwrap_or_else(|| config.filters.event_kinds.clone());

        let Some(cursor) = cursor else {
            outcome.bootstrap_repos += 1;
            if config.bootstrap_lookback_hours == 0 {
                let batch = RepoPersistBatch {
                    repo: repo.name.clone(),
                    poll_started_at,
                    events: Vec::new(),
                    queue_notifications: false,
                };
                if let Err(err) = state.persist_repo_batch(&batch) {
                    record_repo_poll_failure(
                        state,
                        clock,
                        outcome,
                        repo.name.as_str(),
                        format!("failed to set bootstrap cursor for {}: {err:#}", repo.name)
                            .as_str(),
                    );
                }
                continue;
            }

            plans.push(RepoPollPlan {
                repo_name: repo.name.clone(),
                since: bootstrap_since(poll_started_at, config.bootstrap_lookback_hours),
                poll_started_at,
                is_bootstrap: true,
                allowed_event_kinds,
            });
            continue;
        };

        plans.push(RepoPollPlan {
            repo_name: repo.name.clone(),
            since: with_fixed_overlap(cursor),
            poll_started_at,
            is_bootstrap: false,
            allowed_event_kinds,
        });
    }

    Ok(plans)
}

fn bootstrap_since(now: chrono::DateTime<Utc>, lookback_hours: u64) -> chrono::DateTime<Utc> {
    let bounded_hours = lookback_hours.min(i64::MAX as u64) as i64;
    now.checked_sub_signed(Duration::hours(bounded_hours))
        .unwrap_or(now)
}

fn with_fixed_overlap(cursor: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    cursor
        .checked_sub_signed(Duration::seconds(POLL_OVERLAP_SECONDS))
        .unwrap_or(cursor)
}

async fn collect_events<C>(
    config: &Config,
    gh: &C,
    plans: Vec<RepoPollPlan>,
) -> Vec<RepoFetchResult>
where
    C: GhClientPort,
{
    let timeout = StdDuration::from_secs(config.poll.timeout_seconds);
    let timeout_seconds = config.poll.timeout_seconds;
    let mut fetches = stream::iter(plans.into_iter().map(|plan| async move {
        let result =
            tokio::time::timeout(timeout, gh.fetch_repo_events(&plan.repo_name, plan.since)).await;
        match result {
            Ok(Ok(events)) => RepoFetchResult::Fetched { plan, events },
            Ok(Err(err)) => RepoFetchResult::Failed {
                repo_name: plan.repo_name,
                error_message: err.to_string(),
            },
            Err(_) => RepoFetchResult::Failed {
                repo_name: plan.repo_name,
                error_message: format!("repo polling timed out after {timeout_seconds}s"),
            },
        }
    }))
    .buffer_unordered(config.poll.max_concurrency);

    let mut results = Vec::new();
    while let Some(fetch_result) = fetches.next().await {
        results.push(fetch_result);
    }
    results
}

fn persist_batch<S, N, K>(
    context: &RepoEventProcessingContext<'_, S, N, K>,
    outcome: &mut PollOutcome,
    plan: RepoPollPlan,
    events: Vec<WatchEvent>,
) -> Result<()>
where
    S: StateStorePort,
    N: NotifierPort,
    K: ClockPort,
{
    let mut events = events
        .into_iter()
        .filter(|event| event.created_at <= plan.poll_started_at)
        .collect::<Vec<_>>();

    if !plan.is_bootstrap {
        events.retain(|event| {
            event_matches_notification_filters(
                event,
                &plan.allowed_event_kinds,
                &context.config.filters.ignore_actors,
                context.config.filters.only_involving_me,
                context.viewer_login.as_deref(),
            )
        });
    }

    let batch = RepoPersistBatch {
        repo: plan.repo_name.clone(),
        poll_started_at: plan.poll_started_at,
        events: events.clone(),
        queue_notifications: !plan.is_bootstrap && context.config.notifications.enabled,
    };
    let persist_result = context
        .state
        .persist_repo_batch(&batch)
        .with_context(|| format!("failed to persist event batch for {}", plan.repo_name))?;

    let newly_logged = persist_result
        .newly_logged_event_keys
        .into_iter()
        .collect::<HashSet<_>>();
    outcome.timeline_events.extend(
        events
            .into_iter()
            .filter(|event| newly_logged.contains(&event.event_key())),
    );

    Ok(())
}

fn drain_notification_queue<S, N, K>(
    context: &RepoEventProcessingContext<'_, S, N, K>,
    outcome: &mut PollOutcome,
) where
    S: StateStorePort,
    N: NotifierPort,
    K: ClockPort,
{
    if !context.config.notifications.enabled {
        return;
    }

    let now = context.clock.now();
    let due_items = match context
        .state
        .load_due_notifications(now, NOTIFICATION_QUEUE_DRAIN_LIMIT)
    {
        Ok(items) => items,
        Err(err) => {
            record_repo_poll_failure(
                context.state,
                context.clock,
                outcome,
                "<notification_queue>",
                format!("failed to load due notifications: {err:#}").as_str(),
            );
            return;
        }
    };

    for pending in due_items {
        match context
            .notifier
            .notify(&pending.event, context.config.notifications.include_url)
        {
            Ok(_) => {
                if let Err(err) = context
                    .state
                    .mark_notification_delivered(&pending.event_key, now)
                {
                    record_repo_poll_failure(
                        context.state,
                        context.clock,
                        outcome,
                        pending.event.repo.as_str(),
                        format!(
                            "notification delivered but failed to persist delivery state for {}: {err:#}",
                            pending.event_key
                        )
                        .as_str(),
                    );
                    continue;
                }
                outcome.notified_count += 1;
                outcome.notified_events.push(pending.event);
            }
            Err(err) => {
                let attempts = pending.attempts.saturating_add(1);
                let next_attempt_at =
                    now + Duration::seconds(notification_retry_backoff_seconds(attempts) as i64);
                let err_message = err.to_string();

                if let Err(reschedule_err) = context.state.reschedule_notification(
                    &pending.event_key,
                    attempts,
                    next_attempt_at,
                    &err_message,
                ) {
                    record_repo_poll_failure(
                        context.state,
                        context.clock,
                        outcome,
                        pending.event.repo.as_str(),
                        format!(
                            "failed to reschedule notification for {}: {reschedule_err:#}",
                            pending.event_key
                        )
                        .as_str(),
                    );
                }

                let failure = FailureRecord::new(
                    FAILURE_KIND_NOTIFICATION,
                    pending.event.repo.clone(),
                    context.clock.now(),
                    format!("{}: {}", pending.event_key, err_message),
                );
                if let Err(record_err) = context.state.record_failure(&failure) {
                    outcome.repo_errors.push(format!(
                        "{}: notification failed for {} but failure record persistence failed: {}",
                        pending.event.repo, pending.event_key, record_err
                    ));
                } else {
                    outcome.failures.push(failure);
                    outcome.repo_errors.push(format!(
                        "{}: notification failed for {}: {}",
                        pending.event.repo, pending.event_key, err_message
                    ));
                }
            }
        }
    }
}

fn notification_retry_backoff_seconds(attempts: u32) -> u64 {
    match attempts {
        0 => 0,
        1 => 30,
        2 => 120,
        3 => 600,
        4 => 3600,
        _ => 21600,
    }
}

fn record_repo_poll_failure<S, K>(
    state: &S,
    clock: &K,
    outcome: &mut PollOutcome,
    repo_name: &str,
    error_message: &str,
) where
    S: StateStorePort,
    K: ClockPort,
{
    let failure = FailureRecord::new(
        FAILURE_KIND_REPO_POLL,
        repo_name.to_string(),
        clock.now(),
        error_message.to_string(),
    );

    let mut rendered = format!("{repo_name}: {error_message}");
    if let Err(err) = state.record_failure(&failure) {
        rendered.push_str(&format!(" | failed to persist failure record: {err}"));
    } else {
        outcome.failures.push(failure);
    }

    outcome.repo_errors.push(rendered);
}
