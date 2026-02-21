#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
use anyhow::{Context, Result};

#[cfg(target_os = "macos")]
use super::process_error::render_process_failure;

#[cfg(target_os = "macos")]
pub(super) fn check_osascript_available() -> Result<()> {
    let output = Command::new("osascript")
        .args(["-e", "return \"ok\""])
        .output()
        .context("failed to execute osascript")?;

    if output.status.success() {
        return Ok(());
    }

    Err(render_process_failure(
        "osascript",
        "health-check",
        &output.stdout,
        &output.stderr,
        output.status,
    ))
}

#[cfg(target_os = "macos")]
pub(super) fn notify_via_osascript(title: &str, body: &str) -> Result<()> {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_apple_script_literal(body),
        escape_apple_script_literal(title)
    );

    let output = Command::new("osascript")
        .args(["-e", script.as_str()])
        .output()
        .context("failed to execute osascript")?;

    if output.status.success() {
        return Ok(());
    }

    Err(render_process_failure(
        "osascript",
        "notify",
        &output.stdout,
        &output.stderr,
        output.status,
    ))
}

#[cfg(target_os = "macos")]
pub(super) fn escape_apple_script_literal(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::escape_apple_script_literal;

    #[test]
    fn escape_apple_script_literal_escapes_quotes_and_newlines() {
        let escaped = escape_apple_script_literal("a\n\"b\"");
        assert_eq!(escaped, "a\\n\\\"b\\\"");
    }
}
