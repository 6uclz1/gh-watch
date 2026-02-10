use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Utc};
use serde::Serialize;
use std::{collections::HashSet, time::Duration as StdDuration};

use crate::{
    config::Config,
    domain::events::{event_matches_notification_filters, EventKind, WatchEvent},
    ports::{ClockPort, GhClientPort, NotifierPort, PollStatePort, RepoPersistBatch},
};

const POLL_OVERLAP_SECONDS: i64 = 300;
const REPO_FETCH_MAX_ATTEMPTS: usize = 3;
const REPO_FETCH_RETRY_BACKOFFS_SECONDS: [u64; REPO_FETCH_MAX_ATTEMPTS - 1] = [1, 2];

#[derive(Debug, Clone, Default, Serialize)]
pub struct RepoFetchFailure {
    pub repo: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PollOutcome {
    pub notified_count: usize,
    pub bootstrap_repos: usize,
    pub notified_events: Vec<WatchEvent>,
    pub timeline_events: Vec<WatchEvent>,
    pub fetch_failures: Vec<RepoFetchFailure>,
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

struct PollPlanner<'a, S, K> {
    config: &'a Config,
    state: &'a S,
    clock: &'a K,
}

impl<'a, S, K> PollPlanner<'a, S, K>
where
    S: PollStatePort,
    K: ClockPort,
{
    fn new(config: &'a Config, state: &'a S, clock: &'a K) -> Self {
        Self {
            config,
            state,
            clock,
        }
    }

    fn build(&self) -> Result<Vec<RepoPollPlan>> {
        let mut plans = Vec::new();

        for repo in self.config.repositories.iter().filter(|r| r.enabled) {
            let cursor = self
                .state
                .get_cursor(&repo.name)
                .with_context(|| format!("failed to load cursor for {}", repo.name))?;

            let poll_started_at = self.clock.now();
            let allowed_event_kinds = repo
                .event_kinds
                .clone()
                .unwrap_or_else(|| self.config.filters.event_kinds.clone());

            match cursor {
                Some(cursor) => plans.push(RepoPollPlan {
                    repo_name: repo.name.clone(),
                    since: with_fixed_overlap(cursor),
                    poll_started_at,
                    is_bootstrap: false,
                    allowed_event_kinds,
                }),
                None => plans.push(RepoPollPlan {
                    repo_name: repo.name.clone(),
                    since: bootstrap_since(poll_started_at, self.config.bootstrap_lookback_hours),
                    poll_started_at,
                    is_bootstrap: true,
                    allowed_event_kinds,
                }),
            }
        }

        Ok(plans)
    }
}

struct RepoEventCollector<'a, C> {
    config: &'a Config,
    gh: &'a C,
}

impl<'a, C> RepoEventCollector<'a, C>
where
    C: GhClientPort,
{
    fn new(config: &'a Config, gh: &'a C) -> Self {
        Self { config, gh }
    }

    async fn collect(&self, plans: Vec<RepoPollPlan>) -> Vec<RepoFetchResult> {
        let mut results = Vec::new();
        for plan in plans {
            results.push(self.fetch_with_retry(plan).await);
        }
        results
    }

    async fn fetch_with_retry(&self, plan: RepoPollPlan) -> RepoFetchResult {
        let timeout = StdDuration::from_secs(self.config.poll.timeout_seconds);
        let timeout_seconds = self.config.poll.timeout_seconds;
        let mut fetched_events: Option<Vec<WatchEvent>> = None;
        let mut last_error = String::new();

        for attempt in 1..=REPO_FETCH_MAX_ATTEMPTS {
            let result = tokio::time::timeout(
                timeout,
                self.gh.fetch_repo_events(&plan.repo_name, plan.since),
            )
            .await;

            match result {
                Ok(Ok(events)) => {
                    fetched_events = Some(events);
                    break;
                }
                Ok(Err(err)) => {
                    last_error = err.to_string();
                }
                Err(_) => {
                    last_error = format!("repo polling timed out after {timeout_seconds}s");
                }
            }

            if attempt < REPO_FETCH_MAX_ATTEMPTS {
                let backoff_index = attempt - 1;
                let wait_seconds = REPO_FETCH_RETRY_BACKOFFS_SECONDS[backoff_index];
                tokio::time::sleep(StdDuration::from_secs(wait_seconds)).await;
            }
        }

        match fetched_events {
            Some(events) => RepoFetchResult::Fetched { plan, events },
            None => RepoFetchResult::Failed {
                repo_name: plan.repo_name,
                error_message: last_error,
            },
        }
    }
}

struct RepoBatchProcessor<'a, S, N> {
    context: RepoEventProcessingContext<'a, S, N>,
}

impl<'a, S, N> RepoBatchProcessor<'a, S, N>
where
    S: PollStatePort,
    N: NotifierPort,
{
    fn new(
        config: &'a Config,
        state: &'a S,
        notifier: &'a N,
        viewer_login: Option<String>,
    ) -> Self {
        Self {
            context: RepoEventProcessingContext {
                config,
                state,
                notifier,
                viewer_login,
            },
        }
    }

    fn apply(&self, outcome: &mut PollOutcome, fetch_result: RepoFetchResult) -> Result<()> {
        match fetch_result {
            RepoFetchResult::Fetched { plan, events } => {
                self.persist_and_notify(outcome, plan, events)?;
            }
            RepoFetchResult::Failed {
                repo_name,
                error_message,
            } => outcome.fetch_failures.push(RepoFetchFailure {
                repo: repo_name,
                message: error_message,
            }),
        }

        Ok(())
    }

    fn persist_and_notify(
        &self,
        outcome: &mut PollOutcome,
        plan: RepoPollPlan,
        events: Vec<WatchEvent>,
    ) -> Result<()> {
        let mut events = events
            .into_iter()
            .filter(|event| event.created_at <= plan.poll_started_at)
            .collect::<Vec<_>>();

        if !plan.is_bootstrap {
            events.retain(|event| {
                event_matches_notification_filters(
                    event,
                    &plan.allowed_event_kinds,
                    &self.context.config.filters.ignore_actors,
                    self.context.config.filters.only_involving_me,
                    self.context.viewer_login.as_deref(),
                )
            });
        }

        let batch = RepoPersistBatch {
            repo: plan.repo_name.clone(),
            poll_started_at: plan.poll_started_at,
            events: events.clone(),
        };
        let persist_result = self
            .context
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

        if plan.is_bootstrap || !self.context.config.notifications.enabled {
            return Ok(());
        }

        for event in newly_logged_events {
            self.context
                .notifier
                .notify(&event, self.context.config.notifications.include_url)
                .with_context(|| format!("notification failed for {}", event.event_key()))?;
            outcome.notified_count += 1;
            outcome.notified_events.push(event);
        }

        Ok(())
    }
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
    S: PollStatePort,
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

    let plans = PollPlanner::new(config, state, clock).build()?;
    let fetch_results = RepoEventCollector::new(config, gh).collect(plans).await;
    let fetched_repo_count = fetch_results
        .iter()
        .filter(|result| matches!(result, RepoFetchResult::Fetched { .. }))
        .count();

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

    let processor = RepoBatchProcessor::new(config, state, notifier, viewer_login);
    for fetch_result in fetch_results {
        processor.apply(&mut outcome, fetch_result)?;
    }

    if fetched_repo_count == 0 && !outcome.fetch_failures.is_empty() {
        let details = outcome
            .fetch_failures
            .iter()
            .map(|failure| format!("{}: {}", failure.repo, failure.message))
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(anyhow!("all repository fetches failed: {details}"));
    }

    Ok(outcome)
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
