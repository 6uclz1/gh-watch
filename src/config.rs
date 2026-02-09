use std::{
    env,
    fmt::{Display, Formatter},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use directories::BaseDirs;
use serde::Deserialize;

use crate::domain::events::EventKind;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_interval_seconds")]
    pub interval_seconds: u64,
    #[serde(default = "default_bootstrap_lookback_hours")]
    pub bootstrap_lookback_hours: u64,
    #[serde(default = "default_timeline_limit")]
    pub timeline_limit: usize,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_failure_history_limit")]
    pub failure_history_limit: usize,
    pub state_db_path: Option<String>,
    pub repositories: Vec<RepositoryConfig>,
    #[serde(default)]
    pub notifications: NotificationConfig,
    #[serde(default)]
    pub filters: FiltersConfig,
    #[serde(default)]
    pub poll: PollConfig,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub resolved_path: ResolvedConfigPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfigPath {
    pub path: PathBuf,
    pub source: ConfigPathSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPathSource {
    ExplicitArg,
    EnvironmentVariable,
    CurrentDirectory,
    BinaryDirectory,
}

impl ConfigPathSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitArg => "--config",
            Self::EnvironmentVariable => "GH_WATCH_CONFIG",
            Self::CurrentDirectory => "./config.toml",
            Self::BinaryDirectory => "binary-directory",
        }
    }
}

impl Display for ConfigPathSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub event_kinds: Option<Vec<EventKind>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub include_url: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            include_url: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FiltersConfig {
    #[serde(default)]
    pub event_kinds: Vec<EventKind>,
    #[serde(default)]
    pub ignore_actors: Vec<String>,
    #[serde(default)]
    pub only_involving_me: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PollConfig {
    #[serde(default = "default_poll_max_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_poll_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            max_concurrency: default_poll_max_concurrency(),
            timeout_seconds: default_poll_timeout_seconds(),
        }
    }
}

fn default_interval_seconds() -> u64 {
    300
}

fn default_bootstrap_lookback_hours() -> u64 {
    24
}

fn default_timeline_limit() -> usize {
    500
}

fn default_retention_days() -> u32 {
    90
}

fn default_failure_history_limit() -> usize {
    200
}

fn default_true() -> bool {
    true
}

fn default_poll_max_concurrency() -> usize {
    4
}

fn default_poll_timeout_seconds() -> u64 {
    30
}

pub fn parse_config(src: &str) -> Result<Config> {
    let cfg: Config = toml::from_str(src).context("failed to parse config TOML")?;
    validate_config(&cfg)?;
    Ok(cfg)
}

pub fn load_config(path: Option<&Path>) -> Result<Config> {
    let loaded = load_config_with_path(path)?;
    Ok(loaded.config)
}

pub fn load_config_with_path(path: Option<&Path>) -> Result<LoadedConfig> {
    let resolved_path = resolve_config_path_with_source(path)?;
    let src = fs::read_to_string(&resolved_path.path).with_context(|| {
        format!(
            "failed to read config: {} (source: {}, run `gh-watch init` to create it, use `gh-watch config open`, or pass `--config <path>`)",
            resolved_path.path.display(),
            resolved_path.source
        )
    })?;
    let config = parse_config(&src)?;
    Ok(LoadedConfig {
        config,
        resolved_path,
    })
}

pub fn resolve_config_path(path: Option<&Path>) -> Result<PathBuf> {
    Ok(resolve_config_path_with_source(path)?.path)
}

pub fn resolve_config_path_with_source(path: Option<&Path>) -> Result<ResolvedConfigPath> {
    if let Some(explicit) = path {
        return Ok(ResolvedConfigPath {
            path: explicit.to_path_buf(),
            source: ConfigPathSource::ExplicitArg,
        });
    }

    if let Some(from_env) = gh_watch_config_path_from_env() {
        return Ok(ResolvedConfigPath {
            path: from_env,
            source: ConfigPathSource::EnvironmentVariable,
        });
    }

    let cwd_path = current_directory_config_path()?;
    if cwd_path.exists() {
        return Ok(ResolvedConfigPath {
            path: cwd_path,
            source: ConfigPathSource::CurrentDirectory,
        });
    }

    let installed = installed_config_path()?;
    if installed.exists() {
        return Ok(ResolvedConfigPath {
            path: installed,
            source: ConfigPathSource::BinaryDirectory,
        });
    }

    Ok(ResolvedConfigPath {
        path: cwd_path,
        source: ConfigPathSource::CurrentDirectory,
    })
}

pub fn resolution_candidates() -> Result<Vec<ResolvedConfigPath>> {
    let mut candidates = Vec::new();
    if let Some(from_env) = gh_watch_config_path_from_env() {
        candidates.push(ResolvedConfigPath {
            path: from_env,
            source: ConfigPathSource::EnvironmentVariable,
        });
    }
    candidates.push(ResolvedConfigPath {
        path: current_directory_config_path()?,
        source: ConfigPathSource::CurrentDirectory,
    });
    candidates.push(ResolvedConfigPath {
        path: installed_config_path()?,
        source: ConfigPathSource::BinaryDirectory,
    });
    Ok(candidates)
}

pub fn installed_config_path() -> Result<PathBuf> {
    let exe = env::current_exe().context("could not determine current executable path")?;
    let dir = exe.parent().ok_or_else(|| {
        anyhow!(
            "current executable has no parent directory: {}",
            exe.display()
        )
    })?;
    Ok(dir.join("config.toml"))
}

fn gh_watch_config_path_from_env() -> Option<PathBuf> {
    let raw = env::var_os("GH_WATCH_CONFIG")?;
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

fn current_directory_config_path() -> Result<PathBuf> {
    let cwd = env::current_dir().context("could not determine current working directory")?;
    Ok(cwd.join("config.toml"))
}

pub fn default_state_db_path() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let local_appdata =
            env::var_os("LOCALAPPDATA").ok_or_else(|| anyhow!("LOCALAPPDATA is not set"))?;
        return Ok(PathBuf::from(local_appdata)
            .join("gh-watch")
            .join("state.db"));
    }

    #[cfg(not(windows))]
    {
        let home = home_dir()?;
        Ok(home
            .join(".local")
            .join("share")
            .join("gh-watch")
            .join("state.db"))
    }
}

fn home_dir() -> Result<PathBuf> {
    BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| anyhow!("could not determine home directory"))
}

fn validate_config(cfg: &Config) -> Result<()> {
    if cfg.repositories.is_empty() {
        return Err(anyhow!("repositories must contain at least one entry"));
    }

    for repo in &cfg.repositories {
        validate_repo_name(&repo.name)?;
    }

    if cfg.interval_seconds == 0 {
        return Err(anyhow!("interval_seconds must be >= 1"));
    }

    if cfg.timeline_limit == 0 {
        return Err(anyhow!("timeline_limit must be >= 1"));
    }

    if cfg.failure_history_limit == 0 {
        return Err(anyhow!("failure_history_limit must be >= 1"));
    }

    if cfg.poll.max_concurrency == 0 {
        return Err(anyhow!("poll.max_concurrency must be >= 1"));
    }

    if cfg.poll.timeout_seconds == 0 {
        return Err(anyhow!("poll.timeout_seconds must be >= 1"));
    }

    Ok(())
}

fn validate_repo_name(repo: &str) -> Result<()> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("");
    let extra = parts.next();

    let valid = !owner.is_empty() && !name.is_empty() && extra.is_none();
    if valid {
        Ok(())
    } else {
        Err(anyhow!(
            "repository '{}' is invalid; expected owner/repo format",
            repo
        ))
    }
}
