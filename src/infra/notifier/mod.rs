use anyhow::{anyhow, Result};

use crate::{
    domain::events::WatchEvent,
    ports::{NotificationClickSupport, NotificationDispatchResult, NotifierPort},
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub fn build_notification_body(event: &WatchEvent, include_url: bool) -> String {
    let mut lines = vec![format!("{} by @{}", event.title, event.actor)];
    if include_url {
        lines.push(event.url.clone());
    }
    lines.join("\n")
}

fn build_notification_title(event: &WatchEvent) -> String {
    format!("{} [{}]", event.repo, event.kind)
}

trait PlatformNotifier {
    fn check_health(&self) -> Result<()>;
    fn click_action_support(&self) -> NotificationClickSupport;
    fn notify(&self, title: &str, body: &str, click_url: Option<&str>) -> Result<()>;
}

#[derive(Debug, Clone, Copy)]
struct SystemPlatformNotifier;

impl PlatformNotifier for SystemPlatformNotifier {
    fn check_health(&self) -> Result<()> {
        platform::check_health()
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        platform::click_action_support()
    }

    fn notify(&self, title: &str, body: &str, click_url: Option<&str>) -> Result<()> {
        platform::notify(title, body, click_url)
    }
}

fn dispatch_notification<P: PlatformNotifier>(
    platform: &P,
    event: &WatchEvent,
    include_url: bool,
) -> Result<NotificationDispatchResult> {
    let title = build_notification_title(event);
    let body = build_notification_body(event, include_url);

    match platform.click_action_support() {
        NotificationClickSupport::Supported => {
            match platform.notify(&title, &body, Some(&event.url)) {
                Ok(()) => Ok(NotificationDispatchResult::DeliveredWithClickAction),
                Err(click_err) => {
                    platform
                    .notify(&title, &body, None)
                    .map_err(|fallback_err| {
                        anyhow!(
                            "notification click-action failed: {click_err}; fallback delivery also failed: {fallback_err}"
                        )
                    })?;
                    if include_url {
                        Ok(NotificationDispatchResult::DeliveredWithBodyUrlFallback)
                    } else {
                        Ok(NotificationDispatchResult::Delivered)
                    }
                }
            }
        }
        NotificationClickSupport::Unsupported => {
            platform.notify(&title, &body, None)?;
            if include_url {
                Ok(NotificationDispatchResult::DeliveredWithBodyUrlFallback)
            } else {
                Ok(NotificationDispatchResult::Delivered)
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DesktopNotifier;

impl NotifierPort for DesktopNotifier {
    fn check_health(&self) -> Result<()> {
        SystemPlatformNotifier.check_health()
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        SystemPlatformNotifier.click_action_support()
    }

    fn notify(&self, event: &WatchEvent, include_url: bool) -> Result<NotificationDispatchResult> {
        dispatch_notification(&SystemPlatformNotifier, event, include_url)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NoopNotifier;

impl NotifierPort for NoopNotifier {
    fn check_health(&self) -> Result<()> {
        Ok(())
    }

    fn click_action_support(&self) -> NotificationClickSupport {
        NotificationClickSupport::Unsupported
    }

    fn notify(
        &self,
        _event: &WatchEvent,
        _include_url: bool,
    ) -> Result<NotificationDispatchResult> {
        Ok(NotificationDispatchResult::Delivered)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub use super::macos::{check_health, click_action_support, notify};
}

#[cfg(target_os = "linux")]
mod platform {
    pub use super::linux::{check_health, click_action_support, notify};
}

#[cfg(target_os = "windows")]
mod platform {
    pub use super::windows::{check_health, click_action_support, notify};
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    use anyhow::Result;

    use crate::ports::NotificationClickSupport;

    pub fn check_health() -> Result<()> {
        Ok(())
    }

    pub fn click_action_support() -> NotificationClickSupport {
        NotificationClickSupport::Unsupported
    }

    pub fn notify(_title: &str, _body: &str, _click_url: Option<&str>) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{TimeZone, Utc};

    use super::{build_notification_body, dispatch_notification, PlatformNotifier};
    use crate::{
        domain::events::{EventKind, WatchEvent},
        ports::{NotificationClickSupport, NotificationDispatchResult},
    };

    #[derive(Clone)]
    struct FakePlatform {
        support: NotificationClickSupport,
        fail_click_delivery: bool,
        calls: Arc<Mutex<Vec<(String, String, Option<String>)>>>,
    }

    impl FakePlatform {
        fn new(support: NotificationClickSupport) -> Self {
            Self {
                support,
                fail_click_delivery: false,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl PlatformNotifier for FakePlatform {
        fn check_health(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn click_action_support(&self) -> NotificationClickSupport {
            self.support
        }

        fn notify(&self, title: &str, body: &str, click_url: Option<&str>) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push((
                title.to_string(),
                body.to_string(),
                click_url.map(ToString::to_string),
            ));
            if self.fail_click_delivery && click_url.is_some() {
                return Err(anyhow::anyhow!("click delivery failed"));
            }
            Ok(())
        }
    }

    fn sample_event() -> WatchEvent {
        WatchEvent {
            event_id: "1".to_string(),
            repo: "acme/api".to_string(),
            kind: EventKind::PrCreated,
            actor: "alice".to_string(),
            title: "Add feature".to_string(),
            url: "https://example.com/pr/1".to_string(),
            created_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
            source_item_id: "1".to_string(),
        }
    }

    #[test]
    fn click_supported_uses_click_action_and_preserves_include_url_behavior() {
        let platform = FakePlatform::new(NotificationClickSupport::Supported);
        let event = sample_event();

        let dispatch = dispatch_notification(&platform, &event, false).unwrap();

        assert_eq!(
            dispatch,
            NotificationDispatchResult::DeliveredWithClickAction
        );
        let calls = platform.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].2.as_deref(), Some(event.url.as_str()));
        assert!(!calls[0].1.contains("https://example.com/pr/1"));
    }

    #[test]
    fn click_unsupported_falls_back_to_body_url() {
        let platform = FakePlatform::new(NotificationClickSupport::Unsupported);
        let event = sample_event();

        let dispatch = dispatch_notification(&platform, &event, true).unwrap();

        assert_eq!(
            dispatch,
            NotificationDispatchResult::DeliveredWithBodyUrlFallback
        );
        let calls = platform.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.contains("https://example.com/pr/1"));
        assert!(calls[0].2.is_none());
    }

    #[test]
    fn click_delivery_failure_falls_back_without_failing_notification() {
        let mut platform = FakePlatform::new(NotificationClickSupport::Supported);
        platform.fail_click_delivery = true;
        let event = sample_event();

        let dispatch = dispatch_notification(&platform, &event, true).unwrap();

        assert_eq!(
            dispatch,
            NotificationDispatchResult::DeliveredWithBodyUrlFallback
        );
        let calls = platform.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].2.as_deref(), Some(event.url.as_str()));
        assert!(calls[1].2.is_none());
    }

    #[test]
    fn notification_body_contains_url_when_requested() {
        let event = sample_event();
        let body = build_notification_body(&event, true);
        assert!(body.contains("https://example.com/pr/1"));
    }
}
