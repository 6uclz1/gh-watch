use std::{
    fs,
    path::{Path, PathBuf},
};

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

#[test]
fn init_reset_state_recreates_custom_state_db() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db = dir.path().join("state.db");
    write_config_with_state_db_path(&config_path, &state_db);
    seed_legacy_timeline_event(&state_db);
    assert_eq!(timeline_event_count(&state_db), 1);

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init")
        .arg("--reset-state")
        .arg("--path")
        .arg(&config_path)
        .assert()
        .success()
        .stdout(contains("reset state db:"))
        .stdout(contains(state_db.display().to_string()));

    assert!(state_db.exists());
    assert_eq!(timeline_event_count(&state_db), 0);
}

#[test]
fn init_reset_state_keeps_config_file_unchanged() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db = dir.path().join("state.db");
    write_config_with_state_db_path(&config_path, &state_db);
    let before = fs::read_to_string(&config_path).unwrap();

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init")
        .arg("--reset-state")
        .arg("--path")
        .arg(&config_path)
        .assert()
        .success();

    let after = fs::read_to_string(&config_path).unwrap();
    assert_eq!(before, after);
}

#[test]
fn init_reset_state_rejects_interactive_mode() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    write_config_with_state_db_path(&config_path, &dir.path().join("state.db"));

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init")
        .arg("--reset-state")
        .arg("--interactive")
        .arg("--path")
        .arg(&config_path)
        .assert()
        .failure()
        .stderr(contains("--reset-state cannot be used with --interactive"));
}

#[test]
fn init_reset_state_fails_when_config_is_invalid() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        r#"
interval_seconds = "oops"

[[repositories]]
name = "acme/api"
"#,
    )
    .unwrap();

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("init")
        .arg("--reset-state")
        .arg("--path")
        .arg(&config_path)
        .assert()
        .failure()
        .stderr(contains("failed to parse config TOML"));
}

#[test]
fn init_reset_state_uses_default_state_db_when_config_is_missing() {
    let dir = tempdir().unwrap();
    let missing_config = dir.path().join("missing.toml");

    #[cfg(not(windows))]
    let expected_state = {
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let mut cmd = cargo_bin_cmd!("gh-watch");
        cmd.arg("init")
            .arg("--reset-state")
            .arg("--path")
            .arg(&missing_config)
            .env("HOME", &home)
            .assert()
            .success();

        home.join(".local")
            .join("share")
            .join("gh-watch")
            .join("state.db")
    };

    #[cfg(windows)]
    let expected_state = {
        let local_appdata = dir.path().join("localappdata");
        fs::create_dir_all(&local_appdata).unwrap();

        let mut cmd = cargo_bin_cmd!("gh-watch");
        cmd.arg("init")
            .arg("--reset-state")
            .arg("--path")
            .arg(&missing_config)
            .env("LOCALAPPDATA", &local_appdata)
            .assert()
            .success();

        local_appdata.join("gh-watch").join("state.db")
    };

    assert!(expected_state.exists());
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

fn write_config_with_state_db_path(config_path: &Path, state_db_path: &Path) {
    let escaped = state_db_path.display().to_string().replace('\\', "\\\\");
    let src = format!(
        r#"
interval_seconds = 300
state_db_path = "{escaped}"

[[repositories]]
name = "acme/api"
"#
    );
    fs::write(config_path, src).unwrap();
}

fn seed_legacy_timeline_event(state_db_path: &Path) {
    if let Some(parent) = state_db_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }

    let conn = rusqlite::Connection::open(state_db_path).unwrap();
    conn.execute_batch(
        "
CREATE TABLE IF NOT EXISTS timeline_events (
  event_key TEXT PRIMARY KEY,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
",
    )
    .unwrap();

    let payload = r#"{"event_id":"pr:1","repo":"acme/api","kind":"PrCreated","actor":"alice","title":"hello","url":"https://example.com/pull/1","created_at":"2026-02-07T16:15:06Z","source_item_id":"1"}"#;
    conn.execute(
        "
INSERT OR REPLACE INTO timeline_events (event_key, payload_json, created_at)
VALUES (?1, ?2, ?3)
",
        rusqlite::params!["acme/api:pr_created:1", payload, "2026-02-07T16:15:06Z"],
    )
    .unwrap();
}

fn timeline_event_count(state_db_path: &Path) -> i64 {
    let conn = rusqlite::Connection::open(state_db_path).unwrap();
    let has_event_log_v2: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='event_log_v2')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    if has_event_log_v2 == 1 {
        return conn
            .query_row("SELECT COUNT(*) FROM event_log_v2", [], |row| row.get(0))
            .unwrap();
    }

    let has_legacy_timeline: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='timeline_events')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    if has_legacy_timeline == 1 {
        return conn
            .query_row("SELECT COUNT(*) FROM timeline_events", [], |row| row.get(0))
            .unwrap();
    }

    0
}
