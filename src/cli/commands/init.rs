use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};

use crate::{
    cli::state::{open_state_store, remove_state_db_files, resolve_state_db_path_for_reset},
    config::installed_config_path,
};

pub(crate) fn run(path: Option<PathBuf>, force: bool) -> Result<()> {
    let path = path.unwrap_or(installed_config_path()?);
    prepare_init_target(&path, force)?;

    fs::write(&path, include_str!("../../../config.example.toml"))
        .with_context(|| format!("failed to write config: {}", path.display()))?;

    println!("created config: {}", path.display());
    println!("next: edit [[repositories]] in the config file");
    Ok(())
}

pub(crate) fn run_reset_state(config_path: Option<PathBuf>) -> Result<()> {
    let state_db_path = resolve_state_db_path_for_reset(config_path.as_deref())?;
    remove_state_db_files(&state_db_path)?;
    let _store = open_state_store(&state_db_path)?;

    println!("reset state db: {}", state_db_path.display());
    println!("state db initialized");
    Ok(())
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
