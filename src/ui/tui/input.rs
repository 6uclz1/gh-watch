use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::{
    layout::{contains_point, timeline_inner_area},
    model::TuiModel,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputCommand {
    ScrollUp,
    ScrollDown,
    SelectIndex(usize),
    PageUp,
    PageDown,
    JumpTop,
    JumpBottom,
    ToggleHelp,
    Refresh,
    OpenSelectedUrl,
    NextTab,
    PrevTab,
    EscapePressed,
    Quit,
    None,
}

pub fn parse_input(key: KeyEvent) -> InputCommand {
    match key.code {
        KeyCode::Char('q') => InputCommand::Quit,
        KeyCode::Char('r') => InputCommand::Refresh,
        KeyCode::Char('?') => InputCommand::ToggleHelp,
        KeyCode::Tab => InputCommand::NextTab,
        KeyCode::BackTab => InputCommand::PrevTab,
        KeyCode::Esc => InputCommand::EscapePressed,
        KeyCode::Enter => InputCommand::OpenSelectedUrl,
        KeyCode::Up | KeyCode::Char('k') => InputCommand::ScrollUp,
        KeyCode::Down | KeyCode::Char('j') => InputCommand::ScrollDown,
        KeyCode::PageUp => InputCommand::PageUp,
        KeyCode::PageDown => InputCommand::PageDown,
        KeyCode::Home | KeyCode::Char('g') => InputCommand::JumpTop,
        KeyCode::End | KeyCode::Char('G') => InputCommand::JumpBottom,
        _ => InputCommand::None,
    }
}

pub fn parse_mouse_input(mouse: MouseEvent, terminal_area: Rect, model: &TuiModel) -> InputCommand {
    if !model.active_tab.supports_timeline_navigation() {
        return InputCommand::None;
    }

    let timeline_inner = timeline_inner_area(terminal_area);
    if !contains_point(timeline_inner, mouse.column, mouse.row) {
        return InputCommand::None;
    }

    match mouse.kind {
        MouseEventKind::ScrollUp => InputCommand::ScrollUp,
        MouseEventKind::ScrollDown => InputCommand::ScrollDown,
        MouseEventKind::Down(MouseButton::Left) => {
            if model.timeline.is_empty() {
                return InputCommand::None;
            }

            let row = mouse.row.saturating_sub(timeline_inner.y) as usize;
            if row == 0 {
                return InputCommand::None;
            }

            let index = model.timeline_offset + row.saturating_sub(1);
            if index < model.timeline.len() {
                InputCommand::SelectIndex(index)
            } else {
                InputCommand::None
            }
        }
        _ => InputCommand::None,
    }
}

pub fn handle_input(model: &mut TuiModel, command: InputCommand) {
    match command {
        InputCommand::ToggleHelp => {
            model.help_visible = !model.help_visible;
        }
        InputCommand::NextTab => {
            model.set_active_tab(model.active_tab.next());
        }
        InputCommand::PrevTab => {
            model.set_active_tab(model.active_tab.prev());
        }
        InputCommand::ScrollUp => {
            if model.active_tab.supports_timeline_navigation() {
                model.selected = model.selected.saturating_sub(1);
            }
        }
        InputCommand::ScrollDown => {
            if model.active_tab.supports_timeline_navigation() && !model.timeline.is_empty() {
                model.selected = (model.selected + 1).min(model.timeline.len() - 1);
            }
        }
        InputCommand::PageUp => {
            if model.active_tab.supports_timeline_navigation() && !model.timeline.is_empty() {
                model.selected = model.selected.saturating_sub(model.page_size());
            }
        }
        InputCommand::PageDown => {
            if model.active_tab.supports_timeline_navigation() && !model.timeline.is_empty() {
                model.selected = (model.selected + model.page_size()).min(model.timeline.len() - 1);
            }
        }
        InputCommand::JumpTop => {
            if model.active_tab.supports_timeline_navigation() && !model.timeline.is_empty() {
                model.selected = 0;
            }
        }
        InputCommand::JumpBottom => {
            if model.active_tab.supports_timeline_navigation() && !model.timeline.is_empty() {
                model.selected = model.timeline.len() - 1;
            }
        }
        InputCommand::SelectIndex(index) => {
            if model.active_tab.supports_timeline_navigation() && !model.timeline.is_empty() {
                model.selected = index.min(model.timeline.len() - 1);
            }
        }
        _ => {}
    }

    if matches!(
        command,
        InputCommand::ScrollUp
            | InputCommand::ScrollDown
            | InputCommand::PageUp
            | InputCommand::PageDown
            | InputCommand::JumpTop
            | InputCommand::JumpBottom
            | InputCommand::SelectIndex(_)
    ) && model.active_tab.supports_timeline_navigation()
    {
        model.sync_selected_event_key();
    }
}
