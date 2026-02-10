use std::{future::Future, pin::Pin, time::Duration};

use anyhow::Result;
use futures_util::StreamExt;
use tokio::time::MissedTickBehavior;

use crate::{
    app::poll_once::{poll_once, PollOutcome},
    config::Config,
    ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort},
    ui::tui::{TerminalUi, TuiModel},
};

mod browser;
mod poll_result;
mod poll_state;
mod stream_controller;

use browser::open_url_in_browser;
use poll_result::{apply_poll_result, enabled_repository_names};
use poll_state::PollExecutionState;
use stream_controller::{handle_stream_event, LoopControl};

const SPINNER_REDRAW_INTERVAL_MS: u64 = 120;

type PollFuture<'a> = Pin<Box<dyn Future<Output = Result<PollOutcome>> + 'a>>;

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
    let timeline = state.load_timeline_events(config.timeline_limit)?;
    let timeline_keys = timeline
        .iter()
        .map(|event| event.event_key())
        .collect::<Vec<_>>();
    let read_event_keys = state.load_read_event_keys(&timeline_keys)?;
    model.replace_timeline(timeline);
    model.replace_read_event_keys(read_event_keys);
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

                apply_poll_result(result, &mut model, clock);
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
