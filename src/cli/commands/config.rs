use std::{ffi::OsStr, path::Path, process::Command};

use anyhow::{anyhow, Result};

use crate::{cli::args::ConfigCommands, config::resolve_config_path_with_source};

pub(crate) fn run(command: ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::Open => run_open_cmd(),
        ConfigCommands::Path => run_path_cmd(),
    }
}

fn run_open_cmd() -> Result<()> {
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

fn run_path_cmd() -> Result<()> {
    let resolved = resolve_config_path_with_source(None)?;
    println!("{} (source: {})", resolved.path.display(), resolved.source);
    Ok(())
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
