use std::{future::Future, pin::Pin, process::Command, time::Duration};

use anyhow::{anyhow, Result};
use crossterm::event::Event;
use futures_util::StreamExt;
use ratatui::layout::Rect;
use tokio::time::MissedTickBehavior;

use crate::{
    app::poll_once::{poll_once, PollOutcome},
    config::Config,
    domain::failure::{FailureRecord, FAILURE_KIND_INPUT_STREAM, FAILURE_KIND_POLL_LOOP},
    ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort},
    ui::tui::{handle_input, parse_input, parse_mouse_input, InputCommand, TerminalUi, TuiModel},
};

const SPINNER_REDRAW_INTERVAL_MS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Continue,
    RequestPoll,
    Redraw,
    Quit,
}

type PollFuture<'a> = Pin<Box<dyn Future<Output = Result<PollOutcome>> + 'a>>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PollExecutionState {
    poll_requested: bool,
    in_flight: bool,
    queued_refresh: bool,
}

impl PollExecutionState {
    fn request_poll(&mut self) -> bool {
        if self.in_flight {
            self.queued_refresh = true;
            return false;
        }

        if self.poll_requested {
            return false;
        }

        self.poll_requested = true;
        true
    }

    fn start_poll(&mut self) -> bool {
        if self.in_flight || !self.poll_requested {
            return false;
        }

        self.poll_requested = false;
        self.in_flight = true;
        true
    }

    fn finish_poll_and_take_next_request(&mut self) -> bool {
        self.in_flight = false;

        if self.queued_refresh {
            self.queued_refresh = false;
            self.poll_requested = true;
            return true;
        }

        false
    }

    fn in_flight(&self) -> bool {
        self.in_flight
    }

    fn queued_refresh(&self) -> bool {
        self.queued_refresh
    }
}

fn handle_stream_event<S, K>(
    maybe_event: Option<Result<Event, std::io::Error>>,
    model: &mut TuiModel,
    state: &S,
    clock: &K,
    terminal_area: Rect,
    open_url: &dyn Fn(&str) -> Result<()>,
) -> LoopControl
where
    S: StateStorePort,
    K: ClockPort,
{
    match maybe_event {
        Some(Ok(Event::Key(key))) => {
            let cmd = parse_input(key);
            match cmd {
                InputCommand::Quit => LoopControl::Quit,
                InputCommand::Refresh => LoopControl::RequestPoll,
                InputCommand::OpenSelectedUrl => {
                    let Some(url) = model
                        .timeline
                        .get(model.selected)
                        .map(|event| event.url.clone())
                    else {
                        return LoopControl::Continue;
                    };

                    match open_url(&url) {
                        Ok(()) => {
                            model.status_line = format!("opened: {url}");
                        }
                        Err(err) => {
                            model.status_line = format!("open failed: {err}");
                        }
                    }
                    LoopControl::Redraw
                }
                InputCommand::ToggleHelp => {
                    handle_input(model, cmd);
                    LoopControl::Redraw
                }
                InputCommand::ScrollUp
                | InputCommand::ScrollDown
                | InputCommand::PageUp
                | InputCommand::PageDown
                | InputCommand::JumpTop
                | InputCommand::JumpBottom
                | InputCommand::SelectIndex(_) => {
                    handle_input(model, cmd);
                    LoopControl::Redraw
                }
                InputCommand::None => LoopControl::Continue,
            }
        }
        Some(Ok(Event::Mouse(mouse))) => {
            let cmd = parse_mouse_input(mouse, terminal_area, model);
            match cmd {
                InputCommand::ScrollUp
                | InputCommand::ScrollDown
                | InputCommand::SelectIndex(_) => {
                    handle_input(model, cmd);
                    LoopControl::Redraw
                }
                _ => LoopControl::Continue,
            }
        }
        Some(Ok(Event::Resize(_, _))) => LoopControl::Redraw,
        Some(Ok(_)) => LoopControl::Continue,
        Some(Err(err)) => {
            tracing::warn!(error = %err, "input stream failed");

            model.failure_count += 1;
            model.status_line = format!("input stream failed: {err}");
            let failure = FailureRecord::new(
                FAILURE_KIND_INPUT_STREAM,
                "<watch_loop>",
                clock.now(),
                err.to_string(),
            );

            if let Err(record_err) = state.record_failure(&failure) {
                tracing::warn!(error = %record_err, "failed to persist input stream failure");
                model.status_line =
                    format!("input stream failed: {err} | failed to persist error: {record_err}");
            } else {
                model.latest_failure = Some(failure);
            }

            LoopControl::Redraw
        }
        None => LoopControl::Quit,
    }
}

fn enabled_repository_names(config: &Config) -> Vec<String> {
    config
        .repositories
        .iter()
        .filter(|repo| repo.enabled)
        .map(|repo| repo.name.clone())
        .collect()
}

fn apply_poll_result<S, K>(result: Result<PollOutcome>, model: &mut TuiModel, state: &S, clock: &K)
where
    S: StateStorePort,
    K: ClockPort,
{
    match result {
        Ok(outcome) => {
            if !outcome.timeline_events.is_empty() {
                model.push_timeline(outcome.timeline_events);
            }
            if outcome.repo_errors.is_empty() {
                model.status_line = format!("ok (new={})", outcome.notified_count);
                model.last_success_at = Some(clock.now());
            } else {
                model.status_line = format!(
                    "partial errors={} (new={})",
                    outcome.repo_errors.len(),
                    outcome.notified_count
                );
                model.failure_count += outcome.repo_errors.len() as u64;
            }
            if let Some(last_failure) = outcome.failures.last().cloned() {
                model.latest_failure = Some(last_failure);
            }
        }
        Err(err) => {
            model.failure_count += 1;
            model.status_line = format!("poll failed: {err}");
            let failure = FailureRecord::new(
                FAILURE_KIND_POLL_LOOP,
                "<watch_loop>",
                clock.now(),
                err.to_string(),
            );
            if let Err(record_err) = state.record_failure(&failure) {
                model.status_line =
                    format!("poll failed: {err} | failed to persist error: {record_err}");
            } else {
                model.latest_failure = Some(failure);
            }
        }
    }
}

pub async fn run_watch<C, S, N, K>(
    config: &Config,
    gh: &C,
    state: &S,
    notifier: &N,
    clock: &K,
) -> Result<()>
where
    C: GhClientPort,
    S: StateStorePort,
    N: NotifierPort,
    K: ClockPort,
{
    let mut ui = TerminalUi::new()?;
    let mut model = TuiModel::new(config.timeline_limit);
    model.watched_repositories = enabled_repository_names(config);
    model.timeline = state.load_timeline_events(config.timeline_limit)?;
    model.latest_failure = state.latest_failure()?;
    model.status_line = "ready".to_string();
    model.next_poll_at = Some(clock.now());
    ui.draw(&mut model)?;

    let mut interval = tokio::time::interval(Duration::from_secs(config.interval_seconds));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut spinner_interval =
        tokio::time::interval(Duration::from_millis(SPINNER_REDRAW_INTERVAL_MS));
    spinner_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    spinner_interval.tick().await;
    let mut reader = crossterm::event::EventStream::new();
    let mut poll_state = PollExecutionState::default();
    let mut in_flight_poll: Option<PollFuture<'_>> = None;
    poll_state.request_poll();

    loop {
        if poll_state.start_poll() {
            model.is_polling = poll_state.in_flight();
            model.poll_started_at = Some(clock.now());
            model.queued_refresh = poll_state.queued_refresh();
            model.status_line = "polling".to_string();
            ui.draw(&mut model)?;
            in_flight_poll = Some(Box::pin(poll_once(config, gh, state, notifier, clock)));
        }

        tokio::select! {
            _ = interval.tick() => {
                poll_state.request_poll();
                model.queued_refresh = poll_state.queued_refresh();
                if model.is_polling {
                    ui.draw(&mut model)?;
                }
            }
            _ = spinner_interval.tick(), if model.is_polling => {
                ui.draw(&mut model)?;
            }
            poll_result = async {
                match in_flight_poll.as_mut() {
                    Some(fut) => Some(fut.await),
                    None => None,
                }
            }, if in_flight_poll.is_some() => {
                let result = poll_result.expect("poll future must exist when branch is active");
                in_flight_poll = None;

                apply_poll_result(result, &mut model, state, clock);
                let queued_for_immediate_next = poll_state.finish_poll_and_take_next_request();

                model.is_polling = poll_state.in_flight();
                model.poll_started_at = None;
                model.queued_refresh = poll_state.queued_refresh();
                model.next_poll_at =
                    Some(clock.now() + chrono::Duration::seconds(config.interval_seconds as i64));

                if queued_for_immediate_next {
                    model.status_line = format!("{} | queued refresh", model.status_line);
                }

                ui.draw(&mut model)?;
            }
            maybe_event = reader.next() => {
                let terminal_area = ui.terminal_area().unwrap_or_default();
                match handle_stream_event(
                    maybe_event,
                    &mut model,
                    state,
                    clock,
                    terminal_area,
                    &open_url_in_browser,
                ) {
                    LoopControl::Quit => break,
                    LoopControl::RequestPoll => {
                        poll_state.request_poll();
                        model.queued_refresh = poll_state.queued_refresh();
                        if model.is_polling {
                            ui.draw(&mut model)?;
                        }
                    }
                    LoopControl::Redraw => {
                        ui.draw(&mut model)?;
                    }
                    LoopControl::Continue => {}
                }
            }
        }
    }

    Ok(())
}

fn open_url_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let ok = Command::new("open")
            .arg(url)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }

        return Err(anyhow!("failed to open URL with open: {url}"));
    }

    #[cfg(target_os = "linux")]
    {
        let ok = Command::new("xdg-open")
            .arg(url)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }

        return Err(anyhow!("failed to open URL with xdg-open: {url}"));
    }

    #[cfg(target_os = "windows")]
    {
        let ok = Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(url)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }

        return Err(anyhow!("failed to open URL with start: {url}"));
    }

    #[allow(unreachable_code)]
    Err(anyhow!("unsupported OS for opening URLs"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use anyhow::{anyhow, Result};
    use chrono::{TimeZone, Utc};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    use super::{enabled_repository_names, handle_stream_event, LoopControl, PollExecutionState};
    use crate::{
        config::{Config, NotificationConfig, PollConfig, RepositoryConfig},
        domain::{
            events::{EventKind, WatchEvent},
            failure::FailureRecord,
        },
        ports::{ClockPort, StateStorePort},
        ui::tui::TuiModel,
    };

    #[derive(Clone, Default)]
    struct FakeState {
        failures: Arc<Mutex<Vec<FailureRecord>>>,
        fail_record_failure: Arc<Mutex<bool>>,
    }

    impl FakeState {
        fn set_record_failure_error(&self, should_fail: bool) {
            *self.fail_record_failure.lock().unwrap() = should_fail;
        }
    }

    impl StateStorePort for FakeState {
        fn get_cursor(&self, _repo: &str) -> Result<Option<chrono::DateTime<Utc>>> {
            Ok(None)
        }

        fn set_cursor(&self, _repo: &str, _at: chrono::DateTime<Utc>) -> Result<()> {
            Ok(())
        }

        fn is_event_notified(&self, _event_key: &str) -> Result<bool> {
            Ok(false)
        }

        fn record_notified_event(
            &self,
            _event: &WatchEvent,
            _notified_at: chrono::DateTime<Utc>,
        ) -> Result<()> {
            Ok(())
        }

        fn record_failure(&self, failure: &FailureRecord) -> Result<()> {
            if *self.fail_record_failure.lock().unwrap() {
                return Err(anyhow!("state store down"));
            }
            self.failures.lock().unwrap().push(failure.clone());
            Ok(())
        }

        fn latest_failure(&self) -> Result<Option<FailureRecord>> {
            Ok(self.failures.lock().unwrap().last().cloned())
        }

        fn append_timeline_event(&self, _event: &WatchEvent) -> Result<()> {
            Ok(())
        }

        fn load_timeline_events(&self, _limit: usize) -> Result<Vec<WatchEvent>> {
            Ok(Vec::new())
        }

        fn cleanup_old(
            &self,
            _retention_days: u32,
            _failure_history_limit: usize,
            _now: chrono::DateTime<Utc>,
        ) -> Result<()> {
            Ok(())
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

    fn open_ok(_url: &str) -> Result<()> {
        Ok(())
    }

    #[test]
    fn stream_error_is_recorded_for_traceability() {
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
        assert!(model.status_line.contains("event stream disconnected"));

        let failure = state.latest_failure().unwrap().unwrap();
        assert_eq!(
            failure.kind,
            crate::domain::failure::FAILURE_KIND_INPUT_STREAM
        );
        assert_eq!(failure.repo, "<watch_loop>");
        assert_eq!(failure.failed_at, clock.now);
        assert!(failure.message.contains("event stream disconnected"));
    }

    #[test]
    fn stream_error_persistence_failure_keeps_root_cause_visible() {
        let state = FakeState::default();
        state.set_record_failure_error(true);
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 8, 0, 0, 0).unwrap(),
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
        assert!(model.status_line.contains("event stream disconnected"));
        assert!(model
            .status_line
            .contains("failed to persist error: state store down"));
        assert!(model.latest_failure.is_none());
        assert!(state.latest_failure().unwrap().is_none());
    }

    #[test]
    fn enabled_repository_names_keeps_config_order_and_filters_disabled() {
        let config = Config {
            interval_seconds: 300,
            timeline_limit: 500,
            retention_days: 90,
            failure_history_limit: 200,
            state_db_path: None,
            repositories: vec![
                RepositoryConfig {
                    name: "acme/one".to_string(),
                    enabled: true,
                },
                RepositoryConfig {
                    name: "acme/two".to_string(),
                    enabled: false,
                },
                RepositoryConfig {
                    name: "acme/three".to_string(),
                    enabled: true,
                },
            ],
            notifications: NotificationConfig::default(),
            poll: PollConfig::default(),
        };

        let watched = enabled_repository_names(&config);

        assert_eq!(
            watched,
            vec!["acme/one".to_string(), "acme/three".to_string()]
        );
    }

    #[test]
    fn enter_opens_selected_url_and_requests_redraw() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 9, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.timeline = vec![WatchEvent {
            event_id: "ev1".to_string(),
            repo: "acme/api".to_string(),
            kind: EventKind::IssueCommentCreated,
            actor: "dev".to_string(),
            title: "comment".to_string(),
            url: "https://example.com/ev1".to_string(),
            created_at: clock.now,
            source_item_id: "ev1".to_string(),
        }];
        model.selected = 0;

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert!(model
            .status_line
            .contains("opened: https://example.com/ev1"));
    }

    #[test]
    fn enter_open_failure_updates_status_and_keeps_loop_alive() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 10, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.timeline = vec![WatchEvent {
            event_id: "ev2".to_string(),
            repo: "acme/api".to_string(),
            kind: EventKind::IssueCommentCreated,
            actor: "dev".to_string(),
            title: "comment".to_string(),
            url: "https://example.com/ev2".to_string(),
            created_at: clock.now,
            source_item_id: "ev2".to_string(),
        }];
        model.selected = 0;

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &|_| Err(anyhow!("browser unavailable")),
        );

        assert_eq!(control, LoopControl::Redraw);
        assert!(model
            .status_line
            .contains("open failed: browser unavailable"));
    }

    #[test]
    fn resize_event_requests_redraw() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 11, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);

        let control = handle_stream_event(
            Some(Ok(Event::Resize(100, 30))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
    }

    #[test]
    fn resize_event_does_not_mutate_navigation_state() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 12, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.status_line = "ready".to_string();
        model.selected = 3;
        model.timeline_offset = 2;

        let selected_before = model.selected;
        let offset_before = model.timeline_offset;
        let status_before = model.status_line.clone();

        let control = handle_stream_event(
            Some(Ok(Event::Resize(90, 20))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert_eq!(model.selected, selected_before);
        assert_eq!(model.timeline_offset, offset_before);
        assert_eq!(model.status_line, status_before);
    }

    #[test]
    fn help_toggle_requests_redraw_without_polling() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 13, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        assert!(!model.help_visible);

        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert!(model.help_visible);
    }

    #[test]
    fn page_down_key_updates_selection() {
        let state = FakeState::default();
        let clock = FixedClock {
            now: Utc.with_ymd_and_hms(2025, 1, 14, 0, 0, 0).unwrap(),
        };
        let mut model = TuiModel::new(10);
        model.timeline_page_size = 2;
        model.timeline = vec![
            WatchEvent {
                event_id: "ev1".to_string(),
                repo: "acme/api".to_string(),
                kind: EventKind::IssueCommentCreated,
                actor: "dev".to_string(),
                title: "comment".to_string(),
                url: "https://example.com/ev1".to_string(),
                created_at: clock.now,
                source_item_id: "ev1".to_string(),
            },
            WatchEvent {
                event_id: "ev2".to_string(),
                repo: "acme/api".to_string(),
                kind: EventKind::IssueCommentCreated,
                actor: "dev".to_string(),
                title: "comment".to_string(),
                url: "https://example.com/ev2".to_string(),
                created_at: clock.now,
                source_item_id: "ev2".to_string(),
            },
            WatchEvent {
                event_id: "ev3".to_string(),
                repo: "acme/api".to_string(),
                kind: EventKind::IssueCommentCreated,
                actor: "dev".to_string(),
                title: "comment".to_string(),
                url: "https://example.com/ev3".to_string(),
                created_at: clock.now,
                source_item_id: "ev3".to_string(),
            },
        ];

        let key = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
        let control = handle_stream_event(
            Some(Ok(Event::Key(key))),
            &mut model,
            &state,
            &clock,
            test_area(),
            &open_ok,
        );

        assert_eq!(control, LoopControl::Redraw);
        assert_eq!(model.selected, 2);
    }

    #[test]
    fn refresh_requested_while_polling_is_queued_without_parallel_start() {
        let mut state = PollExecutionState::default();
        assert!(state.request_poll());
        assert!(state.start_poll());
        assert!(!state.request_poll());
        assert!(state.queued_refresh());
        assert!(state.in_flight());
    }

    #[test]
    fn queued_refresh_is_consumed_exactly_once_after_poll_completion() {
        let mut state = PollExecutionState::default();
        assert!(state.request_poll());
        assert!(state.start_poll());
        assert!(!state.request_poll());

        assert!(state.finish_poll_and_take_next_request());
        assert!(!state.finish_poll_and_take_next_request());
        assert!(!state.queued_refresh());
        assert!(!state.in_flight());
    }
}
