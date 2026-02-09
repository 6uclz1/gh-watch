use anyhow::{Context, Result};

use crate::{
    app::watch_loop::run_watch,
    cli::{
        state::{open_state_store, resolve_state_db_path},
        SystemClock,
    },
    config::{Config, ResolvedConfigPath},
    infra::{gh_client::GhCliClient, notifier::DesktopNotifier},
    ports::{GhClientPort, NotifierPort},
};

pub(crate) async fn run(cfg: Config, resolved_config: ResolvedConfigPath) -> Result<()> {
    eprintln!(
        "config: {} (source: {})",
        resolved_config.path.display(),
        resolved_config.source
    );

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

    run_watch(&cfg, &gh, &state, &notifier, &SystemClock).await
}
