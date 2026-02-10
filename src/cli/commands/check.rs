use anyhow::{Context, Result};

use crate::{
    cli::state::{open_state_store, resolve_state_db_path},
    config::{Config, ResolvedConfigPath},
    infra::{gh_client::GhCliClient, notifier::DesktopNotifier},
    ports::{GhClientPort, NotifierPort},
};

pub(crate) async fn run(cfg: Config, resolved_config: ResolvedConfigPath) -> Result<()> {
    for warning in crate::config::stability_warnings(&cfg) {
        eprintln!("{warning}");
    }

    let gh = GhCliClient::default();
    gh.check_auth()
        .await
        .context("GitHub authentication is invalid. Run `gh auth login -h github.com`.")?;

    let notifier = DesktopNotifier::from_notification_config(&cfg.notifications);
    for warning in notifier.startup_warnings() {
        eprintln!("notification backend warning: {warning}");
    }
    notifier
        .check_health()
        .context("Notification backend check failed")?;

    let state_path = resolve_state_db_path(&cfg)?;
    let _store = open_state_store(&state_path)?;

    println!(
        "config: {} (source: {})",
        resolved_config.path.display(),
        resolved_config.source
    );
    println!("gh auth: ok");
    println!("notifier: ok");
    println!("state db: {}", state_path.display());
    Ok(())
}
