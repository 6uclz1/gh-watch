use std::{
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::Deserialize;

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
        #[arg(long)]
        interactive: bool,
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
        Commands::Init {
            path,
            force,
            interactive,
        } => {
            let path = path.unwrap_or(installed_config_path()?);
            if interactive {
                run_init_interactive_cmd(path, force).await
            } else {
                run_init_cmd(path, force)
            }
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

#[derive(Debug, Deserialize)]
struct RepoCandidate {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

async fn run_init_interactive_cmd(path: PathBuf, force: bool) -> Result<()> {
    prepare_init_target(&path, force)?;

    let gh = GhCliClient::default();
    gh.check_auth()
        .await
        .context("GitHub authentication is invalid. Run `gh auth login -h github.com`.")?;

    println!("auth: ok");
    let repositories = match fetch_repo_candidates() {
        Ok(candidates) if !candidates.is_empty() => select_repositories(&candidates)?,
        Ok(_) => {
            println!("repo candidates were empty, falling back to manual input");
            prompt_manual_repositories()?
        }
        Err(err) => {
            println!("failed to fetch repo candidates: {err}");
            println!("falling back to manual input");
            prompt_manual_repositories()?
        }
    };

    let interval_seconds = prompt_u64("interval_seconds", 300)?;
    let notifications_enabled = prompt_bool("notifications.enabled", true)?;
    let include_url = prompt_bool("notifications.include_url", true)?;

    println!("config path: {}", path.display());
    let should_write = prompt_bool("write config now", true)?;
    if !should_write {
        return Err(anyhow!("init aborted by user"));
    }

    let config_src = render_config(
        &repositories,
        interval_seconds,
        notifications_enabled,
        include_url,
    );
    parse_config(&config_src).context("generated config is invalid")?;
    fs::write(&path, config_src).with_context(|| format!("failed to write config: {}", path.display()))?;

    println!("created config: {}", path.display());
    println!("next: run `gh-watch check --config {}`", path.display());
    Ok(())
}

fn fetch_repo_candidates() -> Result<Vec<String>> {
    let gh_bin = std::env::var_os("GH_WATCH_GH_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gh"));
    let output = Command::new(&gh_bin)
        .args(["repo", "list", "--limit", "20", "--json", "nameWithOwner"])
        .output()
        .with_context(|| format!("failed to execute `{}`", gh_bin.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "gh repo list failed (status={}): {}",
            output.status,
            stderr.trim()
        ));
    }

    let payload = String::from_utf8(output.stdout).context("gh repo list output was not UTF-8")?;
    let repos: Vec<RepoCandidate> =
        serde_json::from_str(&payload).context("failed to parse repo candidates JSON")?;
    Ok(repos.into_iter().map(|repo| repo.name_with_owner).collect())
}

fn select_repositories(candidates: &[String]) -> Result<Vec<String>> {
    println!("repository candidates:");
    for (index, repo) in candidates.iter().enumerate() {
        println!("  {}. {}", index + 1, repo);
    }

    let raw = prompt_line("select repositories by number (comma-separated), or press Enter for manual input: ")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return prompt_manual_repositories();
    }

    let mut selected = Vec::new();
    for part in trimmed.split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        let index: usize = token
            .parse()
            .with_context(|| format!("invalid repository selection index: {token}"))?;
        let Some(repo) = candidates.get(index.saturating_sub(1)) else {
            return Err(anyhow!(
                "repository selection out of range: {index} (1..={})",
                candidates.len()
            ));
        };
        selected.push(repo.clone());
    }

    selected.sort();
    selected.dedup();
    if selected.is_empty() {
        return Err(anyhow!("at least one repository must be selected"));
    }
    Ok(selected)
}

fn prompt_manual_repositories() -> Result<Vec<String>> {
    let raw = prompt_line("enter repositories (owner/repo, comma-separated): ")?;
    let repos = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if repos.is_empty() {
        return Err(anyhow!("at least one repository is required"));
    }
    Ok(repos)
}

fn prompt_u64(label: &str, default: u64) -> Result<u64> {
    loop {
        let raw = prompt_line(&format!("{label} [{default}]: "))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(default);
        }

        let parsed = trimmed
            .parse::<u64>()
            .with_context(|| format!("expected an integer for {label}"))?;
        if parsed == 0 {
            println!("{label} must be >= 1");
            continue;
        }
        return Ok(parsed);
    }
}

fn prompt_bool(label: &str, default: bool) -> Result<bool> {
    let hint = if default { "Y/n" } else { "y/N" };
    loop {
        let raw = prompt_line(&format!("{label} [{hint}]: "))?;
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("please answer with y or n"),
        }
    }
}

fn prompt_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut raw = String::new();
    io::stdin().read_line(&mut raw)?;
    Ok(raw)
}

fn render_config(
    repositories: &[String],
    interval_seconds: u64,
    notifications_enabled: bool,
    include_url: bool,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("interval_seconds = {interval_seconds}\n"));
    out.push_str("timeline_limit = 500\n");
    out.push_str("retention_days = 90\n");
    out.push_str("failure_history_limit = 200\n");
    out.push('\n');
    out.push_str("[notifications]\n");
    out.push_str(&format!("enabled = {notifications_enabled}\n"));
    out.push_str(&format!("include_url = {include_url}\n"));
    out.push('\n');
    out.push_str("[poll]\n");
    out.push_str("max_concurrency = 4\n");
    out.push_str("timeout_seconds = 30\n");

    for repo in repositories {
        out.push('\n');
        out.push_str("[[repositories]]\n");
        out.push_str(&format!("name = \"{repo}\"\n"));
        out.push_str("enabled = true\n");
    }

    out
}

fn prepare_init_target(path: &Path, force: bool) -> Result<()> {
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

    Ok(())
}

fn run_init_cmd(path: PathBuf, force: bool) -> Result<()> {
    prepare_init_target(&path, force)?;

    fs::write(&path, include_str!("../config.example.toml"))
        .with_context(|| format!("failed to write config: {}", path.display()))?;

    println!("created config: {}", path.display());
    println!("next: edit [[repositories]] in the config file");
    Ok(())
}
