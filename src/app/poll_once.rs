use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Utc};
use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use std::{collections::HashSet, time::Duration as StdDuration};

use crate::{
    config::Config,
    domain::events::{event_matches_notification_filters, EventKind, WatchEvent},
    ports::{ClockPort, GhClientPort, NotifierPort, RepoPersistBatch, StateStorePort},
};

const POLL_OVERLAP_SECONDS: i64 = 300;

#[derive(Debug, Clone, Default, Serialize)]
pub struct PollOutcome {
    pub notified_count: usize,
    pub bootstrap_repos: usize,
    pub notified_events: Vec<WatchEvent>,
    pub timeline_events: Vec<WatchEvent>,
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

struct RepoEventProcessingContext<'a, S, N> {
    config: &'a Config,
    state: &'a S,
    notifier: &'a N,
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
    state.cleanup_old(config.retention_days, now)?;
    let viewer_login = if config.filters.only_involving_me {
        Some(
            gh.viewer_login()
                .await
                .context("failed to resolve viewer login for only_involving_me filter")?,
        )
    } else {
        None
    };

    let plans = plan_window(config, state, clock)?;
    let fetch_results = collect_events(config, gh, plans).await;
    let mut outcome = PollOutcome {
        bootstrap_repos: fetch_results
            .iter()
            .filter(|result| {
                matches!(
                    result,
                    RepoFetchResult::Fetched {
                        plan: RepoPollPlan {
                            is_bootstrap: true,
                            ..
                        },
                        ..
                    }
                )
            })
            .count(),
        ..PollOutcome::default()
    };

    let processing_context = RepoEventProcessingContext {
        config,
        state,
        notifier,
        viewer_login,
    };

    for fetch_result in fetch_results {
        match fetch_result {
            RepoFetchResult::Fetched { plan, events } => {
                persist_batch(&processing_context, &mut outcome, plan, events)?;
            }
            RepoFetchResult::Failed {
                repo_name,
                error_message,
            } => {
                return Err(anyhow!("{repo_name}: {error_message}"));
            }
        }
    }

    Ok(outcome)
}

fn plan_window<S, K>(config: &Config, state: &S, clock: &K) -> Result<Vec<RepoPollPlan>>
where
    S: StateStorePort,
    K: ClockPort,
{
    let mut plans = Vec::new();
    for repo in config.repositories.iter().filter(|r| r.enabled) {
        let cursor = state
            .get_cursor(&repo.name)
            .with_context(|| format!("failed to load cursor for {}", repo.name))?;

        let poll_started_at = clock.now();
        let allowed_event_kinds = repo
            .event_kinds
            .clone()
            .unwrap_or_else(|| config.filters.event_kinds.clone());

        let Some(cursor) = cursor else {
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

fn persist_batch<S, N>(
    context: &RepoEventProcessingContext<'_, S, N>,
    outcome: &mut PollOutcome,
    plan: RepoPollPlan,
    events: Vec<WatchEvent>,
) -> Result<()>
where
    S: StateStorePort,
    N: NotifierPort,
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
    };
    let persist_result = context
        .state
        .persist_repo_batch(&batch)
        .with_context(|| format!("failed to persist event batch for {}", plan.repo_name))?;

    let newly_logged = persist_result
        .newly_logged_event_keys
        .into_iter()
        .collect::<HashSet<_>>();
    let newly_logged_events = events
        .into_iter()
        .filter(|event| newly_logged.contains(&event.event_key()))
        .collect::<Vec<_>>();
    outcome.timeline_events.extend(newly_logged_events.clone());

    if plan.is_bootstrap || !context.config.notifications.enabled {
        return Ok(());
    }

    for event in newly_logged_events {
        context
            .notifier
            .notify(&event, context.config.notifications.include_url)
            .with_context(|| format!("notification failed for {}", event.event_key()))?;
        outcome.notified_count += 1;
        outcome.notified_events.push(event);
    }

    Ok(())
}
