use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::{
    app::poll_once::poll_once,
    cli::{
        state::{open_state_store, resolve_state_db_path},
        SystemClock,
    },
    config::{Config, ResolvedConfigPath},
    infra::{gh_client::GhCliClient, notifier::DesktopNotifier},
    ports::{GhClientPort, NotifierPort, PersistBatchResult, RepoPersistBatch, StateStorePort},
};

struct DryRunStateStore<'a, S> {
    inner: &'a S,
}

impl<'a, S> DryRunStateStore<'a, S> {
    fn new(inner: &'a S) -> Self {
        Self { inner }
    }
}

impl<S> StateStorePort for DryRunStateStore<'_, S>
where
    S: StateStorePort,
{
    fn get_cursor(&self, repo: &str) -> Result<Option<DateTime<Utc>>> {
        self.inner.get_cursor(repo)
    }

    fn set_cursor(&self, _repo: &str, _at: DateTime<Utc>) -> Result<()> {
        Ok(())
    }

    fn load_timeline_events(&self, limit: usize) -> Result<Vec<crate::domain::events::WatchEvent>> {
        self.inner.load_timeline_events(limit)
    }

    fn mark_timeline_event_read(&self, _event_key: &str, _read_at: DateTime<Utc>) -> Result<()> {
        Ok(())
    }

    fn load_read_event_keys(&self, event_keys: &[String]) -> Result<HashSet<String>> {
        self.inner.load_read_event_keys(event_keys)
    }

    fn cleanup_old(&self, _retention_days: u32, _now: DateTime<Utc>) -> Result<()> {
        Ok(())
    }

    fn persist_repo_batch(&self, _batch: &RepoPersistBatch) -> Result<PersistBatchResult> {
        Ok(PersistBatchResult::default())
    }
}

pub(crate) async fn run(
    cfg: Config,
    resolved_config: ResolvedConfigPath,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    for warning in crate::config::stability_warnings(&cfg) {
        eprintln!("{warning}");
    }

    let gh = GhCliClient::default();
    gh.check_auth()
        .await
        .context("GitHub authentication is invalid. Run `gh auth login -h github.com`.")?;

    let state_path = resolve_state_db_path(&cfg)?;
    let state = open_state_store(&state_path)?;

    let notifier = DesktopNotifier::from_notification_config(&cfg.notifications);
    for warning in notifier.startup_warnings() {
        eprintln!("notification backend warning: {warning}");
    }
    notifier
        .check_health()
        .context("Notification backend check failed")?;

    let outcome = if dry_run {
        let dry_run_state = DryRunStateStore::new(&state);
        poll_once(&cfg, &gh, &dry_run_state, &notifier, &SystemClock).await?
    } else {
        poll_once(&cfg, &gh, &state, &notifier, &SystemClock).await?
    };

    if json {
        println!("{}", serde_json::to_string(&outcome)?);
    } else {
        println!(
            "config: {} (source: {})",
            resolved_config.path.display(),
            resolved_config.source
        );
        println!("notified: {}", outcome.notified_count);
        println!("bootstrap_repos: {}", outcome.bootstrap_repos);
        println!("repo_fetch_failures: {}", outcome.fetch_failures.len());
        for failure in &outcome.fetch_failures {
            println!("- {}: {}", failure.repo, failure.message);
        }
        if dry_run {
            println!("mode: dry-run (state unchanged)");
        }
    }

    Ok(())
}
