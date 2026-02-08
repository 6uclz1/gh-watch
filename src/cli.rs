use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};

use crate::{
    app::watch_loop::run_watch,
    config::{
        default_state_db_path, installed_config_path, load_config_with_path, parse_config,
        resolution_candidates, resolve_config_path_with_source, Config, ResolvedConfigPath,
    },
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
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommands {
    #[command(alias = "edit")]
    Open,
    Path,
    Doctor,
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
            let loaded = load_config_with_path(config.as_deref())?;
            let mut cfg = loaded.config;
            if let Some(interval) = interval_seconds {
                cfg.interval_seconds = interval;
            }
            run_watch_cmd(cfg, loaded.resolved_path).await
        }
        Commands::Check { config } => {
            let loaded = load_config_with_path(config.as_deref())?;
            run_check_cmd(loaded.config, loaded.resolved_path).await
        }
        Commands::Init { path, force } => {
            let path = path.unwrap_or(installed_config_path()?);
            run_init_cmd(path, force)
        }
        Commands::Config { command } => run_config_cmd(command),
    }
}

async fn run_check_cmd(cfg: Config, resolved_config: ResolvedConfigPath) -> Result<()> {
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

async fn run_watch_cmd(cfg: Config, resolved_config: ResolvedConfigPath) -> Result<()> {
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

fn run_config_cmd(command: ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::Open => run_config_open_cmd(),
        ConfigCommands::Path => run_config_path_cmd(),
        ConfigCommands::Doctor => run_config_doctor_cmd(),
    }
}

fn run_config_open_cmd() -> Result<()> {
    let resolved = resolve_config_path_with_source(None)?;
    if !resolved.path.exists() {
        return Err(anyhow!(
            "config does not exist: {} (source: {}, run `gh-watch init`, `gh-watch config open`, or pass `--config <path>`)",
            resolved.path.display(),
            resolved.source
        ));
    }

    open_config_file(&resolved.path)
}

fn run_config_path_cmd() -> Result<()> {
    let resolved = resolve_config_path_with_source(None)?;
    println!(
        "{} (source: {})",
        resolved.path.display(),
        resolved.source
    );
    Ok(())
}

fn run_config_doctor_cmd() -> Result<()> {
    let selected = resolve_config_path_with_source(None)?;
    println!(
        "selected: {} (source: {})",
        selected.path.display(),
        selected.source
    );
    println!("candidates:");
    println!("- --config: not provided");

    if let Some(raw) = std::env::var_os("GH_WATCH_CONFIG") {
        println!("- GH_WATCH_CONFIG: {}", PathBuf::from(raw).display());
    } else {
        println!("- GH_WATCH_CONFIG: not set");
    }

    for candidate in resolution_candidates()? {
        println!(
            "- {}: {} | {}",
            candidate.source,
            candidate.path.display(),
            describe_config_candidate(&candidate.path)
        );
    }

    if !selected.path.exists() {
        println!(
            "next: run `gh-watch init` to create {}, or pass `--config <path>`",
            selected.path.display()
        );
        return Ok(());
    }

    let config_src = fs::read_to_string(&selected.path)
        .with_context(|| format!("failed to read selected config: {}", selected.path.display()))?;
    match parse_config(&config_src) {
        Ok(_) => println!("doctor: selected config parses successfully"),
        Err(err) => {
            println!("doctor: selected config has errors");
            println!("next: fix config TOML or re-run `gh-watch init --force`");
            println!("error: {err}");
        }
    }

    Ok(())
}

fn describe_config_candidate(path: &Path) -> String {
    if !path.exists() {
        return "missing".to_string();
    }

    let src = match fs::read_to_string(path) {
        Ok(src) => src,
        Err(err) => return format!("read_error: {err}"),
    };

    match parse_config(&src) {
        Ok(_) => "exists, parse=ok".to_string(),
        Err(err) => format!("exists, parse=error ({err})"),
    }
}

fn open_config_file(path: &Path) -> Result<()> {
    if let Some(raw) = std::env::var_os("VISUAL") {
        if try_editor_command(&raw, path)? {
            return Ok(());
        }
    }

    if let Some(raw) = std::env::var_os("EDITOR") {
        if try_editor_command(&raw, path)? {
            return Ok(());
        }
    }

    if try_os_default_opener(path)? {
        return Ok(());
    }

    Err(anyhow!(
        "failed to open config: {} (tried VISUAL, EDITOR, and OS default opener)",
        path.display()
    ))
}

fn try_editor_command(raw: &OsStr, path: &Path) -> Result<bool> {
    let raw = raw.to_string_lossy();
    let mut tokens = raw.split_whitespace();
    let Some(bin) = tokens.next() else {
        return Ok(false);
    };

    let mut cmd = Command::new(bin);
    cmd.args(tokens);
    cmd.arg(path);
    let ok = cmd.status().map(|status| status.success()).unwrap_or(false);
    Ok(ok)
}

fn try_os_default_opener(path: &Path) -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let ok = Command::new("open")
            .arg(path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        return Ok(ok);
    }

    #[cfg(target_os = "linux")]
    {
        let ok = Command::new("xdg-open")
            .arg(path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        return Ok(ok);
    }

    #[cfg(target_os = "windows")]
    {
        let ok = Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        return Ok(ok);
    }

    #[allow(unreachable_code)]
    Ok(false)
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
