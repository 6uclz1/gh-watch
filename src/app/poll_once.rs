use anyhow::{Context, Result};

use crate::{
    config::Config,
    domain::events::WatchEvent,
    ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort},
};

#[derive(Debug, Clone, Default)]
pub struct PollOutcome {
    pub notified_count: usize,
    pub bootstrap_repos: usize,
    pub repo_errors: Vec<String>,
    pub notified_events: Vec<WatchEvent>,
    pub timeline_events: Vec<WatchEvent>,
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

    let mut outcome = PollOutcome::default();

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

        let events = match gh.fetch_repo_events(&repo.name, since).await {
            Ok(events) => events,
            Err(err) => {
                outcome.repo_errors.push(format!("{}: {}", repo.name, err));
                continue;
            }
        };

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
                        outcome.repo_errors.push(format!(
                            "notification failed for {}: {}",
                            event.event_key(),
                            err
                        ));
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

        state
            .set_cursor(&repo.name, now)
            .with_context(|| format!("failed to update cursor for {}", repo.name))?;
    }

    Ok(outcome)
}
