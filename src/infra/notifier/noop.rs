use anyhow::Result;

use crate::ports::{
    NotificationClickSupport, NotificationDispatchResult, NotificationPayload, NotifierPort,
};

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
        _payload: &NotificationPayload,
        _include_url: bool,
    ) -> Result<NotificationDispatchResult> {
        Ok(NotificationDispatchResult::Delivered)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::{
        domain::events::{EventKind, WatchEvent},
        ports::{NotificationDispatchResult, NotificationPayload, NotifierPort},
    };

    use super::NoopNotifier;

    #[test]
    fn noop_notify_returns_delivered() {
        let event = WatchEvent {
            event_id: "1".to_string(),
            repo: "acme/api".to_string(),
            kind: EventKind::PrCreated,
            actor: "alice".to_string(),
            title: "Add feature".to_string(),
            url: "https://example.com/pr/1".to_string(),
            created_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
            source_item_id: "1".to_string(),
            subject_author: Some("alice".to_string()),
            requested_reviewer: None,
            mentions: Vec::new(),
        };

        let notifier = NoopNotifier;
        let result = notifier
            .notify(&NotificationPayload::Event(event), true)
            .expect("notify should succeed");

        assert_eq!(result, NotificationDispatchResult::Delivered);
    }
}
