use std::collections::HashSet;

use chrono::{DateTime, Utc};

use crate::domain::{events::WatchEvent, failure::FailureRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Timeline,
    Repositories,
}

impl ActiveTab {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Timeline => Self::Repositories,
            Self::Repositories => Self::Timeline,
        }
    }

    pub(crate) fn prev(self) -> Self {
        match self {
            Self::Timeline => Self::Repositories,
            Self::Repositories => Self::Timeline,
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Timeline => 0,
            Self::Repositories => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TuiModel {
    pub timeline: Vec<WatchEvent>,
    timeline_all: Vec<WatchEvent>,
    read_event_keys: HashSet<String>,
    pub watched_repositories: Vec<String>,
    pub selected: usize,
    pub timeline_offset: usize,
    pub timeline_page_size: usize,
    pub selected_event_key: Option<String>,
    pub help_visible: bool,
    pub status_line: String,
    pub failure_count: u64,
    pub latest_failure: Option<FailureRecord>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
    pub is_polling: bool,
    pub poll_started_at: Option<DateTime<Utc>>,
    pub queued_refresh: bool,
    pub active_tab: ActiveTab,
    pub esc_armed_until: Option<DateTime<Utc>>,
    limit: usize,
}

impl TuiModel {
    pub fn new(limit: usize) -> Self {
        Self {
            timeline: Vec::new(),
            timeline_all: Vec::new(),
            read_event_keys: HashSet::new(),
            watched_repositories: Vec::new(),
            selected: 0,
            timeline_offset: 0,
            timeline_page_size: 1,
            selected_event_key: None,
            help_visible: false,
            status_line: "starting".to_string(),
            failure_count: 0,
            latest_failure: None,
            last_success_at: None,
            next_poll_at: None,
            is_polling: false,
            poll_started_at: None,
            queued_refresh: false,
            active_tab: ActiveTab::Timeline,
            esc_armed_until: None,
            limit,
        }
    }

    pub fn push_timeline(&mut self, mut events: Vec<WatchEvent>) {
        let previous_selected_key = self.snapshot_selected_key();
        self.timeline_all.append(&mut events);
        self.normalize_timeline_all();
        self.rebuild_timeline(previous_selected_key);
    }

    pub fn replace_timeline(&mut self, mut events: Vec<WatchEvent>) {
        let previous_selected_key = self.snapshot_selected_key();
        self.timeline_all.clear();
        self.timeline_all.append(&mut events);
        self.normalize_timeline_all();
        self.rebuild_timeline(previous_selected_key);
    }

    pub fn replace_read_event_keys(&mut self, read_event_keys: HashSet<String>) {
        self.read_event_keys = read_event_keys;
    }

    pub(crate) fn mark_event_read(&mut self, event_key: &str) {
        self.read_event_keys.insert(event_key.to_string());
    }

    pub(crate) fn is_event_read(&self, event_key: &str) -> bool {
        self.read_event_keys.contains(event_key)
    }

    fn normalize_timeline_all(&mut self) {
        self.timeline_all
            .sort_by(|a, b| b.created_at.cmp(&a.created_at));
        self.timeline_all
            .dedup_by(|a, b| a.event_key() == b.event_key());
        self.timeline_all.truncate(self.limit);
    }

    fn snapshot_selected_key(&self) -> Option<String> {
        self.selected_event_key
            .clone()
            .or_else(|| self.timeline.get(self.selected).map(WatchEvent::event_key))
    }

    fn rebuild_timeline(&mut self, previous_selected_key: Option<String>) {
        self.timeline = self.timeline_all.clone();
        self.restore_selection(previous_selected_key);
    }

    fn restore_selection(&mut self, previous_selected_key: Option<String>) {
        if self.timeline.is_empty() {
            self.selected = 0;
            self.timeline_offset = 0;
            self.selected_event_key = None;
            return;
        }

        if let Some(key) = previous_selected_key {
            if let Some(index) = self.timeline.iter().position(|ev| ev.event_key() == key) {
                self.selected = index;
            } else if self.selected >= self.timeline.len() {
                self.selected = self.timeline.len() - 1;
            }
        } else if self.selected >= self.timeline.len() {
            self.selected = self.timeline.len() - 1;
        }

        self.sync_selected_event_key();
    }

    pub(crate) fn sync_selected_event_key(&mut self) {
        self.selected_event_key = self.timeline.get(self.selected).map(WatchEvent::event_key);
    }

    pub(crate) fn page_size(&self) -> usize {
        self.timeline_page_size.max(1)
    }
}
