use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::{
    config::{default_state_db_path, parse_config, resolve_config_path_with_source, Config},
    infra::state_sqlite::{SqliteStateStore, StateSchemaMismatchError},
};

pub(crate) fn resolve_state_db_path(cfg: &Config) -> Result<PathBuf> {
    match &cfg.state_db_path {
        Some(raw) => Ok(PathBuf::from(raw)),
        None => default_state_db_path(),
    }
}

pub(crate) fn resolve_state_db_path_for_reset(config_path: Option<&Path>) -> Result<PathBuf> {
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
    let state_db_path = parse_state_db_path_for_reset(&src).with_context(|| {
        format!(
            "failed to parse config for --reset-state: {}",
            resolved.path.display()
        )
    })?;

    match state_db_path {
        Some(path) => Ok(path),
        None => default_state_db_path(),
    }
}

#[derive(Debug, Deserialize)]
struct ResetStateConfigCompat {
    state_db_path: Option<String>,
}

fn parse_state_db_path_for_reset(src: &str) -> Result<Option<PathBuf>> {
    match parse_config(src) {
        Ok(cfg) => Ok(cfg.state_db_path.map(PathBuf::from)),
        Err(err) if is_unknown_field_error(&err) => {
            let cfg: ResetStateConfigCompat =
                toml::from_str(src).context("failed to parse config TOML")?;
            Ok(cfg.state_db_path.map(PathBuf::from))
        }
        Err(err) => Err(err),
    }
}

fn is_unknown_field_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let msg = cause.to_string();
        msg.contains("unknown field `")
    })
}

pub(crate) fn remove_state_db_files(path: &Path) -> Result<()> {
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

pub(crate) fn open_state_store(path: &Path) -> Result<SqliteStateStore> {
    SqliteStateStore::new(path).map_err(|err| {
        if err.downcast_ref::<StateSchemaMismatchError>().is_some() {
            anyhow!(
                "state db schema is incompatible: {} (run `gh-watch init --reset-state`)",
                path.display()
            )
        } else {
            err
        }
    })
}
