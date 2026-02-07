use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::path::Path;

use chrono::{TimeZone, Utc};
use gh_watch::domain::events::EventKind;
use gh_watch::infra::gh_client::GhCliClient;
use gh_watch::ports::GhClientPort;
use tempfile::tempdir;

fn write_stub_gh(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(path).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(path, perm).unwrap();
    }
}

fn count_kind(events: &[gh_watch::domain::events::WatchEvent], kind: EventKind) -> usize {
    events.iter().filter(|e| e.kind == kind).count()
}

#[tokio::test]
async fn fetch_repo_events_reads_all_pages_for_all_event_types() {
    let dir = tempdir().unwrap();
    let gh_path = dir.path().join("gh");
    let log_path = dir.path().join("calls.log");

    let script = r#"#!/usr/bin/env bash
set -euo pipefail
LOG_PATH="__LOG_PATH__"
echo "$*" >> "$LOG_PATH"

emit_timestamp() {
  local id="$1"
  local minute=$((id / 60))
  local second=$((id % 60))
  printf "2025-01-02T00:%02d:%02dZ" "$minute" "$second"
}

emit_items() {
  local kind="$1"
  local start="$2"
  local end="$3"
  local first=1
  printf '['
  for ((id=start; id>=end; id--)); do
    if [[ $first -eq 0 ]]; then
      printf ','
    fi
    first=0
    ts="$(emit_timestamp "$id")"
    case "$kind" in
      pull)
        printf '{"id":%d,"title":"PR %d","html_url":"https://example.com/pr/%d","created_at":"%s","user":{"login":"alice"}}' "$id" "$id" "$id" "$ts"
        ;;
      issue)
        printf '{"id":%d,"title":"Issue %d","html_url":"https://example.com/issue/%d","created_at":"%s","user":{"login":"bob"},"pull_request":null}' "$id" "$id" "$id" "$ts"
        ;;
      issue_comment)
        printf '{"id":%d,"html_url":"https://example.com/issue-comment/%d","created_at":"%s","body":"issue comment %d","user":{"login":"carol"}}' "$id" "$id" "$ts" "$id"
        ;;
      review_comment)
        printf '{"id":%d,"html_url":"https://example.com/review-comment/%d","created_at":"%s","body":"review comment %d","user":{"login":"dave"}}' "$id" "$id" "$ts" "$id"
        ;;
      *)
        echo "unexpected kind: $kind" >&2
        exit 1
        ;;
    esac
  done
  printf ']'
}

emit_page() {
  local kind="$1"
  local page="$2"
  if [[ "$page" == "1" ]]; then
    emit_items "$kind" 150 51
    return
  fi
  if [[ "$page" == "2" ]]; then
    emit_items "$kind" 50 1
    return
  fi
  printf '[]'
}

emit_slurp() {
  local kind="$1"
  printf '['
  emit_page "$kind" 1
  printf ','
  emit_page "$kind" 2
  printf ']'
}

if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi

if [[ "$1" != "api" ]]; then
  echo "unexpected args: $*" >&2
  exit 1
fi

endpoint="${@: -1}"

if [[ "$endpoint" == "repos/acme/api/issues/comments"* ]]; then
  emit_slurp issue_comment
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/pulls/comments"* ]]; then
  emit_slurp review_comment
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/pulls"* ]]; then
  page="${endpoint##*page=}"
  emit_page pull "$page"
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/issues"* ]]; then
  page="${endpoint##*page=}"
  emit_page issue "$page"
  exit 0
fi

echo "unexpected endpoint: $endpoint" >&2
exit 1
"#
    .replace("__LOG_PATH__", &log_path.to_string_lossy());

    write_stub_gh(&gh_path, &script);

    let gh = GhCliClient::new_with_bin(&gh_path);
    let since = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let events = gh.fetch_repo_events("acme/api", since).await.unwrap();

    assert_eq!(events.len(), 600);
    assert_eq!(count_kind(&events, EventKind::PrCreated), 150);
    assert_eq!(count_kind(&events, EventKind::IssueCreated), 150);
    assert_eq!(count_kind(&events, EventKind::IssueCommentCreated), 150);
    assert_eq!(count_kind(&events, EventKind::PrReviewCommentCreated), 150);
    assert!(events.iter().all(|e| e.created_at > since));
}

#[tokio::test]
async fn fetch_repo_events_stops_when_since_is_reached() {
    let dir = tempdir().unwrap();
    let gh_path = dir.path().join("gh");
    let log_path = dir.path().join("calls.log");

    let script = r#"#!/usr/bin/env bash
set -euo pipefail
LOG_PATH="__LOG_PATH__"
echo "$*" >> "$LOG_PATH"

emit_recent_page() {
  local kind="$1"
  local first=1
  printf '['
  for ((id=200; id>=101; id--)); do
    if [[ $first -eq 0 ]]; then
      printf ','
    fi
    first=0
    local minute=$((id / 60))
    local second=$((id % 60))
    local ts
    ts="$(printf "2025-01-02T00:%02d:%02dZ" "$minute" "$second")"
    if [[ "$kind" == "pull" ]]; then
      printf '{"id":%d,"title":"PR %d","html_url":"https://example.com/pr/%d","created_at":"%s","user":{"login":"alice"}}' "$id" "$id" "$id" "$ts"
    else
      printf '{"id":%d,"title":"Issue %d","html_url":"https://example.com/issue/%d","created_at":"%s","user":{"login":"bob"},"pull_request":null}' "$id" "$id" "$id" "$ts"
    fi
  done
  printf ']'
}

emit_cutoff_page() {
  local kind="$1"
  if [[ "$kind" == "pull" ]]; then
    cat <<JSON
[{"id":100,"title":"PR 100","html_url":"https://example.com/pr/100","created_at":"2025-01-02T00:01:40Z","user":{"login":"alice"}},{"id":99,"title":"PR 99","html_url":"https://example.com/pr/99","created_at":"2025-01-02T00:01:39Z","user":{"login":"alice"}},{"id":98,"title":"PR 98","html_url":"https://example.com/pr/98","created_at":"2025-01-01T00:00:00Z","user":{"login":"alice"}}]
JSON
  else
    cat <<JSON
[{"id":100,"title":"Issue 100","html_url":"https://example.com/issue/100","created_at":"2025-01-02T00:01:40Z","user":{"login":"bob"},"pull_request":null},{"id":99,"title":"Issue 99","html_url":"https://example.com/issue/99","created_at":"2025-01-02T00:01:39Z","user":{"login":"bob"},"pull_request":null},{"id":98,"title":"Issue 98","html_url":"https://example.com/issue/98","created_at":"2025-01-01T00:00:00Z","user":{"login":"bob"},"pull_request":null}]
JSON
  fi
}

if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi

if [[ "$1" != "api" ]]; then
  echo "unexpected args: $*" >&2
  exit 1
fi

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
  page="${endpoint##*page=}"
  if [[ "$page" == "1" ]]; then
    emit_recent_page pull
    exit 0
  fi
  if [[ "$page" == "2" ]]; then
    emit_cutoff_page pull
    exit 0
  fi
  echo "unexpected pulls page: $page" >&2
  exit 1
fi

if [[ "$endpoint" == "repos/acme/api/issues"* ]]; then
  page="${endpoint##*page=}"
  if [[ "$page" == "1" ]]; then
    emit_recent_page issue
    exit 0
  fi
  if [[ "$page" == "2" ]]; then
    emit_cutoff_page issue
    exit 0
  fi
  echo "unexpected issues page: $page" >&2
  exit 1
fi

echo "unexpected endpoint: $endpoint" >&2
exit 1
"#
    .replace("__LOG_PATH__", &log_path.to_string_lossy());

    write_stub_gh(&gh_path, &script);

    let gh = GhCliClient::new_with_bin(&gh_path);
    let since = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let events = gh.fetch_repo_events("acme/api", since).await.unwrap();
    let log = fs::read_to_string(&log_path).unwrap();

    assert_eq!(events.len(), 204);
    assert_eq!(count_kind(&events, EventKind::PrCreated), 102);
    assert_eq!(count_kind(&events, EventKind::IssueCreated), 102);
    assert!(!log.contains(
        "repos/acme/api/pulls?state=all&sort=created&direction=desc&per_page=100&page=3"
    ));
    assert!(!log.contains(
        "repos/acme/api/issues?state=all&sort=created&direction=desc&per_page=100&page=3"
    ));
}

#[tokio::test]
async fn fetch_repo_events_fails_when_max_pages_are_exceeded() {
    let dir = tempdir().unwrap();
    let gh_path = dir.path().join("gh");

    let script = r#"#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi

if [[ "$1" != "api" ]]; then
  echo "unexpected args: $*" >&2
  exit 1
fi

endpoint="${@: -1}"

if [[ "$endpoint" == "repos/acme/api/pulls"* ]]; then
  cat <<JSON
[{"id":1,"title":"PR 1","html_url":"https://example.com/pr/1","created_at":"2025-01-02T00:00:00Z","user":{"login":"alice"}}]
JSON
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/issues"* ]]; then
  echo '[]'
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/issues/comments"* ]]; then
  echo '[[]]'
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/pulls/comments"* ]]; then
  echo '[[]]'
  exit 0
fi

echo "unexpected endpoint: $endpoint" >&2
exit 1
"#;

    write_stub_gh(&gh_path, script);

    let gh = GhCliClient::new_with_bin(&gh_path);
    let since = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let err = gh.fetch_repo_events("acme/api", since).await.unwrap_err();
    let msg = format!("{err:#}");

    assert!(msg.contains("max pages reached while fetching pulls"));
    assert!(msg.contains("limit=1000"));
}

#[cfg(unix)]
#[tokio::test]
async fn fetch_repo_events_retries_when_gh_binary_is_temporarily_busy() {
    let dir = tempdir().unwrap();
    let gh_path = dir.path().join("gh");

    let script = r#"#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == "auth" && "$2" == "status" ]]; then
  exit 0
fi

if [[ "$1" != "api" ]]; then
  echo "unexpected args: $*" >&2
  exit 1
fi

endpoint="${@: -1}"

if [[ "$endpoint" == "repos/acme/api/pulls"* ]]; then
  echo '[]'
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/issues"* ]]; then
  echo '[]'
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/issues/comments"* ]]; then
  echo '[[]]'
  exit 0
fi

if [[ "$endpoint" == "repos/acme/api/pulls/comments"* ]]; then
  echo '[[]]'
  exit 0
fi

echo "unexpected endpoint: $endpoint" >&2
exit 1
"#;

    write_stub_gh(&gh_path, script);

    // Hold the stub binary open for write briefly; Linux returns ETXTBSY for exec.
    let busy_handle = OpenOptions::new().write(true).open(&gh_path).unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(90)).await;
        drop(busy_handle);
    });

    let gh = GhCliClient::new_with_bin(&gh_path);
    let since = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let events = gh.fetch_repo_events("acme/api", since).await.unwrap();
    release.await.unwrap();

    assert!(events.is_empty());
}
