use std::{fs, path::PathBuf};

use assert_cmd::{cargo::cargo_bin_cmd, Command};
use predicates::str::contains;
use tempfile::{tempdir, TempDir};

#[test]
fn init_creates_config_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init").arg("--path").arg(&path).assert().success();

    let content = fs::read_to_string(path).unwrap();
    assert!(content.contains("[[repositories]]"));
    assert!(content.contains("[notifications]"));
}

#[test]
fn init_prevents_overwrite_without_force() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "interval_seconds = 123\n").unwrap();

    let mut fail_cmd = cargo_bin_cmd!("gh-watch");
    fail_cmd
        .arg("init")
        .arg("--path")
        .arg(&path)
        .assert()
        .failure()
        .stderr(contains("use --force to overwrite"));

    let mut ok_cmd = cargo_bin_cmd!("gh-watch");
    ok_cmd
        .arg("init")
        .arg("--path")
        .arg(&path)
        .arg("--force")
        .assert()
        .success();

    let content = fs::read_to_string(path).unwrap();
    assert!(content.contains("[[repositories]]"));
}

#[test]
fn init_without_path_uses_binary_directory_config() {
    let (dir, bin_path) = copy_binary_to_tempdir();
    let config_path = dir.path().join("config.toml");

    let mut cmd = Command::new(&bin_path);
    cmd.arg("init").assert().success();

    let content = fs::read_to_string(config_path).unwrap();
    assert!(content.contains("[[repositories]]"));
}

#[test]
fn init_interactive_fails_fast_when_auth_is_invalid() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let gh = write_stub_gh(
        dir.path(),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  echo "auth required" >&2
  exit 1
fi
echo "unexpected args: $@" >&2
exit 1
"#,
    );

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init")
        .arg("--interactive")
        .arg("--path")
        .arg(&path)
        .env("GH_WATCH_GH_BIN", gh)
        .assert()
        .failure()
        .stderr(contains("gh auth login -h github.com"));

    assert!(!path.exists());
}

#[test]
fn init_interactive_uses_manual_repo_input_when_candidate_fetch_fails() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let gh = write_stub_gh(
        dir.path(),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi
if [[ "$1" == "repo" && "$2" == "list" ]]; then
  echo "repo list unavailable" >&2
  exit 1
fi
echo "unexpected args: $@" >&2
exit 1
"#,
    );

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init")
        .arg("--interactive")
        .arg("--path")
        .arg(&path)
        .env("GH_WATCH_GH_BIN", gh)
        .write_stdin("acme/fallback\n\n\n\n\n")
        .assert()
        .success()
        .stdout(contains("falling back to manual input"));

    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("name = \"acme/fallback\""));
    assert!(content.contains("interval_seconds = 300"));
}

#[test]
fn init_interactive_creates_config_from_prompt_answers() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let gh = write_stub_gh(
        dir.path(),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi
if [[ "$1" == "repo" && "$2" == "list" ]]; then
  cat <<JSON
[{"nameWithOwner":"acme/api"},{"nameWithOwner":"acme/web"}]
JSON
  exit 0
fi
echo "unexpected args: $@" >&2
exit 1
"#,
    );

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init")
        .arg("--interactive")
        .arg("--path")
        .arg(&path)
        .env("GH_WATCH_GH_BIN", gh)
        .write_stdin("1,2\n120\nn\ny\ny\n")
        .assert()
        .success()
        .stdout(contains("repository candidates:"));

    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("interval_seconds = 120"));
    assert!(content.contains("enabled = false"));
    assert!(content.contains("include_url = true"));
    assert!(content.contains("name = \"acme/api\""));
    assert!(content.contains("name = \"acme/web\""));
}

fn copy_binary_to_tempdir() -> (TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let src = assert_cmd::cargo::cargo_bin!("gh-watch");
    let bin_name = if cfg!(windows) {
        "gh-watch.exe"
    } else {
        "gh-watch"
    };
    let dst = dir.path().join(bin_name);
    fs::copy(src, &dst).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&dst).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&dst, perm).unwrap();
    }

    (dir, dst)
}

fn write_stub_gh(dir: &std::path::Path, script: &str) -> PathBuf {
    let path = dir.join("gh");
    fs::write(&path, script).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&path).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&path, perm).unwrap();
    }

    path
}
