use std::{fs, path::Path, path::PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use chrono::{Duration, Utc};
use gh_watch::domain::{
    events::{EventKind, WatchEvent},
    failure::FailureRecord,
};
use gh_watch::infra::state_sqlite::SqliteStateStore;
use gh_watch::ports::StateStorePort;
use predicates::str::contains;
use tempfile::tempdir;

#[test]
fn once_json_returns_partial_failure_exit_code_2() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db_path = dir.path().join("state.db");
    write_config(&config_path, &state_db_path, &["acme/api", "acme/web"]);

    let gh_path = write_stub_gh(
        dir.path(),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi
if [[ "$1" == "api" ]]; then
  endpoint="${@: -1}"
  if [[ "$endpoint" == "repos/acme/web/pulls"* ]]; then
    echo "boom" >&2
    exit 1
  fi
  if [[ "$endpoint" == "repos/acme/api/pulls"* ]]; then
    echo '[]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/issues"* ]]; then
    echo '[]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/web/issues"* ]]; then
    echo '[]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/issues/comments"* ]]; then
    echo '[[]]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/web/issues/comments"* ]]; then
    echo '[[]]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/pulls/comments"* ]]; then
    echo '[[]]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/web/pulls/comments"* ]]; then
    echo '[[]]'
    exit 0
  fi
fi
echo "unexpected args: $@" >&2
exit 1
"#,
    );

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("once")
        .arg("--config")
        .arg(&config_path)
        .arg("--json")
        .env("GH_WATCH_GH_BIN", gh_path)
        .assert()
        .failure()
        .code(2)
        .stdout(contains("\"repo_errors\":["));
}

#[test]
fn help_lists_once_report_and_doctor_commands() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("once"))
        .stdout(contains("report"))
        .stdout(contains("doctor"));
}

#[test]
fn once_dry_run_keeps_cursor_and_tables_unchanged() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db_path = dir.path().join("state.db");
    write_config(&config_path, &state_db_path, &["acme/api"]);

    let cursor = Utc::now() - Duration::hours(2);
    let store = SqliteStateStore::new(&state_db_path).unwrap();
    store.set_cursor("acme/api", cursor).unwrap();

    let gh_path = write_stub_gh(
        dir.path(),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi
if [[ "$1" == "api" ]]; then
  endpoint="${@: -1}"
  if [[ "$endpoint" == "repos/acme/api/pulls/comments"* ]]; then
    echo '[[]]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/pulls"* ]]; then
    cat <<JSON
[{"id":101,"number":1,"title":"Add API","html_url":"https://example.com/pr/1","created_at":"2025-01-02T00:00:00Z","updated_at":"2025-01-02T00:00:00Z","user":{"login":"alice"},"requested_reviewers":[]}]
JSON
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/issues/comments"* ]]; then
    echo '[[]]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/issues"* ]]; then
    echo '[]'
    exit 0
  fi
fi
echo "unexpected args: $@" >&2
exit 1
"#,
    );

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("once")
        .arg("--config")
        .arg(&config_path)
        .arg("--dry-run")
        .env("GH_WATCH_GH_BIN", gh_path)
        .assert()
        .success();

    let conn = rusqlite::Connection::open(&state_db_path).unwrap();
    let notified_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM notified_events", [], |row| row.get(0))
        .unwrap();
    let timeline_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM timeline_events", [], |row| row.get(0))
        .unwrap();
    let cursor_after: String = conn
        .query_row(
            "SELECT last_polled_at FROM polling_cursors WHERE repo = ?1",
            rusqlite::params!["acme/api"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(notified_count, 0);
    assert_eq!(timeline_count, 0);
    assert_eq!(cursor_after, cursor.to_rfc3339());
}

#[test]
fn report_markdown_default_includes_counts() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db_path = dir.path().join("state.db");
    write_config(&config_path, &state_db_path, &["acme/api"]);
    seed_report_data(&state_db_path);

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("report")
        .arg("--config")
        .arg(&config_path)
        .assert()
        .success()
        .stdout(contains("# gh-watch report"))
        .stdout(contains("events_total: 1"))
        .stdout(contains("failures_total: 1"));
}

#[test]
fn report_json_is_machine_readable() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db_path = dir.path().join("state.db");
    write_config(&config_path, &state_db_path, &["acme/api"]);
    seed_report_data(&state_db_path);

    let output = cargo_bin_cmd!("gh-watch")
        .arg("report")
        .arg("--config")
        .arg(&config_path)
        .arg("--since")
        .arg("72h")
        .arg("--format")
        .arg("json")
        .output()
        .unwrap();

    assert!(output.status.success());
    let raw = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(parsed["events_total"].as_u64().unwrap(), 1);
    assert_eq!(parsed["failures_total"].as_u64().unwrap(), 1);
}

#[test]
fn doctor_fails_when_gh_auth_is_invalid() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db_path = dir.path().join("state.db");
    write_config(&config_path, &state_db_path, &["acme/api"]);

    let gh_path = write_stub_gh(
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
    cmd.arg("doctor")
        .arg("--config")
        .arg(&config_path)
        .env("GH_WATCH_GH_BIN", gh_path)
        .assert()
        .failure()
        .stderr(contains("GitHub authentication is invalid"));
}

#[test]
fn doctor_succeeds_and_reports_checks() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db_path = dir.path().join("state.db");
    write_config(&config_path, &state_db_path, &["acme/api"]);

    let gh_path = write_stub_gh(
        dir.path(),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi
if [[ "$1" == "api" && "$2" == "user" ]]; then
  echo "alice"
  exit 0
fi
echo "unexpected args: $@" >&2
exit 1
"#,
    );

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("doctor")
        .arg("--config")
        .arg(&config_path)
        .env("GH_WATCH_GH_BIN", gh_path)
        .assert()
        .success()
        .stdout(contains("config doctor: ok"))
        .stdout(contains("gh auth: ok"))
        .stdout(contains("state db:"));
}

fn write_stub_gh(dir: &Path, script: &str) -> PathBuf {
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

fn write_config(config_path: &Path, state_db_path: &Path, repositories: &[&str]) {
    let escaped = state_db_path.display().to_string().replace('\\', "\\\\");
    let mut src = format!(
        r#"
interval_seconds = 300
bootstrap_lookback_hours = 24
timeline_limit = 500
retention_days = 90
failure_history_limit = 200
state_db_path = "{escaped}"

[notifications]
enabled = true
include_url = true

[poll]
max_concurrency = 4
timeout_seconds = 30
"#
    );

    for repo in repositories {
        src.push_str("\n[[repositories]]\n");
        src.push_str(&format!("name = \"{repo}\"\n"));
        src.push_str("enabled = true\n");
    }

    fs::write(config_path, src).unwrap();
}

fn seed_report_data(state_db_path: &Path) {
    let store = SqliteStateStore::new(state_db_path).unwrap();
    let now = Utc::now();
    store
        .append_timeline_event(&WatchEvent {
            event_id: "ev-1".to_string(),
            repo: "acme/api".to_string(),
            kind: EventKind::IssueCommentCreated,
            actor: "alice".to_string(),
            title: "Report event".to_string(),
            url: "https://example.com/ev-1".to_string(),
            created_at: now - Duration::minutes(30),
            source_item_id: "ev-1".to_string(),
            subject_author: Some("bob".to_string()),
            requested_reviewer: None,
            mentions: vec!["alice".to_string()],
        })
        .unwrap();
    store
        .record_failure(&FailureRecord::new(
            "repo_poll",
            "acme/api",
            now - Duration::minutes(10),
            "temporary failure",
        ))
        .unwrap();
}
