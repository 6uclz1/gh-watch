use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use gh_watch::{
    app::poll_once::poll_once,
    config::{Config, FiltersConfig, NotificationConfig, PollConfig, RepositoryConfig},
    domain::events::{EventKind, WatchEvent},
    ports::{
        ClockPort, GhClientPort, NotificationClickSupport, NotificationDispatchResult,
        NotifierPort, PersistBatchResult, RepoPersistBatch, StateStorePort,
    },
};

type CleanupCall = (u32, chrono::DateTime<Utc>);

#[derive(Clone, Default)]
struct FakeGh {
    viewer_login: Arc<Mutex<String>>,
    events_by_repo: Arc<Mutex<HashMap<String, Vec<WatchEvent>>>>,
    fail_repos: Arc<Mutex<HashMap<String, String>>>,
}

impl FakeGh {
    fn set_events(&self, repo: &str, events: Vec<WatchEvent>) {
        self.events_by_repo
            .lock()
            .unwrap()
            .insert(repo.to_string(), events);
    }

    fn fail_repo(&self, repo: &str, message: &str) {
        self.fail_repos
            .lock()
            .unwrap()
            .insert(repo.to_string(), message.to_string());
    }
}

#[async_trait]
impl GhClientPort for FakeGh {
    async fn check_auth(&self) -> Result<()> {
        Ok(())
    }

    async fn viewer_login(&self) -> Result<String> {
        Ok(self.viewer_login.lock().unwrap().clone())
    }

    async fn fetch_repo_events(
        &self,
        repo: &str,
        _since: chrono::DateTime<Utc>,
    ) -> Result<Vec<WatchEvent>> {
        if let Some(message) = self.fail_repos.lock().unwrap().get(repo).cloned() {
            return Err(anyhow!(message));
        }
        Ok(self
            .events_by_repo
            .lock()
            .unwrap()
            .get(repo)
            .cloned()
            .unwrap_or_default())
    }
}

#[derive(Clone, Default)]
struct FakeState {
    cursors: Arc<Mutex<HashMap<String, chrono::DateTime<Utc>>>>,
    fail_get_cursor: Arc<Mutex<HashSet<String>>>,
    fail_persist_repo: Arc<Mutex<HashSet<String>>>,
    event_log: Arc<Mutex<HashSet<String>>>,
    cleanup_calls: Arc<Mutex<Vec<CleanupCall>>>,
}

impl FakeState {
    fn set_cursor(&self, repo: &str, at: chrono::DateTime<Utc>) {
        self.cursors.lock().unwrap().insert(repo.to_string(), at);
    }

    fn fail_cursor_for_repo(&self, repo: &str) {
        self.fail_get_cursor
            .lock()
            .unwrap()
            .insert(repo.to_string());
    }

    fn fail_persist_for_repo(&self, repo: &str) {
        self.fail_persist_repo
            .lock()
            .unwrap()
            .insert(repo.to_string());
    }
}

impl StateStorePort for FakeState {
    fn get_cursor(&self, repo: &str) -> Result<Option<chrono::DateTime<Utc>>> {
        if self.fail_get_cursor.lock().unwrap().contains(repo) {
            return Err(anyhow!("cursor read failed for {repo}"));
        }
        Ok(self.cursors.lock().unwrap().get(repo).cloned())
    }

    fn set_cursor(&self, repo: &str, at: chrono::DateTime<Utc>) -> Result<()> {
        self.cursors.lock().unwrap().insert(repo.to_string(), at);
        Ok(())
    }

    fn load_timeline_events(&self, _limit: usize) -> Result<Vec<WatchEvent>> {
        Ok(Vec::new())
    }

    fn mark_timeline_event_read(
        &self,
        _event_key: &str,
        _read_at: chrono::DateTime<Utc>,
    ) -> Result<()> {
        Ok(())
    }

    fn load_read_event_keys(&self, _event_keys: &[String]) -> Result<HashSet<String>> {
        Ok(HashSet::new())
    }

    fn cleanup_old(&self, retention_days: u32, now: chrono::DateTime<Utc>) -> Result<()> {
        self.cleanup_calls
            .lock()
            .unwrap()
            .push((retention_days, now));
        Ok(())
    }

    fn persist_repo_batch(&self, batch: &RepoPersistBatch) -> Result<PersistBatchResult> {
        if self
            .fail_persist_repo
            .lock()
            .unwrap()
            .contains(&batch.repo.clone())
        {
            return Err(anyhow!("persist batch failed for {}", batch.repo));
        }

        self.cursors
            .lock()
            .unwrap()
            .insert(batch.repo.clone(), batch.poll_started_at);
        let mut logged = self.event_log.lock().unwrap();
        let mut result = PersistBatchResult::default();
        for event in &batch.events {
            let key = event.event_key();
            if logged.insert(key.clone()) {
                result.newly_logged_event_keys.push(key);
            }
        }
        Ok(result)
    }
}

#[derive(Clone, Default)]
struct FakeNotifier {
    sent: Arc<Mutex<Vec<String>>>,
    fail_once: Arc<Mutex<HashSet<String>>>,
}

impl FakeNotifier {
    fn fail_once_for(&self, event_key: &str) {
        self.fail_once.lock().unwrap().insert(event_key.to_string());
    }

    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl NotifierPort for FakeNotifier {
    fn check_health(&self) -> Result<()> {
        Ok(())
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        NotificationClickSupport::Unsupported
    }

    fn notify(&self, event: &WatchEvent, _include_url: bool) -> Result<NotificationDispatchResult> {
        let key = event.event_key();
        if self.fail_once.lock().unwrap().remove(&key) {
            return Err(anyhow!("notify failed once"));
        }
        self.sent.lock().unwrap().push(key);
        Ok(NotificationDispatchResult::Delivered)
    }
}

#[derive(Clone)]
struct FixedClock {
    now: chrono::DateTime<Utc>,
}

impl ClockPort for FixedClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        self.now
    }
}

fn cfg() -> Config {
    Config {
        interval_seconds: 300,
        bootstrap_lookback_hours: 24,
        timeline_limit: 500,
        retention_days: 90,
        state_db_path: None,
        repositories: vec![
            RepositoryConfig {
                name: "acme/api".to_string(),
                enabled: true,
                event_kinds: None,
            },
            RepositoryConfig {
                name: "acme/web".to_string(),
                enabled: true,
                event_kinds: None,
            },
        ],
        notifications: NotificationConfig {
            enabled: true,
            include_url: true,
        },
        filters: FiltersConfig::default(),
        poll: PollConfig {
            max_concurrency: 4,
            timeout_seconds: 30,
        },
    }
}

fn event(repo: &str, id: &str, created_at: chrono::DateTime<Utc>) -> WatchEvent {
    WatchEvent {
        event_id: id.to_string(),
        repo: repo.to_string(),
        kind: EventKind::IssueCommentCreated,
        actor: "dev".to_string(),
        title: "comment".to_string(),
        url: format!("https://example.com/{id}"),
        created_at,
        source_item_id: id.to_string(),
        subject_author: Some("dev".to_string()),
        requested_reviewer: None,
        mentions: Vec::new(),
    }
}

#[tokio::test]
async fn bootstrap_poll_populates_timeline_without_notifications() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    };

    gh.set_events(
        "acme/api",
        vec![event(
            "acme/api",
            "ev-1",
            Utc.with_ymd_and_hms(2025, 1, 19, 23, 0, 0).unwrap(),
        )],
    );
    gh.set_events("acme/web", Vec::new());

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .unwrap();
    assert_eq!(out.bootstrap_repos, 2);
    assert_eq!(out.timeline_events.len(), 1);
    assert_eq!(out.notified_count, 0);
    assert!(notifier.sent().is_empty());
}

#[tokio::test]
async fn non_bootstrap_poll_notifies_new_events() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 20, 0, 10, 0).unwrap(),
    };

    state.set_cursor(
        "acme/api",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    );
    state.set_cursor(
        "acme/web",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    );
    gh.set_events(
        "acme/api",
        vec![event(
            "acme/api",
            "ev-2",
            Utc.with_ymd_and_hms(2025, 1, 20, 0, 5, 0).unwrap(),
        )],
    );
    gh.set_events("acme/web", Vec::new());

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .unwrap();
    assert_eq!(out.bootstrap_repos, 0);
    assert_eq!(out.notified_count, 1);
    assert_eq!(notifier.sent().len(), 1);
    assert_eq!(state.cleanup_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn repo_fetch_failure_returns_error() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    };

    state.set_cursor(
        "acme/api",
        Utc.with_ymd_and_hms(2025, 1, 19, 0, 0, 0).unwrap(),
    );
    state.set_cursor(
        "acme/web",
        Utc.with_ymd_and_hms(2025, 1, 19, 0, 0, 0).unwrap(),
    );
    gh.fail_repo("acme/api", "boom");

    let err = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect_err("repo fetch should fail");
    assert!(err.to_string().contains("acme/api"));
}

#[tokio::test]
async fn notification_failure_returns_error() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 21, 0, 0, 0).unwrap(),
    };

    let ev = event(
        "acme/api",
        "ev-3",
        Utc.with_ymd_and_hms(2025, 1, 21, 0, 0, 0).unwrap(),
    );
    state.set_cursor(
        "acme/api",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    );
    state.set_cursor(
        "acme/web",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    );
    gh.set_events("acme/api", vec![ev.clone()]);
    gh.set_events("acme/web", Vec::new());
    notifier.fail_once_for(&ev.event_key());

    let err = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect_err("notification should fail");
    assert!(err.to_string().contains("notification failed for"));
}

#[tokio::test]
async fn cursor_failure_returns_error() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 22, 0, 0, 0).unwrap(),
    };

    state.fail_cursor_for_repo("acme/api");
    let err = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect_err("cursor load should fail");
    assert!(err
        .to_string()
        .contains("failed to load cursor for acme/api"));
}

#[tokio::test]
async fn persist_failure_returns_error() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 23, 0, 0, 0).unwrap(),
    };

    state.set_cursor(
        "acme/api",
        Utc.with_ymd_and_hms(2025, 1, 22, 0, 0, 0).unwrap(),
    );
    state.set_cursor(
        "acme/web",
        Utc.with_ymd_and_hms(2025, 1, 22, 0, 0, 0).unwrap(),
    );
    state.fail_persist_for_repo("acme/api");
    gh.set_events(
        "acme/api",
        vec![event(
            "acme/api",
            "ev-4",
            Utc.with_ymd_and_hms(2025, 1, 23, 0, 0, 0).unwrap(),
        )],
    );
    gh.set_events("acme/web", Vec::new());

    let err = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect_err("persist should fail");
    assert!(err
        .to_string()
        .contains("failed to persist event batch for acme/api"));
}
