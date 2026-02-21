use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration as StdDuration,
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use gh_watch::{
    app::poll_once::poll_once,
    config::{Config, FiltersConfig, NotificationConfig, PollConfig, RepositoryConfig},
    domain::events::{EventKind, WatchEvent},
    ports::{
        ClockPort, CursorPort, GhClientPort, NotificationClickSupport, NotificationDispatchResult,
        NotificationPayload, NotifierPort, PersistBatchResult, RepoBatchPort, RepoPersistBatch,
        RetentionPort,
    },
};

type CleanupCall = (u32, chrono::DateTime<Utc>);

#[derive(Clone, Default)]
struct FakeGh {
    viewer_login: Arc<Mutex<String>>,
    events_by_repo: Arc<Mutex<HashMap<String, Vec<WatchEvent>>>>,
    fail_repos: Arc<Mutex<HashMap<String, String>>>,
    fail_n_times_repos: Arc<Mutex<HashMap<String, (usize, String)>>>,
    fetch_delay_ms_by_repo: Arc<Mutex<HashMap<String, u64>>>,
    fetch_attempts_by_repo: Arc<Mutex<HashMap<String, usize>>>,
    in_flight_fetches: Arc<Mutex<usize>>,
    max_concurrent_fetches: Arc<Mutex<usize>>,
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

    fn fail_repo_n_times(&self, repo: &str, times: usize, message: &str) {
        self.fail_n_times_repos
            .lock()
            .unwrap()
            .insert(repo.to_string(), (times, message.to_string()));
    }

    fn set_fetch_delay_ms(&self, repo: &str, delay_ms: u64) {
        self.fetch_delay_ms_by_repo
            .lock()
            .unwrap()
            .insert(repo.to_string(), delay_ms);
    }

    fn fetch_attempt_count(&self, repo: &str) -> usize {
        self.fetch_attempts_by_repo
            .lock()
            .unwrap()
            .get(repo)
            .copied()
            .unwrap_or(0)
    }

    fn max_in_flight_fetches(&self) -> usize {
        *self.max_concurrent_fetches.lock().unwrap()
    }
}

struct InFlightGuard {
    counter: Arc<Mutex<usize>>,
}

impl InFlightGuard {
    fn enter(counter: Arc<Mutex<usize>>, max_counter: Arc<Mutex<usize>>) -> Self {
        {
            let mut active = counter.lock().unwrap();
            *active += 1;
            let mut max = max_counter.lock().unwrap();
            if *active > *max {
                *max = *active;
            }
        }
        Self { counter }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut active = self.counter.lock().unwrap();
        *active = active.saturating_sub(1);
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
        let _guard = InFlightGuard::enter(
            self.in_flight_fetches.clone(),
            self.max_concurrent_fetches.clone(),
        );

        self.fetch_attempts_by_repo
            .lock()
            .unwrap()
            .entry(repo.to_string())
            .and_modify(|attempts| *attempts += 1)
            .or_insert(1);

        let delay_ms = self
            .fetch_delay_ms_by_repo
            .lock()
            .unwrap()
            .get(repo)
            .copied()
            .unwrap_or(0);
        if delay_ms > 0 {
            tokio::time::sleep(StdDuration::from_millis(delay_ms)).await;
        }

        let transient_error = {
            let mut transient = self.fail_n_times_repos.lock().unwrap();
            if let Some((remaining, message)) = transient.get_mut(repo) {
                if *remaining > 0 {
                    *remaining -= 1;
                    Some(message.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(message) = transient_error {
            return Err(anyhow!(message));
        }

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

impl CursorPort for FakeState {
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
}

impl RetentionPort for FakeState {
    fn cleanup_old(&self, retention_days: u32, now: chrono::DateTime<Utc>) -> Result<()> {
        self.cleanup_calls
            .lock()
            .unwrap()
            .push((retention_days, now));
        Ok(())
    }
}

impl RepoBatchPort for FakeState {
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
    sent: Arc<Mutex<Vec<NotificationPayload>>>,
    fail_once_for_event: Arc<Mutex<HashSet<String>>>,
    fail_digest_once: Arc<Mutex<bool>>,
}

impl FakeNotifier {
    fn fail_once_for_event(&self, event_key: &str) {
        self.fail_once_for_event
            .lock()
            .unwrap()
            .insert(event_key.to_string());
    }

    fn fail_digest_once(&self) {
        *self.fail_digest_once.lock().unwrap() = true;
    }

    fn sent(&self) -> Vec<NotificationPayload> {
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

    fn notify(
        &self,
        payload: &NotificationPayload,
        _include_url: bool,
    ) -> Result<NotificationDispatchResult> {
        match payload {
            NotificationPayload::Event(event) => {
                let key = event.event_key();
                if self.fail_once_for_event.lock().unwrap().remove(&key) {
                    return Err(anyhow!("notify failed once"));
                }
            }
            NotificationPayload::Digest(_) => {
                let mut fail_once = self.fail_digest_once.lock().unwrap();
                if *fail_once {
                    *fail_once = false;
                    return Err(anyhow!("digest notify failed once"));
                }
            }
        }
        self.sent.lock().unwrap().push(payload.clone());
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
    let sent = notifier.sent();
    assert_eq!(sent.len(), 1);
    assert!(matches!(sent[0], NotificationPayload::Event(_)));
    assert_eq!(state.cleanup_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn repo_fetch_partial_failure_returns_success_with_failure_details() {
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
    gh.set_events(
        "acme/web",
        vec![event(
            "acme/web",
            "ev-web-1",
            Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
        )],
    );

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect("partial failure should still return success");
    assert_eq!(out.notified_count, 1);
    assert_eq!(out.fetch_failures.len(), 1);
    assert_eq!(out.fetch_failures[0].repo, "acme/api");
    assert!(out.fetch_failures[0].message.contains("boom"));
}

#[tokio::test]
async fn repo_fetch_returns_error_when_all_repositories_fail() {
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
    gh.fail_repo("acme/api", "api boom");
    gh.fail_repo("acme/web", "web boom");

    let err = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect_err("all repo failures should return error");
    let message = err.to_string();
    assert!(message.contains("all repository fetches failed"));
    assert!(message.contains("acme/api"));
    assert!(message.contains("acme/web"));
}

#[tokio::test]
async fn repo_fetch_retries_temporary_failures_and_succeeds() {
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
    gh.fail_repo_n_times("acme/api", 2, "transient");
    gh.set_events(
        "acme/api",
        vec![event(
            "acme/api",
            "ev-retry-1",
            Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
        )],
    );
    gh.set_events("acme/web", Vec::new());

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect("third attempt should succeed");
    assert_eq!(gh.fetch_attempt_count("acme/api"), 3);
    assert_eq!(out.fetch_failures.len(), 0);
    assert_eq!(out.notified_count, 1);
}

#[tokio::test]
async fn repo_fetch_runs_sequentially() {
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
    gh.set_fetch_delay_ms("acme/api", 30);
    gh.set_fetch_delay_ms("acme/web", 30);
    gh.set_events("acme/api", Vec::new());
    gh.set_events("acme/web", Vec::new());

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect("poll should succeed");
    assert_eq!(out.fetch_failures.len(), 0);
    assert_eq!(gh.max_in_flight_fetches(), 1);
}

#[tokio::test]
async fn multiple_events_in_single_poll_send_digest_notification_once() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 21, 0, 0, 0).unwrap(),
    };

    state.set_cursor(
        "acme/api",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    );
    state.set_cursor(
        "acme/web",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    );

    let ev_latest = event(
        "acme/api",
        "ev-latest",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 7, 0).unwrap(),
    );
    let ev_tie_api = event(
        "acme/api",
        "ev-tie-api",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 6, 0).unwrap(),
    );
    let ev_tie_web = event(
        "acme/web",
        "ev-tie-web",
        Utc.with_ymd_and_hms(2025, 1, 20, 0, 6, 0).unwrap(),
    );

    gh.set_events("acme/api", vec![ev_tie_api.clone(), ev_latest.clone()]);
    gh.set_events("acme/web", vec![ev_tie_web.clone()]);

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect("poll with multiple events should succeed");

    let expected_keys = vec![
        ev_latest.event_key(),
        ev_tie_api.event_key(),
        ev_tie_web.event_key(),
    ];
    let notified_keys = out
        .notified_events
        .iter()
        .map(|event| event.event_key())
        .collect::<Vec<_>>();
    assert_eq!(out.notified_count, 1);
    assert_eq!(notified_keys, expected_keys);

    let sent = notifier.sent();
    assert_eq!(sent.len(), 1);
    match &sent[0] {
        NotificationPayload::Digest(digest) => {
            assert_eq!(digest.total_events, 3);
            let sample_keys = digest
                .sample_events
                .iter()
                .map(|event| event.event_key())
                .collect::<Vec<_>>();
            assert_eq!(sample_keys, expected_keys);
        }
        NotificationPayload::Event(_) => panic!("expected digest payload for multiple events"),
    }
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
    notifier.fail_once_for_event(&ev.event_key());

    let err = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect_err("notification should fail");
    assert!(err.to_string().contains("notification failed for"));
}

#[tokio::test]
async fn digest_notification_failure_returns_error() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 21, 0, 30, 0).unwrap(),
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
            "ev-digest-a",
            Utc.with_ymd_and_hms(2025, 1, 21, 0, 10, 0).unwrap(),
        )],
    );
    gh.set_events(
        "acme/web",
        vec![event(
            "acme/web",
            "ev-digest-b",
            Utc.with_ymd_and_hms(2025, 1, 21, 0, 11, 0).unwrap(),
        )],
    );
    notifier.fail_digest_once();

    let err = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .expect_err("digest notification should fail");
    assert!(err.to_string().contains("digest notification failed"));
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
