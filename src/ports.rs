use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::{events::WatchEvent, failure::FailureRecord};

#[async_trait]
pub trait GhClientPort: Send + Sync {
    async fn check_auth(&self) -> Result<()>;
    async fn fetch_repo_events(&self, repo: &str, since: DateTime<Utc>) -> Result<Vec<WatchEvent>>;
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
    fn cleanup_old(
        &self,
        retention_days: u32,
        failure_history_limit: usize,
        now: DateTime<Utc>,
    ) -> Result<()>;
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
