use crate::{
    domain::events::WatchEvent,
    ports::{NotificationDigest, NotificationDispatchResult, NotificationPayload},
};

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

fn build_digest_notification_body(digest: &NotificationDigest, include_url: bool) -> String {
    let mut lines = vec![format!("{} updates", digest.total_events)];
    for event in &digest.sample_events {
        lines.push(format!(
            "- {} [{}] {} by @{}",
            event.repo, event.kind, event.title, event.actor
        ));
        if include_url {
            lines.push(event.url.clone());
        }
    }

    let remaining = digest
        .total_events
        .saturating_sub(digest.sample_events.len());
    if remaining > 0 {
        lines.push(format!("... and {remaining} more"));
    }

    lines.join("\n")
}

pub(super) fn build_notification_title_from_payload(payload: &NotificationPayload) -> String {
    match payload {
        NotificationPayload::Event(event) => build_notification_title(event),
        NotificationPayload::Digest(_) => "gh-watch [digest]".to_string(),
    }
}

pub(super) fn build_notification_body_from_payload(
    payload: &NotificationPayload,
    include_url: bool,
) -> String {
    match payload {
        NotificationPayload::Event(event) => build_notification_body(event, include_url),
        NotificationPayload::Digest(digest) => build_digest_notification_body(digest, include_url),
    }
}

pub(super) fn dispatch_result(
    include_url: bool,
    click_action: bool,
) -> NotificationDispatchResult {
    if click_action {
        NotificationDispatchResult::DeliveredWithClickAction
    } else if include_url {
        NotificationDispatchResult::DeliveredWithBodyUrlFallback
    } else {
        NotificationDispatchResult::Delivered
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{
        build_digest_notification_body, build_notification_body,
        build_notification_title_from_payload, dispatch_result,
    };
    use crate::domain::events::{EventKind, WatchEvent};
    use crate::ports::{
        NotificationDigest, NotificationDispatchResult, NotificationPayload,
    };

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
            subject_author: Some("alice".to_string()),
            requested_reviewer: None,
            mentions: Vec::new(),
        }
    }

    fn sample_digest(total_events: usize, sample_events: Vec<WatchEvent>) -> NotificationDigest {
        NotificationDigest {
            total_events,
            sample_events,
        }
    }

    #[test]
    fn notification_body_contains_url_when_requested() {
        let body = build_notification_body(&sample_event(), true);
        assert!(body.contains("https://example.com/pr/1"));
    }

    #[test]
    fn digest_notification_title_is_fixed() {
        let payload = NotificationPayload::Digest(sample_digest(2, vec![sample_event()]));
        assert_eq!(
            build_notification_title_from_payload(&payload),
            "gh-watch [digest]"
        );
    }

    #[test]
    fn digest_notification_body_contains_total_samples_and_remaining_count() {
        let first = WatchEvent {
            title: "One".to_string(),
            source_item_id: "1".to_string(),
            ..sample_event()
        };
        let second = WatchEvent {
            repo: "acme/web".to_string(),
            title: "Two".to_string(),
            source_item_id: "2".to_string(),
            ..sample_event()
        };
        let third = WatchEvent {
            repo: "acme/mobile".to_string(),
            title: "Three".to_string(),
            source_item_id: "3".to_string(),
            ..sample_event()
        };
        let digest = sample_digest(5, vec![first, second, third]);

        let body = build_digest_notification_body(&digest, false);

        assert!(body.contains("5 updates"));
        assert!(body.contains("- acme/api [pr_created] One by @alice"));
        assert!(body.contains("- acme/web [pr_created] Two by @alice"));
        assert!(body.contains("- acme/mobile [pr_created] Three by @alice"));
        assert!(body.contains("... and 2 more"));
    }

    #[test]
    fn digest_notification_body_includes_urls_when_requested() {
        let digest = sample_digest(2, vec![sample_event(), sample_event()]);
        let body = build_digest_notification_body(&digest, true);
        assert!(body.matches("https://example.com/pr/1").count() >= 2);
    }

    #[test]
    fn dispatch_result_returns_body_url_fallback_without_click_action() {
        assert_eq!(
            dispatch_result(true, false),
            NotificationDispatchResult::DeliveredWithBodyUrlFallback
        );
    }

    #[test]
    fn dispatch_result_returns_delivered_without_url_and_click_action() {
        assert_eq!(
            dispatch_result(false, false),
            NotificationDispatchResult::Delivered
        );
    }
}
