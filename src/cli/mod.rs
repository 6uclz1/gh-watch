mod args;
mod commands;
mod state;

use anyhow::Result;
use chrono::Utc;
use clap::Parser;

use crate::{config::load_config_with_path, ports::ClockPort};

use args::{Cli, Commands};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SystemClock;

impl ClockPort for SystemClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc::now()
    }
}

pub fn exit_code_for_error(_err: &anyhow::Error) -> i32 {
    1
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Watch {
            config,
            interval_seconds,
        } => {
            let loaded = load_config_with_path(config.as_deref())?;
            let mut cfg = loaded.config;
            if let Some(interval) = interval_seconds {
                cfg.interval_seconds = interval;
            }
            commands::watch::run(cfg, loaded.resolved_path).await
        }
        Commands::Check { config } => {
            let loaded = load_config_with_path(config.as_deref())?;
            commands::check::run(loaded.config, loaded.resolved_path).await
        }
        Commands::Once {
            config,
            dry_run,
            json,
        } => {
            let loaded = load_config_with_path(config.as_deref())?;
            commands::once::run(loaded.config, loaded.resolved_path, dry_run, json).await
        }
        Commands::Init {
            path,
            force,
            reset_state,
        } => {
            if reset_state {
                commands::init::run_reset_state(path)
            } else {
                commands::init::run(path, force)
            }
        }
        Commands::Config { command } => commands::config::run(command),
    }
}
