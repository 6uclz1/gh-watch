use anyhow::Result;
use crossterm::event::Event;
use ratatui::layout::Rect;

use crate::{
    ports::{ClockPort, StateStorePort},
    ui::tui::{handle_input, parse_input, parse_mouse_input, ActiveTab, InputCommand, TuiModel},
};

const ESC_DOUBLE_PRESS_WINDOW_MS: i64 = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoopControl {
    Continue,
    RequestPoll,
    Redraw,
    Quit,
}

pub(super) fn handle_stream_event<S, K>(
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
            if cmd != InputCommand::EscapePressed {
                model.esc_armed_until = None;
            }

            match cmd {
                InputCommand::Quit => LoopControl::Quit,
                InputCommand::EscapePressed => {
                    let now = clock.now();
                    if model
                        .esc_armed_until
                        .is_some_and(|armed_until| now <= armed_until)
                    {
                        LoopControl::Quit
                    } else {
                        model.esc_armed_until =
                            Some(now + chrono::Duration::milliseconds(ESC_DOUBLE_PRESS_WINDOW_MS));
                        model.status_line = "press Esc again to quit (1.5s)".to_string();
                        LoopControl::Redraw
                    }
                }
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
                    mark_selected_event_read(model, state, clock);
                    LoopControl::Redraw
                }
                InputCommand::ToggleHelp | InputCommand::NextTab | InputCommand::PrevTab => {
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
                    if model.active_tab == ActiveTab::Timeline {
                        mark_selected_event_read(model, state, clock);
                    }
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
                    if model.active_tab == ActiveTab::Timeline {
                        mark_selected_event_read(model, state, clock);
                    }
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
            LoopControl::Redraw
        }
        None => LoopControl::Quit,
    }
}

pub(super) fn mark_selected_event_read<S, K>(model: &mut TuiModel, state: &S, clock: &K)
where
    S: StateStorePort,
    K: ClockPort,
{
    let Some(event_key) = model
        .timeline
        .get(model.selected)
        .map(|event| event.event_key())
    else {
        return;
    };

    if model.is_event_read(&event_key) {
        return;
    }

    if let Err(err) = state.mark_timeline_event_read(&event_key, clock.now()) {
        tracing::warn!(error = %err, event_key = %event_key, "failed to persist read state");
        model.status_line = format!("read mark failed: {err}");
        return;
    }

    model.mark_event_read(&event_key);
}

#[cfg(test)]
mod tests;
