use std::{
    collections::{BTreeMap, HashSet},
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::{
    app::{poll_once::poll_once, watch_loop::run_watch},
    config::{
        default_state_db_path, installed_config_path, load_config_with_path, parse_config,
        resolution_candidates, resolve_config_path_with_source, Config, ResolvedConfigPath,
    },
    infra::{gh_client::GhCliClient, notifier::DesktopNotifier, state_sqlite::SqliteStateStore},
    ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort},
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
    Once {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    Report {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, default_value = "24h")]
        since: String,
        #[arg(long, value_enum, default_value_t = ReportFormatArg::Markdown)]
        format: ReportFormatArg,
    },
    Doctor {
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
        #[arg(long)]
        reset_state: bool,
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

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum ReportFormatArg {
    Markdown,
    Json,
}

#[derive(Debug, Clone, Copy)]
struct SystemClock;

impl ClockPort for SystemClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug)]
pub struct OncePartialFailure;

impl std::fmt::Display for OncePartialFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("one or more repositories failed during once run")
    }
}

impl std::error::Error for OncePartialFailure {}

pub fn exit_code_for_error(err: &anyhow::Error) -> i32 {
    if err.downcast_ref::<OncePartialFailure>().is_some() {
        2
    } else {
        1
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
        Commands::Once {
            config,
            dry_run,
            json,
        } => {
            let loaded = load_config_with_path(config.as_deref())?;
            run_once_cmd(loaded.config, loaded.resolved_path, dry_run, json).await
        }
        Commands::Report {
            config,
            since,
            format,
        } => {
            let loaded = load_config_with_path(config.as_deref())?;
            run_report_cmd(loaded.config, loaded.resolved_path, &since, format).await
        }
        Commands::Doctor { config } => {
            let loaded = load_config_with_path(config.as_deref())?;
            run_doctor_cmd(loaded.config, loaded.resolved_path).await
        }
        Commands::Init {
            path,
            force,
            interactive,
            reset_state,
        } => {
            if reset_state {
                run_init_reset_state_cmd(path, interactive)
            } else {
                let path = path.unwrap_or(installed_config_path()?);
                if interactive {
                    run_init_interactive_cmd(path, force).await
                } else {
                    run_init_cmd(path, force)
                }
            }
        }
        Commands::Config { command } => run_config_cmd(command),
    }
}

fn run_init_reset_state_cmd(config_path: Option<PathBuf>, interactive: bool) -> Result<()> {
    if interactive {
        return Err(anyhow!("--reset-state cannot be used with --interactive"));
    }

    let state_db_path = resolve_state_db_path_for_reset(config_path.as_deref())?;
    remove_state_db_files(&state_db_path)?;
    let _store = SqliteStateStore::new(&state_db_path)?;

    println!("reset state db: {}", state_db_path.display());
    println!("state db initialized");
    Ok(())
}

fn resolve_state_db_path_for_reset(config_path: Option<&Path>) -> Result<PathBuf> {
    let resolved = resolve_config_path_with_source(config_path)?;
    if !resolved.path.exists() {
        return default_state_db_path();
    }

    let src = fs::read_to_string(&resolved.path).with_context(|| {
        format!(
            "failed to read config for --reset-state: {}",
            resolved.path.display()
        )
    })?;
    let cfg = parse_config(&src).with_context(|| {
        format!(
            "failed to parse config for --reset-state: {}",
            resolved.path.display()
        )
    })?;

    resolve_state_db_path(&cfg)
}

fn remove_state_db_files(path: &Path) -> Result<()> {
    for candidate in [
        path.to_path_buf(),
        state_db_sidecar_path(path, "-wal"),
        state_db_sidecar_path(path, "-shm"),
    ] {
        if !candidate.exists() {
            continue;
        }

        fs::remove_file(&candidate)
            .with_context(|| format!("failed to remove state db file: {}", candidate.display()))?;
    }

    Ok(())
}

fn state_db_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut raw: OsString = path.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

async fn run_check_cmd(cfg: Config, resolved_config: ResolvedConfigPath) -> Result<()> {
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

    let notifier = DesktopNotifier::from_notification_config(&cfg.notifications);
    for warning in notifier.startup_warnings() {
        eprintln!("notification backend warning: {warning}");
    }
    if let Err(err) = notifier.check_health() {
        eprintln!("notification backend warning: {err}");
    }

    run_watch(&cfg, &gh, &state, &notifier, &SystemClock).await
}

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

    fn is_event_notified(&self, event_key: &str) -> Result<bool> {
        self.inner.is_event_notified(event_key)
    }

    fn record_notified_event(
        &self,
        _event: &crate::domain::events::WatchEvent,
        _notified_at: DateTime<Utc>,
    ) -> Result<()> {
        Ok(())
    }

    fn record_failure(&self, _failure: &crate::domain::failure::FailureRecord) -> Result<()> {
        Ok(())
    }

    fn latest_failure(&self) -> Result<Option<crate::domain::failure::FailureRecord>> {
        self.inner.latest_failure()
    }

    fn append_timeline_event(&self, _event: &crate::domain::events::WatchEvent) -> Result<()> {
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

    fn cleanup_old(
        &self,
        _retention_days: u32,
        _failure_history_limit: usize,
        _now: DateTime<Utc>,
    ) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct ReportOutput {
    generated_at: DateTime<Utc>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    events_total: usize,
    failures_total: usize,
    events_by_kind: BTreeMap<String, usize>,
    events_by_repo: BTreeMap<String, usize>,
    recent_failures: Vec<crate::domain::failure::FailureRecord>,
}

async fn run_once_cmd(
    cfg: Config,
    resolved_config: ResolvedConfigPath,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let gh = GhCliClient::default();
    gh.check_auth()
        .await
        .context("GitHub authentication is invalid. Run `gh auth login -h github.com`.")?;

    let state_path = resolve_state_db_path(&cfg)?;
    let state = SqliteStateStore::new(&state_path)?;

    let notifier = DesktopNotifier::from_notification_config(&cfg.notifications);
    if let Err(err) = notifier.check_health() {
        eprintln!("notification backend warning: {err}");
    }

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
        println!("repo_errors: {}", outcome.repo_errors.len());
        if dry_run {
            println!("mode: dry-run (state unchanged)");
        }
        for repo_error in &outcome.repo_errors {
            println!("- {repo_error}");
        }
    }

    if outcome.repo_errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow::Error::new(OncePartialFailure))
    }
}

async fn run_report_cmd(
    cfg: Config,
    _resolved_config: ResolvedConfigPath,
    since_raw: &str,
    format: ReportFormatArg,
) -> Result<()> {
    let window = parse_since_duration(since_raw)?;
    let now = Utc::now();
    let start = now
        .checked_sub_signed(window)
        .ok_or_else(|| anyhow!("failed to resolve report time window"))?;

    let state_path = resolve_state_db_path(&cfg)?;
    let state = SqliteStateStore::new(&state_path)?;
    let events = state.load_timeline_events_since(start, 5000)?;
    let failures = state.load_failures_since(start, 5000)?;

    let mut events_by_kind = BTreeMap::new();
    let mut events_by_repo = BTreeMap::new();
    for event in &events {
        *events_by_kind
            .entry(event.kind.as_str().to_string())
            .or_insert(0) += 1;
        *events_by_repo.entry(event.repo.clone()).or_insert(0) += 1;
    }

    let report = ReportOutput {
        generated_at: now,
        window_start: start,
        window_end: now,
        events_total: events.len(),
        failures_total: failures.len(),
        events_by_kind,
        events_by_repo,
        recent_failures: failures.into_iter().take(10).collect(),
    };

    match format {
        ReportFormatArg::Json => println!("{}", serde_json::to_string(&report)?),
        ReportFormatArg::Markdown => print_markdown_report(&report),
    }

    Ok(())
}

async fn run_doctor_cmd(cfg: Config, resolved_config: ResolvedConfigPath) -> Result<()> {
    println!(
        "config: {} (source: {})",
        resolved_config.path.display(),
        resolved_config.source
    );
    println!("config doctor: ok");

    let gh = GhCliClient::default();
    gh.check_auth()
        .await
        .context("GitHub authentication is invalid. Run `gh auth login -h github.com`.")?;
    println!("gh auth: ok");

    let notifier = DesktopNotifier::from_notification_config(&cfg.notifications);
    match notifier.check_health() {
        Ok(()) => println!("notifier: ok"),
        Err(err) => println!("notifier: warning ({err})"),
    }

    let state_path = resolve_state_db_path(&cfg)?;
    let _store = SqliteStateStore::new(&state_path)?;
    println!("state db: {}", state_path.display());

    Ok(())
}

fn parse_since_duration(raw: &str) -> Result<Duration> {
    if raw.len() < 2 {
        return Err(anyhow!(
            "invalid --since value '{raw}'; expected '<number><s|m|h|d>'"
        ));
    }

    let (value, unit) = raw.split_at(raw.len() - 1);
    let amount: i64 = value
        .parse()
        .with_context(|| format!("invalid --since amount in '{raw}'"))?;
    if amount <= 0 {
        return Err(anyhow!("--since must be > 0"));
    }

    let duration = match unit {
        "s" => Duration::seconds(amount),
        "m" => Duration::minutes(amount),
        "h" => Duration::hours(amount),
        "d" => Duration::days(amount),
        _ => {
            return Err(anyhow!(
                "invalid --since unit in '{raw}'; expected one of s,m,h,d"
            ));
        }
    };

    Ok(duration)
}

fn print_markdown_report(report: &ReportOutput) {
    println!("# gh-watch report");
    println!("window_start: {}", report.window_start.to_rfc3339());
    println!("window_end: {}", report.window_end.to_rfc3339());
    println!("events_total: {}", report.events_total);
    println!("failures_total: {}", report.failures_total);
    println!();
    println!("## events_by_kind");
    if report.events_by_kind.is_empty() {
        println!("- (none)");
    } else {
        for (kind, count) in &report.events_by_kind {
            println!("- {kind}: {count}");
        }
    }
    println!();
    println!("## events_by_repo");
    if report.events_by_repo.is_empty() {
        println!("- (none)");
    } else {
        for (repo, count) in &report.events_by_repo {
            println!("- {repo}: {count}");
        }
    }
    println!();
    println!("## recent_failures");
    if report.recent_failures.is_empty() {
        println!("- (none)");
    } else {
        for failure in &report.recent_failures {
            println!(
                "- {} [{}:{}] {}",
                failure.failed_at.to_rfc3339(),
                failure.kind,
                failure.repo,
                failure.message
            );
        }
    }
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
    println!("{} (source: {})", resolved.path.display(), resolved.source);
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

    let config_src = fs::read_to_string(&selected.path).with_context(|| {
        format!(
            "failed to read selected config: {}",
            selected.path.display()
        )
    })?;
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

    println!("auth: ok / 認証OK");
    let repositories = match fetch_repo_candidates() {
        Ok(candidates) if !candidates.is_empty() => select_repositories(&candidates)?,
        Ok(_) => {
            println!("repo candidates were empty, falling back to manual input / 候補が空のため手入力に切り替えます");
            prompt_manual_repositories()?
        }
        Err(err) => {
            println!("failed to fetch repo candidates: {err}");
            println!("falling back to manual input / 手入力に切り替えます");
            prompt_manual_repositories()?
        }
    };

    let interval_seconds = prompt_u64("interval_seconds", 300)?;
    let notifications_enabled = prompt_bool("notifications.enabled", true)?;
    let include_url = prompt_bool("notifications.include_url", true)?;

    println!("config path: {} / 設定ファイルパス", path.display());
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
    fs::write(&path, config_src)
        .with_context(|| format!("failed to write config: {}", path.display()))?;

    println!(
        "created config: {} / 設定ファイルを作成しました",
        path.display()
    );
    println!(
        "next: run `gh-watch check --config {}` / 次に動作確認してください",
        path.display()
    );
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

    let raw = prompt_line(
        "select repositories by number (comma-separated), or press Enter for manual input: ",
    )?;
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
    let raw = prompt_line(
        "enter repositories (owner/repo, comma-separated) / 監視対象を入力 (owner/repo, カンマ区切り): ",
    )?;
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
    out.push_str("bootstrap_lookback_hours = 24\n");
    out.push_str("timeline_limit = 500\n");
    out.push_str("retention_days = 90\n");
    out.push_str("failure_history_limit = 200\n");
    out.push('\n');
    out.push_str("[notifications]\n");
    out.push_str(&format!("enabled = {notifications_enabled}\n"));
    out.push_str(&format!("include_url = {include_url}\n"));
    out.push_str("# macos_bundle_id = \"com.apple.Terminal\"\n");
    out.push_str("# windows_app_id = \"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\\\WindowsPowerShell\\\\v1.0\\\\powershell.exe\"\n");
    out.push_str("# wsl_windows_app_id = \"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\\\WindowsPowerShell\\\\v1.0\\\\powershell.exe\"\n");
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
