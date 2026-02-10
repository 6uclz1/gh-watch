use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Result};
use chrono::{TimeZone, Utc};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use super::{handle_stream_event, LoopControl};
use crate::{
    domain::events::{EventKind, WatchEvent},
    ports::{ClockPort, PersistBatchResult, RepoPersistBatch, StateStorePort},
    ui::tui::TuiModel,
};

#[derive(Clone, Default)]
struct FakeState {
    marked_read_event_keys: Arc<Mutex<Vec<String>>>,
    fail_mark_read: Arc<Mutex<bool>>,
}

impl FakeState {
    fn marked_read_event_keys(&self) -> Vec<String> {
        self.marked_read_event_keys.lock().unwrap().clone()
    }

    fn set_mark_read_error(&self, should_fail: bool) {
        *self.fail_mark_read.lock().unwrap() = should_fail;
    }
}

impl StateStorePort for FakeState {
    fn get_cursor(&self, _repo: &str) -> Result<Option<chrono::DateTime<Utc>>> {
        Ok(None)
    }

    fn set_cursor(&self, _repo: &str, _at: chrono::DateTime<Utc>) -> Result<()> {
        Ok(())
    }

    fn load_timeline_events(&self, _limit: usize) -> Result<Vec<WatchEvent>> {
        Ok(Vec::new())
    }

    fn mark_timeline_event_read(
        &self,
        event_key: &str,
        _read_at: chrono::DateTime<Utc>,
    ) -> Result<()> {
        if *self.fail_mark_read.lock().unwrap() {
            return Err(anyhow!("state store down"));
        }
        self.marked_read_event_keys
            .lock()
            .unwrap()
            .push(event_key.to_string());
        Ok(())
    }

    fn load_read_event_keys(&self, event_keys: &[String]) -> Result<HashSet<String>> {
        let existing = self
            .marked_read_event_keys
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .collect::<HashSet<_>>();
        Ok(event_keys
            .iter()
            .filter(|key| existing.contains(*key))
            .cloned()
            .collect())
    }

    fn cleanup_old(&self, _retention_days: u32, _now: chrono::DateTime<Utc>) -> Result<()> {
        Ok(())
    }

    fn persist_repo_batch(&self, _batch: &RepoPersistBatch) -> Result<PersistBatchResult> {
        Ok(PersistBatchResult::default())
    }
}

struct FixedClock {
    now: chrono::DateTime<Utc>,
}

impl ClockPort for FixedClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        self.now
    }
}

fn test_area() -> Rect {
    Rect::new(0, 0, 120, 40)
}

fn open_fail(_url: &str) -> Result<()> {
    Err(anyhow!("launcher missing"))
}

fn open_ok(_url: &str) -> Result<()> {
    Ok(())
}

fn timeline_event(id: &str, created_at: chrono::DateTime<Utc>) -> WatchEvent {
    WatchEvent {
        event_id: id.to_string(),
        repo: "acme/api".to_string(),
        kind: EventKind::IssueCommentCreated,
        actor: "dev".to_string(),
        title: "comment".to_string(),
        url: format!("https://example.com/{id}"),
        created_at,
        source_item_id: id.to_string(),
        subject_author: Some("dev".to_string()),
        requested_reviewer: None,
        mentions: Vec::new(),
    }
}

#[test]
fn stream_error_sets_status_and_redraws() {
    let state = FakeState::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 7, 0, 0, 0).unwrap(),
    };
    let mut model = TuiModel::new(10);

    let control = handle_stream_event(
        Some(Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "event stream disconnected",
        ))),
        &mut model,
        &state,
        &clock,
        test_area(),
        &open_ok,
    );

    assert_eq!(control, LoopControl::Redraw);
    assert_eq!(model.failure_count, 1);
    assert!(model.status_line.contains("input stream failed"));
}

#[test]
fn enter_open_failure_updates_status_and_requests_redraw() {
    let state = FakeState::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 0).unwrap(),
    };
    let mut model = TuiModel::new(10);
    model.timeline = vec![timeline_event("ev-open-fail", clock.now)];

    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let control = handle_stream_event(
        Some(Ok(Event::Key(key))),
        &mut model,
        &state,
        &clock,
        test_area(),
        &open_fail,
    );

    assert_eq!(control, LoopControl::Redraw);
    assert_eq!(model.status_line, "open failed: launcher missing");
}

#[test]
fn mouse_selection_marks_selected_event_as_read() {
    let state = FakeState::default();
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 30, 0).unwrap(),
    };
    let mut model = TuiModel::new(10);
    model.timeline = vec![
        timeline_event("ev-mouse-1", clock.now),
        timeline_event("ev-mouse-2", clock.now),
    ];

    let mouse = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 9,
        modifiers: KeyModifiers::NONE,
    };
    let control = handle_stream_event(
        Some(Ok(Event::Mouse(mouse))),
        &mut model,
        &state,
        &clock,
        test_area(),
        &open_ok,
    );

    assert_eq!(control, LoopControl::Redraw);
    assert_eq!(model.selected, 1);
    assert_eq!(
        state.marked_read_event_keys(),
        vec![model.timeline[1].event_key()]
    );
}

#[test]
fn read_mark_failure_keeps_event_unread_and_sets_status() {
    let state = FakeState::default();
    state.set_mark_read_error(true);
    let clock = FixedClock {
        now: Utc.with_ymd_and_hms(2025, 1, 9, 1, 30, 0).unwrap(),
    };
    let mut model = TuiModel::new(10);
    model.timeline = vec![
        timeline_event("ev-read-fail-1", clock.now),
        timeline_event("ev-read-fail-2", clock.now),
    ];

    let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let control = handle_stream_event(
        Some(Ok(Event::Key(key))),
        &mut model,
        &state,
        &clock,
        test_area(),
        &open_ok,
    );

    assert_eq!(control, LoopControl::Redraw);
    assert_eq!(model.selected, 1);
    assert!(state.marked_read_event_keys().is_empty());
    assert!(model
        .status_line
        .contains("read mark failed: state store down"));
    assert!(!model.is_event_read(&model.timeline[1].event_key()));
}
