use std::collections::HashSet;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::domain::events::WatchEvent;

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
}

#[derive(Debug, Clone, Default)]
pub struct PersistBatchResult {
    pub newly_logged_event_keys: Vec<String>,
}

pub trait StateStorePort: Send + Sync {
    fn get_cursor(&self, repo: &str) -> Result<Option<DateTime<Utc>>>;
    fn set_cursor(&self, repo: &str, at: DateTime<Utc>) -> Result<()>;
    fn load_timeline_events(&self, limit: usize) -> Result<Vec<WatchEvent>>;
    fn mark_timeline_event_read(&self, event_key: &str, read_at: DateTime<Utc>) -> Result<()>;
    fn load_read_event_keys(&self, event_keys: &[String]) -> Result<HashSet<String>>;
    fn cleanup_old(&self, retention_days: u32, now: DateTime<Utc>) -> Result<()>;
    fn persist_repo_batch(&self, batch: &RepoPersistBatch) -> Result<PersistBatchResult>;
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
