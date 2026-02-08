use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use gh_watch::app::poll_once::poll_once;
use gh_watch::config::{Config, FiltersConfig, NotificationConfig, PollConfig, RepositoryConfig};
use gh_watch::domain::events::{EventKind, WatchEvent};
use gh_watch::domain::failure::{FailureRecord, FAILURE_KIND_NOTIFICATION, FAILURE_KIND_REPO_POLL};
use gh_watch::ports::{
    ClockPort, GhClientPort, NotificationClickSupport, NotificationDispatchResult, NotifierPort,
    StateStorePort,
};

#[derive(Clone)]
struct FakeGh {
    repos: Arc<Mutex<HashMap<String, Vec<WatchEvent>>>>,
    fail_repo: Arc<Mutex<Option<String>>>,
    delays: Arc<Mutex<HashMap<String, StdDuration>>>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    fetched_repos: Arc<Mutex<Vec<String>>>,
    viewer_login: Arc<Mutex<String>>,
    viewer_login_error: Arc<Mutex<Option<String>>>,
}

impl Default for FakeGh {
    fn default() -> Self {
        Self {
            repos: Arc::new(Mutex::new(HashMap::new())),
            fail_repo: Arc::new(Mutex::new(None)),
            delays: Arc::new(Mutex::new(HashMap::new())),
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: Arc::new(AtomicUsize::new(0)),
            fetched_repos: Arc::new(Mutex::new(Vec::new())),
            viewer_login: Arc::new(Mutex::new("alice".to_string())),
            viewer_login_error: Arc::new(Mutex::new(None)),
        }
    }
}

impl FakeGh {
    fn set_delay(&self, repo: &str, delay: StdDuration) {
        self.delays.lock().unwrap().insert(repo.to_string(), delay);
    }

    fn max_concurrency_seen(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    fn fetched_repos(&self) -> Vec<String> {
        self.fetched_repos.lock().unwrap().clone()
    }
}

#[async_trait]
impl GhClientPort for FakeGh {
    async fn check_auth(&self) -> Result<()> {
        Ok(())
    }

    async fn viewer_login(&self) -> Result<String> {
        if let Some(message) = self.viewer_login_error.lock().unwrap().clone() {
            return Err(anyhow!(message));
        }
        Ok(self.viewer_login.lock().unwrap().clone())
    }

    async fn fetch_repo_events(
        &self,
        repo: &str,
        _since: chrono::DateTime<Utc>,
    ) -> Result<Vec<WatchEvent>> {
        let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
        self.fetched_repos.lock().unwrap().push(repo.to_string());

        let delay = {
            let delays = self.delays.lock().unwrap();
            delays.get(repo).copied()
        };
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }

        let result = if self
            .fail_repo
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| r == repo)
            .unwrap_or(false)
        {
            Err(anyhow!("boom"))
        } else {
            Ok(self
                .repos
                .lock()
                .unwrap()
                .get(repo)
                .cloned()
                .unwrap_or_default())
        };

        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

#[derive(Clone, Default)]
struct FakeState {
    cursors: Arc<Mutex<HashMap<String, chrono::DateTime<Utc>>>>,
    notified: Arc<Mutex<HashMap<String, WatchEvent>>>,
    timeline: Arc<Mutex<Vec<WatchEvent>>>,
    failures: Arc<Mutex<Vec<FailureRecord>>>,
    fail_set_cursor_for: Arc<Mutex<HashSet<String>>>,
}

impl FakeState {
    fn fail_set_cursor_for(&self, repo: &str) {
        self.fail_set_cursor_for
            .lock()
            .unwrap()
            .insert(repo.to_string());
    }
}

impl StateStorePort for FakeState {
    fn get_cursor(&self, repo: &str) -> Result<Option<chrono::DateTime<Utc>>> {
        Ok(self.cursors.lock().unwrap().get(repo).copied())
    }

    fn set_cursor(&self, repo: &str, at: chrono::DateTime<Utc>) -> Result<()> {
        if self.fail_set_cursor_for.lock().unwrap().contains(repo) {
            return Err(anyhow!("cursor write failed for {repo}"));
        }
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

    fn record_failure(&self, failure: &FailureRecord) -> Result<()> {
        self.failures.lock().unwrap().push(failure.clone());
        Ok(())
    }

    fn latest_failure(&self) -> Result<Option<FailureRecord>> {
        Ok(self.failures.lock().unwrap().last().cloned())
    }

    fn load_timeline_events(&self, limit: usize) -> Result<Vec<WatchEvent>> {
        let mut items = self.timeline.lock().unwrap().clone();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        items.truncate(limit);
        Ok(items)
    }

    fn cleanup_old(
        &self,
        _retention_days: u32,
        _failure_history_limit: usize,
        _now: chrono::DateTime<Utc>,
    ) -> Result<()> {
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

    fn click_action_support(&self) -> NotificationClickSupport {
        NotificationClickSupport::Unsupported
    }

    fn notify(&self, event: &WatchEvent, _include_url: bool) -> Result<NotificationDispatchResult> {
        let event_key = event.event_key();
        if self.fail_once.lock().unwrap().remove(&event_key) {
            return Err(anyhow!("notify failed once"));
        }
        self.sent.lock().unwrap().push(event_key);
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
        failure_history_limit: 200,
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
        subject_author: Some("alice".to_string()),
        requested_reviewer: None,
        mentions: Vec::new(),
    }
}

fn sample_event_with_kind_actor(repo: &str, id: &str, kind: EventKind, actor: &str) -> WatchEvent {
    WatchEvent {
        event_id: id.to_string(),
        repo: repo.to_string(),
        kind,
        actor: actor.to_string(),
        title: "Filtered Event".to_string(),
        url: format!("https://example.com/{id}"),
        created_at: Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap(),
        source_item_id: id.to_string(),
        subject_author: Some("alice".to_string()),
        requested_reviewer: None,
        mentions: Vec::new(),
    }
}

fn sample_event_with_involvement(
    repo: &str,
    id: &str,
    kind: EventKind,
    actor: &str,
    subject_author: Option<&str>,
    requested_reviewer: Option<&str>,
    mentions: &[&str],
) -> WatchEvent {
    WatchEvent {
        event_id: id.to_string(),
        repo: repo.to_string(),
        kind,
        actor: actor.to_string(),
        title: "Involvement Event".to_string(),
        url: format!("https://example.com/{id}"),
        created_at: Utc.with_ymd_and_hms(2025, 1, 2, 1, 0, 0).unwrap(),
        source_item_id: id.to_string(),
        subject_author: subject_author.map(ToString::to_string),
        requested_reviewer: requested_reviewer.map(ToString::to_string),
        mentions: mentions.iter().map(|v| (*v).to_string()).collect(),
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
async fn bootstrap_lookback_populates_timeline_without_notifications() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/api".to_string(),
        vec![sample_event("acme/api", "boot-api")],
    );
    gh.repos.lock().unwrap().insert(
        "acme/web".to_string(),
        vec![sample_event("acme/web", "boot-web")],
    );
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    };

    let out = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.bootstrap_repos, 2);
    assert_eq!(out.notified_count, 0);
    assert_eq!(out.timeline_events.len(), 2);
    assert!(notifier.sent.lock().unwrap().is_empty());
    assert_eq!(state.timeline.lock().unwrap().len(), 2);
    assert_eq!(state.get_cursor("acme/api").unwrap().unwrap(), clock.now);
    assert_eq!(state.get_cursor("acme/web").unwrap().unwrap(), clock.now);

    let mut fetched = gh.fetched_repos();
    fetched.sort();
    assert_eq!(
        fetched,
        vec!["acme/api".to_string(), "acme/web".to_string()]
    );
}

#[tokio::test]
async fn bootstrap_lookback_zero_keeps_legacy_behavior_without_backfill() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/api".to_string(),
        vec![sample_event("acme/api", "boot-api")],
    );
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 4, 0, 0, 0).unwrap(),
    };
    let cfg = Config {
        bootstrap_lookback_hours: 0,
        ..cfg()
    };

    let out = poll_once(&cfg, &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.bootstrap_repos, 2);
    assert_eq!(out.notified_count, 0);
    assert!(out.timeline_events.is_empty());
    assert!(state.timeline.lock().unwrap().is_empty());
    assert!(gh.fetched_repos().is_empty());
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
async fn global_event_kind_filter_skips_non_target_events() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/api".to_string(),
        vec![sample_event_with_kind_actor(
            "acme/api",
            "evt-1",
            EventKind::PrCreated,
            "alice",
        )],
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
        now: Utc.with_ymd_and_hms(2025, 1, 5, 0, 0, 0).unwrap(),
    };
    let cfg = Config {
        filters: FiltersConfig {
            event_kinds: vec![EventKind::IssueCreated],
            ignore_actors: Vec::new(),
            only_involving_me: false,
        },
        ..cfg()
    };

    let out = poll_once(&cfg, &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.notified_count, 0);
    assert!(notifier.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ignore_actors_filter_skips_matching_actor_notifications() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/api".to_string(),
        vec![sample_event_with_kind_actor(
            "acme/api",
            "evt-2",
            EventKind::PrCreated,
            "dependabot[bot]",
        )],
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
        now: Utc.with_ymd_and_hms(2025, 1, 5, 0, 0, 0).unwrap(),
    };
    let cfg = Config {
        filters: FiltersConfig {
            event_kinds: Vec::new(),
            ignore_actors: vec!["dependabot[bot]".to_string()],
            only_involving_me: false,
        },
        ..cfg()
    };

    let out = poll_once(&cfg, &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.notified_count, 0);
    assert!(notifier.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn only_involving_me_accepts_review_request_addressed_to_viewer() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/api".to_string(),
        vec![sample_event_with_involvement(
            "acme/api",
            "evt-review-request",
            EventKind::PrReviewRequested,
            "maintainer",
            Some("bob"),
            Some("alice"),
            &[],
        )],
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
        now: Utc.with_ymd_and_hms(2025, 1, 5, 0, 0, 0).unwrap(),
    };
    let cfg = Config {
        filters: FiltersConfig {
            event_kinds: Vec::new(),
            ignore_actors: Vec::new(),
            only_involving_me: true,
        },
        ..cfg()
    };

    let out = poll_once(&cfg, &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.notified_count, 1);
}

#[tokio::test]
async fn only_involving_me_accepts_mentions_and_own_subject_updates() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/api".to_string(),
        vec![
            sample_event_with_involvement(
                "acme/api",
                "evt-mention",
                EventKind::IssueCommentCreated,
                "bob",
                Some("carol"),
                None,
                &["alice"],
            ),
            sample_event_with_involvement(
                "acme/api",
                "evt-self-author",
                EventKind::PrReviewCommentCreated,
                "dave",
                Some("alice"),
                None,
                &[],
            ),
            sample_event_with_involvement(
                "acme/api",
                "evt-unrelated",
                EventKind::IssueCommentCreated,
                "erin",
                Some("frank"),
                None,
                &[],
            ),
        ],
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
        now: Utc.with_ymd_and_hms(2025, 1, 5, 0, 0, 0).unwrap(),
    };
    let cfg = Config {
        filters: FiltersConfig {
            event_kinds: Vec::new(),
            ignore_actors: Vec::new(),
            only_involving_me: true,
        },
        ..cfg()
    };

    let out = poll_once(&cfg, &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.notified_count, 2);
    let sent = notifier.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().any(|k| k.contains("evt-mention")));
    assert!(sent.iter().any(|k| k.contains("evt-self-author")));
}

#[tokio::test]
async fn repository_event_kinds_override_global_filter() {
    let api_event =
        sample_event_with_kind_actor("acme/api", "evt-api", EventKind::PrCreated, "alice");
    let web_event =
        sample_event_with_kind_actor("acme/web", "evt-web", EventKind::PrCreated, "bob");

    let gh = FakeGh::default();
    gh.repos
        .lock()
        .unwrap()
        .insert("acme/api".to_string(), vec![api_event.clone()]);
    gh.repos
        .lock()
        .unwrap()
        .insert("acme/web".to_string(), vec![web_event.clone()]);

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
        now: Utc.with_ymd_and_hms(2025, 1, 5, 0, 0, 0).unwrap(),
    };
    let cfg = Config {
        repositories: vec![
            RepositoryConfig {
                name: "acme/api".to_string(),
                enabled: true,
                event_kinds: Some(vec![EventKind::PrCreated]),
            },
            RepositoryConfig {
                name: "acme/web".to_string(),
                enabled: true,
                event_kinds: None,
            },
        ],
        filters: FiltersConfig {
            event_kinds: vec![EventKind::IssueCreated],
            ignore_actors: Vec::new(),
            only_involving_me: false,
        },
        ..cfg()
    };

    let out = poll_once(&cfg, &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.notified_count, 1);
    assert_eq!(
        notifier.sent.lock().unwrap().as_slice(),
        &[api_event.event_key()]
    );
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
    assert_eq!(out.failures.len(), 1);
    assert_eq!(out.failures[0].kind, FAILURE_KIND_REPO_POLL);
    assert_eq!(out.failures[0].repo, "acme/api");
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
    assert_eq!(out1.failures.len(), 1);
    assert_eq!(out1.failures[0].kind, FAILURE_KIND_NOTIFICATION);
    assert_eq!(
        state.get_cursor("acme/api").unwrap().unwrap(),
        event_time - chrono::Duration::nanoseconds(1)
    );

    let out2 = poll_once(&cfg(), &gh, &state, &notifier, &c2)
        .await
        .unwrap();
    assert_eq!(out2.notified_count, 1);
    assert!(out2.repo_errors.is_empty());
    assert!(out2.failures.is_empty());
    assert_eq!(state.get_cursor("acme/api").unwrap().unwrap(), c2.now);

    let out3 = poll_once(&cfg(), &gh, &state, &notifier, &c3)
        .await
        .unwrap();
    assert_eq!(out3.notified_count, 0);
    assert!(out3.repo_errors.is_empty());
    assert!(out3.failures.is_empty());

    assert_eq!(
        notifier.sent.lock().unwrap().as_slice(),
        &[first.event_key(), second.event_key()]
    );
}

#[tokio::test]
async fn cursor_update_failure_has_repo_and_root_cause() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    state.cursors.lock().unwrap().insert(
        "acme/api".to_string(),
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    );
    state.cursors.lock().unwrap().insert(
        "acme/web".to_string(),
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    );
    state.fail_set_cursor_for("acme/api");

    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    };

    let err = poll_once(&cfg(), &gh, &state, &notifier, &clock)
        .await
        .unwrap_err();

    let msg = format!("{err:#}");
    assert!(msg.contains("failed to update cursor for acme/api"));
    assert!(msg.contains("cursor write failed for acme/api"));
}

#[tokio::test]
async fn poll_once_limits_repo_concurrency_from_config() {
    let gh = FakeGh::default();
    let state = FakeState::default();
    let notifier = FakeNotifier::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 20, 0, 0, 0).unwrap(),
    };
    let repo_names = ["acme/repo-a", "acme/repo-b", "acme/repo-c", "acme/repo-d"];

    for repo in repo_names {
        state
            .set_cursor(repo, Utc.with_ymd_and_hms(2025, 1, 19, 0, 0, 0).unwrap())
            .unwrap();
        gh.set_delay(repo, StdDuration::from_millis(120));
    }

    let cfg = Config {
        repositories: repo_names
            .iter()
            .map(|name| RepositoryConfig {
                name: (*name).to_string(),
                enabled: true,
                event_kinds: None,
            })
            .collect(),
        poll: PollConfig {
            max_concurrency: 2,
            timeout_seconds: 30,
        },
        ..cfg()
    };

    let out = poll_once(&cfg, &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert!(out.repo_errors.is_empty());
    assert_eq!(gh.max_concurrency_seen(), 2);
}

#[tokio::test]
async fn repo_timeout_records_failure_and_other_repo_still_completes() {
    let gh = FakeGh::default();
    gh.repos.lock().unwrap().insert(
        "acme/web".to_string(),
        vec![sample_event("acme/web", "ok-1")],
    );
    gh.set_delay("acme/api", StdDuration::from_millis(1200));

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
        now: Utc.with_ymd_and_hms(2025, 1, 21, 0, 0, 0).unwrap(),
    };
    let cfg = Config {
        poll: PollConfig {
            max_concurrency: 2,
            timeout_seconds: 1,
        },
        ..cfg()
    };

    let out = poll_once(&cfg, &gh, &state, &notifier, &clock)
        .await
        .unwrap();

    assert_eq!(out.notified_count, 1);
    assert_eq!(out.repo_errors.len(), 1);
    assert_eq!(out.failures.len(), 1);
    assert_eq!(out.failures[0].kind, FAILURE_KIND_REPO_POLL);
    assert_eq!(out.failures[0].repo, "acme/api");
    assert!(out.repo_errors[0].contains("acme/api"));
}
