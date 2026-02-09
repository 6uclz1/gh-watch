use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use gh_watch::app::notification_test::run_notification_test;
use gh_watch::domain::events::WatchEvent;
use gh_watch::ports::{NotificationClickSupport, NotificationDispatchResult, NotifierPort};

#[derive(Clone)]
enum NotifyOutcome {
    Success(NotificationDispatchResult),
    Failure(String),
}

#[derive(Clone)]
struct FakeNotifier {
    calls: Arc<Mutex<Vec<(WatchEvent, bool)>>>,
    outcome: NotifyOutcome,
}

impl FakeNotifier {
    fn success(result: NotificationDispatchResult) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            outcome: NotifyOutcome::Success(result),
        }
    }

    fn failure(message: &str) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            outcome: NotifyOutcome::Failure(message.to_string()),
        }
    }
}

impl NotifierPort for FakeNotifier {
    fn check_health(&self) -> Result<()> {
        Ok(())
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        NotificationClickSupport::Unsupported
    }

    fn notify(&self, event: &WatchEvent, include_url: bool) -> Result<NotificationDispatchResult> {
        self.calls
            .lock()
            .unwrap()
            .push((event.clone(), include_url));
        match &self.outcome {
            NotifyOutcome::Success(result) => Ok(*result),
            NotifyOutcome::Failure(message) => Err(anyhow!("{message}")),
        }
    }
}

#[test]
fn notification_test_runs_notify_once_with_include_url_enabled() {
    let notifier = FakeNotifier::success(NotificationDispatchResult::Delivered);

    let outcome = run_notification_test(&notifier).unwrap();

    let calls = notifier.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1);
    assert_eq!(
        outcome.dispatch_result,
        NotificationDispatchResult::Delivered
    );
    assert_eq!(outcome.event_key, calls[0].0.event_key());
}

#[test]
fn notification_test_returns_error_when_notify_fails() {
    let notifier = FakeNotifier::failure("notification boom");

    let err = run_notification_test(&notifier).unwrap_err();

    assert!(format!("{err:#}").contains("failed to send test notification"));
    let calls = notifier.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
}
