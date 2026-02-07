use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use directories::BaseDirs;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_interval_seconds")]
    pub interval_seconds: u64,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
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

fn default_interval_seconds() -> u64 {
    300
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

pub fn parse_config(src: &str) -> Result<Config> {
    let cfg: Config = toml::from_str(src).context("failed to parse config TOML")?;
    validate_config(&cfg)?;
    Ok(cfg)
}

pub fn load_config(path: Option<&Path>) -> Result<Config> {
    let config_path = resolve_config_path(path)?;

    let src = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {}", config_path.display()))?;
    parse_config(&src)
}

pub fn resolve_config_path(path: Option<&Path>) -> Result<PathBuf> {
    if let Some(explicit) = path {
        return Ok(explicit.to_path_buf());
    }

    let local = PathBuf::from("config.toml");
    if local.exists() {
        return Ok(local);
    }

    if let Some(raw) = env::var_os("GH_WATCH_CONFIG") {
        return Ok(PathBuf::from(raw));
    }

    default_config_path()
}

pub fn default_config_path() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let appdata = env::var_os("APPDATA").ok_or_else(|| anyhow!("APPDATA is not set"))?;
        return Ok(PathBuf::from(appdata).join("gh-watch").join("config.toml"));
    }

    #[cfg(not(windows))]
    {
        let home = home_dir()?;
        Ok(home.join(".config").join("gh-watch").join("config.toml"))
    }
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
