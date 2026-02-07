use std::{fs, path::PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};

use crate::{
    app::watch_loop::run_watch,
    config::{default_state_db_path, load_config, Config},
    infra::{gh_client::GhCliClient, notifier::DesktopNotifier, state_sqlite::SqliteStateStore},
    ports::{ClockPort, GhClientPort, NotifierPort},
};

#[derive(Debug, Parser)]
#[command(
    name = "gh-watch",
    about = "Watch GitHub PRs/issues and notify on updates"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Watch {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        interval_seconds: Option<u64>,
    },
    Check {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Init {
        #[arg(long, default_value = "config.toml")]
        path: PathBuf,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Clone, Copy)]
struct SystemClock;

impl ClockPort for SystemClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc::now()
    }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Watch {
            config,
            interval_seconds,
        } => {
            let mut cfg = load_config(config.as_deref())?;
            if let Some(interval) = interval_seconds {
                cfg.interval_seconds = interval;
            }
            run_watch_cmd(cfg).await
        }
        Commands::Check { config } => run_check_cmd(load_config(config.as_deref())?).await,
        Commands::Init { path, force } => run_init_cmd(path, force),
    }
}

async fn run_check_cmd(cfg: Config) -> Result<()> {
    let gh = GhCliClient::default();
    gh.check_auth()
        .await
        .context("GitHub authentication is invalid. Run `gh auth login -h github.com`.")?;

    let notifier = DesktopNotifier;
    notifier
        .check_health()
        .context("Notification backend check failed")?;

    let state_path = resolve_state_db_path(&cfg)?;
    let _store = SqliteStateStore::new(&state_path)?;

    println!("config: ok");
    println!("gh auth: ok");
    println!("notifier: ok");
    println!("state db: {}", state_path.display());
    Ok(())
}

async fn run_watch_cmd(cfg: Config) -> Result<()> {
    let gh = GhCliClient::default();
    gh.check_auth()
        .await
        .context("GitHub authentication is invalid. Run `gh auth login -h github.com`.")?;

    let state_path = resolve_state_db_path(&cfg)?;
    let state = SqliteStateStore::new(&state_path)?;

    let notifier = DesktopNotifier;
    if let Err(err) = notifier.check_health() {
        eprintln!("notification backend warning: {err}");
    }

    run_watch(&cfg, &gh, &state, &notifier, &SystemClock).await
}

fn resolve_state_db_path(cfg: &Config) -> Result<PathBuf> {
    match &cfg.state_db_path {
        Some(raw) => Ok(PathBuf::from(raw)),
        None => default_state_db_path(),
    }
}

fn run_init_cmd(path: PathBuf, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Err(anyhow!(
            "config already exists: {} (use --force to overwrite)",
            path.display()
        ));
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory: {}", parent.display())
            })?;
        }
    }

    fs::write(&path, include_str!("../config.example.toml"))
        .with_context(|| format!("failed to write config: {}", path.display()))?;

    println!("created config: {}", path.display());
    println!("next: edit [[repositories]] in the config file");
    Ok(())
}
