use std::process::Command;

use anyhow::{anyhow, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenCommandResult {
    success: bool,
    stderr: String,
}

fn run_open_command(command: &mut Command) -> OpenCommandResult {
    match command.output() {
        Ok(output) => OpenCommandResult {
            success: output.status.success(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        },
        Err(err) => OpenCommandResult {
            success: false,
            stderr: err.to_string(),
        },
    }
}

pub(super) fn open_url_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg(url);
        let result = run_open_command(&mut cmd);
        if result.success {
            return Ok(());
        }
        if !result.stderr.is_empty() {
            tracing::debug!(url = %url, stderr = %result.stderr, "open command failed");
        }

        return Err(anyhow!("failed to open URL with open: {url}"));
    }

    #[cfg(target_os = "linux")]
    {
        let browser_env = std::env::var("BROWSER").ok();
        return open_url_on_linux(
            url,
            detect_wsl(),
            browser_env.as_deref(),
            run_linux_open_backend,
        );
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", ""]).arg(url);
        let result = run_open_command(&mut cmd);
        if result.success {
            return Ok(());
        }
        if !result.stderr.is_empty() {
            tracing::debug!(url = %url, stderr = %result.stderr, "start command failed");
        }

        return Err(anyhow!("failed to open URL with start: {url}"));
    }

    #[allow(unreachable_code)]
    Err(anyhow!("unsupported OS for opening URLs"))
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxOpenBackend {
    BrowserEnv,
    XdgOpen,
}

#[cfg(target_os = "linux")]
fn open_url_on_linux<F>(
    url: &str,
    is_wsl: bool,
    browser_env: Option<&str>,
    mut runner: F,
) -> Result<()>
where
    F: FnMut(LinuxOpenBackend, &str, Option<&str>) -> bool,
{
    let browser_env = browser_env.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    if is_wsl {
        if let Some(browser_env) = browser_env {
            if runner(LinuxOpenBackend::BrowserEnv, url, Some(browser_env)) {
                return Ok(());
            }
            if runner(LinuxOpenBackend::XdgOpen, url, None) {
                return Ok(());
            }
            return Err(anyhow!(
                "failed to open URL in WSL with $BROWSER and xdg-open: {url}"
            ));
        }

        if runner(LinuxOpenBackend::XdgOpen, url, None) {
            return Ok(());
        }
        return Err(anyhow!("failed to open URL in WSL with xdg-open: {url}"));
    }

    if runner(LinuxOpenBackend::XdgOpen, url, None) {
        return Ok(());
    }

    Err(anyhow!("failed to open URL with xdg-open: {url}"))
}

#[cfg(target_os = "linux")]
fn run_linux_open_backend(backend: LinuxOpenBackend, url: &str, browser_env: Option<&str>) -> bool {
    match backend {
        LinuxOpenBackend::BrowserEnv => browser_env
            .and_then(|raw| browser_command_from_env(raw, url))
            .map(|(bin, args)| {
                let mut cmd = Command::new(&bin);
                cmd.args(args);
                let result = run_open_command(&mut cmd);
                if !result.success && !result.stderr.is_empty() {
                    tracing::debug!(
                        command = %bin,
                        stderr = %result.stderr,
                        "linux browser open command failed"
                    );
                }
                result.success
            })
            .unwrap_or(false),
        LinuxOpenBackend::XdgOpen => {
            let mut cmd = Command::new("xdg-open");
            cmd.arg(url);
            let result = run_open_command(&mut cmd);
            if !result.success && !result.stderr.is_empty() {
                tracing::debug!(stderr = %result.stderr, "linux xdg-open command failed");
            }
            result.success
        }
    }
}

#[cfg(target_os = "linux")]
fn browser_command_from_env(raw: &str, url: &str) -> Option<(String, Vec<String>)> {
    let mut tokens = split_shell_words(raw)?;
    let has_placeholder = tokens.iter().any(|token| token.contains("%s"));

    for token in &mut tokens {
        if token.contains("%s") {
            *token = token.replace("%s", url);
        }
    }

    if !has_placeholder {
        tokens.push(url.to_string());
    }

    let bin = tokens.remove(0);
    Some((bin, tokens))
}

#[cfg(target_os = "linux")]
fn split_shell_words(raw: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut active_quote: Option<char> = None;
    let mut escaped = false;

    for ch in raw.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if let Some(quote) = active_quote {
            if ch == quote {
                active_quote = None;
            } else if quote == '"' && ch == '\\' {
                escaped = true;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' | '"' => active_quote = Some(ch),
            '\\' => escaped = true,
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if escaped || active_quote.is_some() {
        return None;
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    if tokens.is_empty() {
        return None;
    }

    Some(tokens)
}

#[cfg(target_os = "linux")]
fn detect_wsl() -> bool {
    let distro_name = std::env::var("WSL_DISTRO_NAME").ok();
    let interop = std::env::var("WSL_INTEROP").ok();
    let proc_hint = read_proc_wsl_hint();
    is_wsl_from_inputs(
        distro_name.as_deref(),
        interop.as_deref(),
        proc_hint.as_deref(),
    )
}

#[cfg(target_os = "linux")]
fn is_wsl_from_inputs(
    wsl_distro_name: Option<&str>,
    wsl_interop: Option<&str>,
    proc_hint: Option<&str>,
) -> bool {
    if wsl_distro_name.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    if wsl_interop.is_some_and(|value| !value.trim().is_empty()) {
        return true;
    }
    proc_hint
        .map(|value| value.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn read_proc_wsl_hint() -> Option<String> {
    let version = std::fs::read_to_string("/proc/version").ok();
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();
    match (version, osrelease) {
        (Some(version), Some(osrelease)) => Some(format!("{version}\n{osrelease}")),
        (Some(version), None) => Some(version),
        (None, Some(osrelease)) => Some(osrelease),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn open_command_capture_collects_stderr_for_failed_process() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "printf 'launcher missing\\n' >&2; exit 1"]);

        let result = super::run_open_command(&mut cmd);
        assert!(!result.success);
        assert_eq!(result.stderr, "launcher missing");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn browser_command_from_env_replaces_percent_s_placeholder() {
        let url = "https://example.com/placeholder";
        let (bin, args) = super::browser_command_from_env("w3m %s --title=%s", url)
            .expect("browser command should parse");

        assert_eq!(bin, "w3m");
        assert_eq!(
            args,
            vec![
                "https://example.com/placeholder".to_string(),
                "--title=https://example.com/placeholder".to_string()
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_open_wsl_failure_after_browser_and_xdg_open_returns_expected_error() {
        let url = "https://example.com/wsl-fail";
        let mut calls = Vec::new();

        let err = super::open_url_on_linux(url, true, Some("firefox"), |backend, _url, _| {
            calls.push(backend);
            false
        })
        .expect_err("wsl browser and xdg-open failures should bubble up");

        assert_eq!(
            calls,
            vec![
                super::LinuxOpenBackend::BrowserEnv,
                super::LinuxOpenBackend::XdgOpen
            ]
        );
        assert_eq!(
            err.to_string(),
            format!("failed to open URL in WSL with $BROWSER and xdg-open: {url}")
        );
    }
}
