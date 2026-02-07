use std::time::Duration;

use anyhow::Result;
use crossterm::event::Event;
use futures_util::StreamExt;
use tokio::time::MissedTickBehavior;

use crate::{
    app::poll_once::poll_once,
    config::Config,
    ports::{ClockPort, GhClientPort, NotifierPort, StateStorePort},
    ui::tui::{handle_input, parse_input, InputCommand, TerminalUi, TuiModel},
};

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
                }
                Err(err) => {
                    model.failure_count += 1;
                    model.status_line = format!("poll failed: {err}");
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
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        let cmd = parse_input(key);
                        match cmd {
                            InputCommand::Quit => break,
                            InputCommand::Refresh => {
                                force_poll = true;
                            }
                            InputCommand::ScrollUp | InputCommand::ScrollDown => {
                                handle_input(&mut model, cmd);
                                ui.draw(&model)?;
                            }
                            InputCommand::None => {}
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => {}
                    None => break,
                }
            }
        }
    }

    Ok(())
}
