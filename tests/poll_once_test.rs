use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use gh_watch::app::poll_once::poll_once;
use gh_watch::config::{Config, NotificationConfig, RepositoryConfig};
use gh_watch::domain::events::{EventKind, WatchEvent};
use gh_watch::ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort};

#[derive(Clone, Default)]
struct FakeGh {
    repos: Arc<Mutex<HashMap<String, Vec<WatchEvent>>>>,
    fail_repo: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl GhClientPort for FakeGh {
    async fn check_auth(&self) -> Result<()> {
        Ok(())
    }

    async fn fetch_repo_events(
        &self,
        repo: &str,
        _since: chrono::DateTime<Utc>,
    ) -> Result<Vec<WatchEvent>> {
        if self
            .fail_repo
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| r == repo)
            .unwrap_or(false)
        {
            return Err(anyhow!("boom"));
        }
        Ok(self
            .repos
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
    notified: Arc<Mutex<HashMap<String, WatchEvent>>>,
    timeline: Arc<Mutex<Vec<WatchEvent>>>,
}

impl StateStorePort for FakeState {
    fn get_cursor(&self, repo: &str) -> Result<Option<chrono::DateTime<Utc>>> {
        Ok(self.cursors.lock().unwrap().get(repo).copied())
    }

    fn set_cursor(&self, repo: &str, at: chrono::DateTime<Utc>) -> Result<()> {
        self.cursors.lock().unwrap().insert(repo.to_string(), at);
        Ok(())
    }

    fn is_event_notified(&self, event_key: &str) -> Result<bool> {
        Ok(self.notified.lock().unwrap().contains_key(event_key))
    }

    fn record_notified_event(
        &self,
        event: &WatchEvent,
        _notified_at: chrono::DateTime<Utc>,
    ) -> Result<()> {
        self.notified
            .lock()
            .unwrap()
            .insert(event.event_key(), event.clone());
        Ok(())
    }

    fn append_timeline_event(&self, event: &WatchEvent) -> Result<()> {
        self.timeline.lock().unwrap().push(event.clone());
        Ok(())
    }

    fn load_timeline_events(&self, limit: usize) -> Result<Vec<WatchEvent>> {
        let mut items = self.timeline.lock().unwrap().clone();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        items.truncate(limit);
        Ok(items)
    }

    fn cleanup_old(&self, _retention_days: u32, _now: chrono::DateTime<Utc>) -> Result<()> {
        Ok(())
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
}

impl NotifierPort for FakeNotifier {
    fn check_health(&self) -> Result<()> {
        Ok(())
    }

    fn notify(&self, event: &WatchEvent, _include_url: bool) -> Result<()> {
        let event_key = event.event_key();
        if self.fail_once.lock().unwrap().remove(&event_key) {
            return Err(anyhow!("notify failed once"));
        }
        self.sent.lock().unwrap().push(event_key);
        Ok(())
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
        timeline_limit: 500,
        retention_days: 90,
        state_db_path: None,
        repositories: vec![
            RepositoryConfig {
                name: "acme/api".to_string(),
                enabled: true,
            },
            RepositoryConfig {
                name: "acme/web".to_string(),
                enabled: true,
            },
        ],
        notifications: NotificationConfig {
            enabled: true,
            include_url: true,
        },
    }
}

fn sample_event(repo: &str, id: &str) -> WatchEvent {
    sample_event_at(repo, id, Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap())
}

fn sample_event_at(repo: &str, id: &str, created_at: chrono::DateTime<Utc>) -> WatchEvent {
    WatchEvent {
        event_id: id.to_string(),
        repo: repo.to_string(),
        kind: EventKind::PrCreated,
        actor: "alice".to_string(),
        title: "Add API".to_string(),
        url: "https://example.com/pr/1".to_string(),
        created_at,
        source_item_id: id.to_string(),
    }
}

#[tokio::test]
async fn bootstrap_does_not_notify_and_sets_cursor() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    };

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.notified_count, 0);
    assert_eq!(out.bootstrap_repos, 2);
    assert!(state.get_cursor("acme/api").unwrap().is_some());
    assert!(notifier.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn second_poll_notifies_new_events_once() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/api".to_string(),
        vec![sample_event("acme/api", "123")],
    );

    let state = FakeState::default();
    state
        .set_cursor(
            "acme/api",
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        )
        .unwrap();
    state
        .set_cursor(
            "acme/web",
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        )
        .unwrap();

    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    };

    let out1 = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .unwrap();
    let out2 = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out1.notified_count, 1);
    assert_eq!(out2.notified_count, 0);
}

#[tokio::test]
async fn repo_failure_does_not_block_others() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/web".to_string(),
        vec![sample_event("acme/web", "456")],
    );
    *gh.fail_repo.lock().unwrap() = Some("acme/api".to_string());

    let state = FakeState::default();
    state
        .set_cursor(
            "acme/api",
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        )
        .unwrap();
    state
        .set_cursor(
            "acme/web",
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        )
        .unwrap();

    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    };

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.notified_count, 1);
    assert_eq!(out.repo_errors.len(), 1);
}

#[tokio::test]
async fn notification_failure_retries_failed_event_without_duplicating_successes() {
    let event_time = Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap();
    let first = sample_event_at("acme/api", "123", event_time);
    let second = sample_event_at("acme/api", "456", event_time);

    let gh = FakeGh::default();
    gh.repos
        .lock()
        .unwrap()
        .insert("acme/api".to_string(), vec![first.clone(), second.clone()]);

    let state = FakeState::default();
    state
        .set_cursor(
            "acme/api",
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        )
        .unwrap();
    state
        .set_cursor(
            "acme/web",
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        )
        .unwrap();

    let notifier = FakeNotifier::default();
    notifier.fail_once_for(&second.event_key());

    let c1 = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    };
    let c2 = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 4, 0, 0, 0).unwrap(),
    };
    let c3 = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 5, 0, 0, 0).unwrap(),
    };

    let out1 = poll_once(&cfg(), &gh, &state, &notifier, &c1)
        .await
        .unwrap();
    assert_eq!(out1.notified_count, 1);
    assert_eq!(out1.repo_errors.len(), 1);
    assert_eq!(
        state.get_cursor("acme/api").unwrap().unwrap(),
        event_time - chrono::Duration::nanoseconds(1)
    );

    let out2 = poll_once(&cfg(), &gh, &state, &notifier, &c2)
        .await
        .unwrap();
    assert_eq!(out2.notified_count, 1);
    assert!(out2.repo_errors.is_empty());
    assert_eq!(state.get_cursor("acme/api").unwrap().unwrap(), c2.now);

    let out3 = poll_once(&cfg(), &gh, &state, &notifier, &c3)
        .await
        .unwrap();
    assert_eq!(out3.notified_count, 0);
    assert!(out3.repo_errors.is_empty());

    assert_eq!(
        notifier.sent.lock().unwrap().as_slice(),
        &[first.event_key(), second.event_key()]
    );
}
