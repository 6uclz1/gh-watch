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
    ports::{
        CursorPort, GhClientPort, NotifierPort, PersistBatchResult, RepoBatchPort,
        RepoPersistBatch, RetentionPort,
    },
};

struct DryRunStateStore<'a, S> {
    inner: &'a S,
}

impl<'a, S> DryRunStateStore<'a, S> {
    fn new(inner: &'a S) -> Self {
        Self { inner }
    }
}

impl<S> CursorPort for DryRunStateStore<'_, S>
where
    S: CursorPort,
{
    fn get_cursor(&self, repo: &str) -> Result<Option<DateTime<Utc>>> {
        self.inner.get_cursor(repo)
    }

    fn set_cursor(&self, _repo: &str, _at: DateTime<Utc>) -> Result<()> {
        Ok(())
    }
}

impl<S> RetentionPort for DryRunStateStore<'_, S>
where
    S: Sync,
{
    fn cleanup_old(&self, _retention_days: u32, _now: DateTime<Utc>) -> Result<()> {
        Ok(())
    }
}

impl<S> RepoBatchPort for DryRunStateStore<'_, S>
where
    S: Sync,
{
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
