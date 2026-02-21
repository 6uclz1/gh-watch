use anyhow::{Context, Result};
use chrono::Utc;

use crate::{
    domain::events::{EventKind, WatchEvent},
    ports::{NotificationDispatchResult, NotificationPayload, NotifierPort},
};

#[derive(Debug, Clone)]
pub struct NotificationTestOutcome {
    pub event_key: String,
    pub dispatch_result: NotificationDispatchResult,
}

pub fn run_notification_test<N>(notifier: &N) -> Result<NotificationTestOutcome>
where
    N: NotifierPort,
{
    let event = build_test_event();
    let dispatch_result = notifier
        .notify(&NotificationPayload::Event(event.clone()), true)
        .context("failed to send test notification")?;

    Ok(NotificationTestOutcome {
        event_key: event.event_key(),
        dispatch_result,
    })
}

fn build_test_event() -> WatchEvent {
    let now = Utc::now();
    WatchEvent {
        event_id: "notification-test".to_string(),
        repo: "gh-watch".to_string(),
        kind: EventKind::IssueCommentCreated,
        actor: "gh-watch".to_string(),
        title: "gh-watch notification test".to_string(),
        url: "https://github.com/6uclz1/gh-watch".to_string(),
        created_at: now,
        source_item_id: now.timestamp_millis().to_string(),
        subject_author: Some("gh-watch".to_string()),
        requested_reviewer: None,
        mentions: Vec::new(),
    }
}
