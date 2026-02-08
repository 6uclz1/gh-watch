use std::collections::HashSet;

use super::events::WatchEvent;

#[derive(Debug, Clone)]
pub enum NotificationDecision {
    Notify(Box<WatchEvent>),
    SkipAlreadyNotified,
    SkipBootstrap,
}

pub fn decide_notification(
    event: &WatchEvent,
    is_bootstrap: bool,
    already_notified: &HashSet<String>,
) -> NotificationDecision {
    if is_bootstrap {
        NotificationDecision::SkipBootstrap
    } else if already_notified.contains(&event.event_key()) {
        NotificationDecision::SkipAlreadyNotified
    } else {
        NotificationDecision::Notify(Box::new(event.clone()))
    }
}

pub fn sort_timeline_desc(mut events: Vec<WatchEvent>) -> Vec<WatchEvent> {
    events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    events
}
