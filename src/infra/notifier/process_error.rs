#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::ExitStatus;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::anyhow;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn render_process_failure(
    program: &str,
    operation: &str,
    stdout: &[u8],
    stderr: &[u8],
    status: ExitStatus,
) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };

    if detail.is_empty() {
        anyhow!("{program} {operation} failed with status {status}")
    } else {
        anyhow!("{program} {operation} failed with status {status}: {detail}")
    }
}
