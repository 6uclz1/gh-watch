use std::collections::HashSet;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::{events::WatchEvent, failure::FailureRecord};

#[async_trait]
pub trait GhClientPort: Send + Sync {
    async fn check_auth(&self) -> Result<()>;
    async fn viewer_login(&self) -> Result<String>;
    async fn fetch_repo_events(&self, repo: &str, since: DateTime<Utc>) -> Result<Vec<WatchEvent>>;
}

#[derive(Debug, Clone)]
pub struct RepoPersistBatch {
    pub repo: String,
    pub poll_started_at: DateTime<Utc>,
    pub events: Vec<WatchEvent>,
    pub queue_notifications: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PersistBatchResult {
    pub newly_logged_event_keys: Vec<String>,
    pub queued_notifications: usize,
}

#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub event_key: String,
    pub event: WatchEvent,
    pub attempts: u32,
    pub next_attempt_at: DateTime<Utc>,
}

pub trait StateStorePort: Send + Sync {
    fn get_cursor(&self, repo: &str) -> Result<Option<DateTime<Utc>>>;
    fn set_cursor(&self, repo: &str, at: DateTime<Utc>) -> Result<()>;
    fn is_event_notified(&self, event_key: &str) -> Result<bool>;
    fn record_notified_event(&self, event: &WatchEvent, notified_at: DateTime<Utc>) -> Result<()>;
    fn record_failure(&self, failure: &FailureRecord) -> Result<()>;
    fn latest_failure(&self) -> Result<Option<FailureRecord>>;
    fn append_timeline_event(&self, event: &WatchEvent) -> Result<()>;
    fn load_timeline_events(&self, limit: usize) -> Result<Vec<WatchEvent>>;
    fn mark_timeline_event_read(&self, event_key: &str, read_at: DateTime<Utc>) -> Result<()>;
    fn load_read_event_keys(&self, event_keys: &[String]) -> Result<HashSet<String>>;
    fn cleanup_old(
        &self,
        retention_days: u32,
        failure_history_limit: usize,
        now: DateTime<Utc>,
    ) -> Result<()>;
    fn persist_repo_batch(&self, batch: &RepoPersistBatch) -> Result<PersistBatchResult>;
    fn load_due_notifications(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<PendingNotification>>;
    fn mark_notification_delivered(
        &self,
        event_key: &str,
        delivered_at: DateTime<Utc>,
    ) -> Result<()>;
    fn reschedule_notification(
        &self,
        event_key: &str,
        attempts: u32,
        next_attempt_at: DateTime<Utc>,
        last_error: &str,
    ) -> Result<()>;
    fn pending_notification_count(&self) -> Result<usize>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationClickSupport {
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDispatchResult {
    Delivered,
    DeliveredWithClickAction,
    DeliveredWithBodyUrlFallback,
}

pub trait NotifierPort: Send + Sync {
    fn check_health(&self) -> Result<()>;
    fn click_action_support(&self) -> NotificationClickSupport;
    fn notify(&self, event: &WatchEvent, include_url: bool) -> Result<NotificationDispatchResult>;
}

pub trait ClockPort: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
