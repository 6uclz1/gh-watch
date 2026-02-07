use std::time::Duration;

use anyhow::Result;
use crossterm::event::Event;
use futures_util::StreamExt;
use tokio::time::MissedTickBehavior;

use crate::{
    app::poll_once::poll_once,
    config::Config,
    domain::failure::{FailureRecord, FAILURE_KIND_INPUT_STREAM, FAILURE_KIND_POLL_LOOP},
    ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort},
    ui::tui::{handle_input, parse_input, InputCommand, TerminalUi, TuiModel},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Continue,
    RequestPoll,
    Redraw,
    Quit,
}

fn handle_stream_event<S, K>(
    maybe_event: Option<Result<Event, std::io::Error>>,
    model: &mut TuiModel,
    state: &S,
    clock: &K,
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
                InputCommand::ScrollUp | InputCommand::ScrollDown => {
                    handle_input(model, cmd);
                    LoopControl::Redraw
                }
                InputCommand::None => LoopControl::Continue,
            }
        }
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
    model.timeline = state.load_timeline_events(config.timeline_limit)?;
    model.latest_failure = state.latest_failure()?;
    model.status_line = "ready".to_string();
    model.next_poll_at = Some(clock.now());
    ui.draw(&model)?;

    let mut interval = tokio::time::interval(Duration::from_secs(config.interval_seconds));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut reader = crossterm::event::EventStream::new();
    let mut force_poll = true;

    loop {
        if force_poll {
            model.status_line = "polling".to_string();
            ui.draw(&model)?;

            match poll_once(config, gh, state, notifier, clock).await {
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

            model.next_poll_at =
                Some(clock.now() + chrono::Duration::seconds(config.interval_seconds as i64));
            ui.draw(&model)?;
            force_poll = false;
        }

        tokio::select! {
            _ = interval.tick() => {
                force_poll = true;
            }
            maybe_event = reader.next() => {
                match handle_stream_event(maybe_event, &mut model, state, clock) {
                    LoopControl::Quit => break,
                    LoopControl::RequestPoll => {
                        force_poll = true;
                    }
                    LoopControl::Redraw => {
                        ui.draw(&model)?;
                    }
                    LoopControl::Continue => {}
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use anyhow::{anyhow, Result};
    use chrono::{TimeZone, Utc};

    use super::{handle_stream_event, LoopControl};
    use crate::{
        domain::{events::WatchEvent, failure::FailureRecord},
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
}
