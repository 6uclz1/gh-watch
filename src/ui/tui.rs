use std::io::{stdout, Stdout};

use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEvent, MouseButton, MouseEvent,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};

use crate::domain::{events::WatchEvent, failure::FailureRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputCommand {
    ScrollUp,
    ScrollDown,
    SelectIndex(usize),
    Refresh,
    OpenSelectedUrl,
    Quit,
    None,
}

#[derive(Debug, Clone)]
pub struct TuiModel {
    pub timeline: Vec<WatchEvent>,
    pub watched_repositories: Vec<String>,
    pub selected: usize,
    pub timeline_offset: usize,
    pub status_line: String,
    pub failure_count: u64,
    pub latest_failure: Option<FailureRecord>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
    limit: usize,
}

impl TuiModel {
    pub fn new(limit: usize) -> Self {
        Self {
            timeline: Vec::new(),
            watched_repositories: Vec::new(),
            selected: 0,
            timeline_offset: 0,
            status_line: "starting".to_string(),
            failure_count: 0,
            latest_failure: None,
            last_success_at: None,
            next_poll_at: None,
            limit,
        }
    }

    pub fn push_timeline(&mut self, mut events: Vec<WatchEvent>) {
        self.timeline.append(&mut events);
        self.timeline
            .sort_by(|a, b| b.created_at.cmp(&a.created_at));
        self.timeline
            .dedup_by(|a, b| a.event_key() == b.event_key());
        self.timeline.truncate(self.limit);
        if self.selected >= self.timeline.len() && !self.timeline.is_empty() {
            self.selected = self.timeline.len() - 1;
        }
    }
}

pub fn parse_input(key: KeyEvent) -> InputCommand {
    match key.code {
        KeyCode::Char('q') => InputCommand::Quit,
        KeyCode::Char('r') => InputCommand::Refresh,
        KeyCode::Enter => InputCommand::OpenSelectedUrl,
        KeyCode::Up => InputCommand::ScrollUp,
        KeyCode::Down => InputCommand::ScrollDown,
        _ => InputCommand::None,
    }
}

pub fn parse_mouse_input(mouse: MouseEvent, terminal_area: Rect, model: &TuiModel) -> InputCommand {
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
            let index = model.timeline_offset + (row / 2);
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
        InputCommand::ScrollUp => {
            model.selected = model.selected.saturating_sub(1);
        }
        InputCommand::ScrollDown => {
            if !model.timeline.is_empty() {
                model.selected = (model.selected + 1).min(model.timeline.len() - 1);
            }
        }
        InputCommand::SelectIndex(index) => {
            if !model.timeline.is_empty() {
                model.selected = index.min(model.timeline.len() - 1);
            }
        }
        _ => {}
    }
}

pub struct TerminalUi {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalUi {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(out);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    pub fn draw(&mut self, model: &mut TuiModel) -> Result<()> {
        self.terminal.draw(|frame| render(frame, model))?;
        Ok(())
    }

    pub fn terminal_area(&self) -> Result<Rect> {
        let size = self.terminal.size()?;
        Ok(Rect::new(0, 0, size.width, size.height))
    }
}

impl Drop for TerminalUi {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

fn render(frame: &mut Frame<'_>, model: &mut TuiModel) {
    let vertical_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let main_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(vertical_areas[1]);

    let last_success = model
        .last_success_at
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "-".to_string());
    let next_poll = model
        .next_poll_at
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "-".to_string());
    let latest_failure = model
        .latest_failure
        .as_ref()
        .map(summarize_failure)
        .unwrap_or_else(|| "-".to_string());

    let header = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "gh-watch",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!(
            "status={} | last_success={} | next_poll={} | failures={}",
            model.status_line, last_success, next_poll, model.failure_count
        )),
        Line::from(format!("latest_failure={latest_failure}")),
    ])
    .block(Block::default().borders(Borders::ALL).title("Status"));
    frame.render_widget(header, vertical_areas[0]);

    let items = if model.timeline.is_empty() {
        vec![ListItem::new("No events yet")]
    } else {
        model
            .timeline
            .iter()
            .map(|event| {
                ListItem::new(vec![
                    Line::from(format!(
                        "[{}] [{}] [{}] @{}",
                        event.created_at.to_rfc3339(),
                        event.kind,
                        event.repo,
                        event.actor
                    )),
                    Line::from(event.title.clone()),
                ])
            })
            .collect()
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Timeline"))
        .highlight_style(Style::default().bg(Color::DarkGray));

    let mut state = ListState::default().with_offset(model.timeline_offset);
    if model.timeline.is_empty() {
        model.selected = 0;
        model.timeline_offset = 0;
    } else {
        model.selected = model.selected.min(model.timeline.len() - 1);
        state.select(Some(model.selected));
    }
    frame.render_stateful_widget(list, main_areas[0], &mut state);
    model.timeline_offset = if model.timeline.is_empty() {
        0
    } else {
        state.offset()
    };

    let repo_items = if model.watched_repositories.is_empty() {
        vec![ListItem::new("No enabled repositories")]
    } else {
        model
            .watched_repositories
            .iter()
            .map(|repo| ListItem::new(repo.clone()))
            .collect()
    };
    let repo_list = List::new(repo_items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Watching Repositories"),
    );
    frame.render_widget(repo_list, main_areas[1]);

    let footer_text = model
        .timeline
        .get(model.selected)
        .map(|ev| {
            format!(
                "q: quit | r: refresh | up/down: move | enter: open | mouse: click/wheel | {}",
                ev.url
            )
        })
        .unwrap_or_else(|| {
            "q: quit | r: refresh | up/down: move | enter: open | mouse: click/wheel".to_string()
        });

    let footer =
        Paragraph::new(footer_text).block(Block::default().borders(Borders::ALL).title("Keys"));
    frame.render_widget(footer, vertical_areas[2]);
}

fn summarize_failure(failure: &FailureRecord) -> String {
    let msg = failure.message.replace('\n', " ");
    let mut clipped = msg.chars().take(96).collect::<String>();
    if msg.chars().count() > 96 {
        clipped.push_str("...");
    }
    format!(
        "{} [{}:{}] {}",
        failure.failed_at.to_rfc3339(),
        failure.kind,
        failure.repo,
        clipped
    )
}

fn timeline_inner_area(area: Rect) -> Rect {
    let vertical_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);
    let main_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(vertical_areas[1]);

    shrink_by_border(main_areas[0])
}

fn shrink_by_border(area: Rect) -> Rect {
    if area.width <= 2 || area.height <= 2 {
        return Rect::new(area.x, area.y, 0, 0);
    }

    Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2)
}

fn contains_point(area: Rect, x: u16, y: u16) -> bool {
    if area.width == 0 || area.height == 0 {
        return false;
    }

    let x_end = area.x.saturating_add(area.width);
    let y_end = area.y.saturating_add(area.height);
    x >= area.x && x < x_end && y >= area.y && y < y_end
}
