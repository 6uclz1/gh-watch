use std::{fs, path::Path, path::PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use chrono::{Duration, Utc};
use gh_watch::infra::state_sqlite::SqliteStateStore;
use gh_watch::ports::CursorPort;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn once_json_returns_success_exit_code_0_when_some_repos_fail() {
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
        .success()
        .code(0)
        .stdout(predicate::str::contains("\"fetch_failures\""))
        .stdout(predicate::str::contains("\"repo\":\"acme/web\""));
}

#[test]
fn once_text_reports_repo_fetch_failures_when_partial_failures_happen() {
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
        .env("GH_WATCH_GH_BIN", gh_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("repo_fetch_failures: 1"))
        .stdout(predicate::str::contains("- acme/web:"));
}

#[test]
fn once_returns_failure_exit_code_1_when_all_repos_fail() {
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
  if [[ "$endpoint" == "repos/acme/api/pulls"* ]]; then
    echo "api boom" >&2
    exit 1
  fi
  if [[ "$endpoint" == "repos/acme/web/pulls"* ]]; then
    echo "web boom" >&2
    exit 1
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
        .code(1)
        .stderr(predicate::str::contains("all repository fetches failed"));
}

#[test]
fn help_lists_minimal_commands_and_hides_removed_commands() {
    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("watch"))
        .stdout(predicate::str::contains("once"))
        .stdout(predicate::str::contains("check"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("commands"))
        .stdout(predicate::str::contains("completion"))
        .stdout(predicate::str::contains("report").not())
        .stdout(predicate::str::contains("doctor").not())
        .stdout(predicate::str::contains("notification-test").not());
}

#[test]
fn removed_top_level_commands_are_unavailable() {
    for command in ["report", "doctor", "notification-test"] {
        let mut cmd = cargo_bin_cmd!("gh-watch");
        cmd.arg(command)
            .assert()
            .failure()
            .stderr(predicate::str::contains("unrecognized subcommand"));
    }
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
    let timeline_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM event_log_v2", [], |row| row.get(0))
        .unwrap();
    let cursor_after: String = conn
        .query_row(
            "SELECT last_polled_at FROM polling_cursors_v2 WHERE repo = ?1",
            rusqlite::params!["acme/api"],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(timeline_count, 0);
    assert_eq!(cursor_after, cursor.to_rfc3339());
}

#[test]
fn check_fails_with_reset_hint_when_state_schema_is_legacy() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db_path = dir.path().join("state.db");
    write_config(&config_path, &state_db_path, &["acme/api"]);
    seed_legacy_state_db(&state_db_path);

    let gh_path = write_stub_gh(
        dir.path(),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi
echo "unexpected args: $@" >&2
exit 1
"#,
    );

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("check")
        .arg("--config")
        .arg(&config_path)
        .env("GH_WATCH_GH_BIN", gh_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("init --reset-state"));
}

#[test]
fn once_fails_with_reset_hint_when_state_schema_is_legacy() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let state_db_path = dir.path().join("state.db");
    write_config(&config_path, &state_db_path, &["acme/api"]);
    seed_legacy_state_db(&state_db_path);

    let gh_path = write_stub_gh(
        dir.path(),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi
echo "unexpected args: $@" >&2
exit 1
"#,
    );

    let mut cmd = cargo_bin_cmd!("gh-watch");
    cmd.arg("once")
        .arg("--config")
        .arg(&config_path)
        .env("GH_WATCH_GH_BIN", gh_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("init --reset-state"));
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

fn seed_legacy_state_db(state_db_path: &Path) {
    let conn = rusqlite::Connection::open(state_db_path).unwrap();
    conn.execute_batch(
        "
CREATE TABLE timeline_events (
  event_key TEXT PRIMARY KEY,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
",
    )
    .unwrap();
}
