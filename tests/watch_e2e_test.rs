use std::fs;

use chrono::{TimeZone, Utc};
use gh_watch::app::poll_once::poll_once;
use gh_watch::config::{Config, FiltersConfig, NotificationConfig, PollConfig, RepositoryConfig};
use gh_watch::infra::gh_client::GhCliClient;
use gh_watch::infra::notifier::NoopNotifier;
use gh_watch::infra::state_sqlite::SqliteStateStore;
use gh_watch::ports::ClockPort;
use tempfile::tempdir;

#[derive(Clone)]
struct FixedClock {
    now: chrono::DateTime<Utc>,
}

impl ClockPort for FixedClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        self.now
    }
}

#[tokio::test]
async fn poll_once_with_stubbed_gh_binary() {
    let dir = tempdir().unwrap();
    let gh_path = dir.path().join("gh");
    let script = r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi
if [[ "$1" == "api" ]]; then
  endpoint="${@: -1}"
  if [[ "$endpoint" == "repos/acme/api/issues/comments"* ]]; then
    echo '[[]]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/pulls/comments"* ]]; then
    echo '[[]]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/pulls"* ]]; then
    if [[ "$endpoint" == *"page=1" ]]; then
      cat <<JSON
[{"id":101,"number":1,"title":"Add API","html_url":"https://example.com/pr/1","created_at":"2025-01-02T00:00:00Z","user":{"login":"alice"}}]
JSON
      exit 0
    fi
    echo '[]'
    exit 0
  fi
  if [[ "$endpoint" == "repos/acme/api/issues"* ]]; then
    echo '[]'
    exit 0
  fi
fi
echo "unexpected args: $@" >&2
exit 1
"#;
    fs::write(&gh_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&gh_path).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&gh_path, perm).unwrap();
    }

    let store = SqliteStateStore::new(dir.path().join("state.db")).unwrap();
    let gh = GhCliClient::new_with_bin(gh_path);
    let notifier = NoopNotifier;

    let cfg = Config {
        interval_seconds: 300,
        bootstrap_lookback_hours: 24,
        timeline_limit: 500,
        retention_days: 90,
        state_db_path: None,
        repositories: vec![RepositoryConfig {
            name: "acme/api".to_string(),
            enabled: true,
            event_kinds: None,
        }],
        notifications: NotificationConfig {
            enabled: true,
            include_url: true,
        },
        filters: FiltersConfig::default(),
        poll: PollConfig {
            max_concurrency: Some(4),
            timeout_seconds: 30,
        },
    };

    let c1 = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    };
    let c2 = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    };

    let first = poll_once(&cfg, &gh, &store, &notifier, &c1).await.unwrap();
    assert_eq!(first.notified_count, 0);

    let second = poll_once(&cfg, &gh, &store, &notifier, &c2).await.unwrap();
    assert_eq!(second.notified_count, 1);
}
