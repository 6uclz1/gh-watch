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
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState, Wrap,
    },
    Frame, Terminal,
};

use crate::domain::{
    events::{EventKind, WatchEvent},
    failure::FailureRecord,
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
    Quit,
    None,
}

#[derive(Debug, Clone)]
pub struct TuiModel {
    pub timeline: Vec<WatchEvent>,
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
    limit: usize,
}

impl TuiModel {
    pub fn new(limit: usize) -> Self {
        Self {
            timeline: Vec::new(),
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
            limit,
        }
    }

    pub fn push_timeline(&mut self, mut events: Vec<WatchEvent>) {
        let previous_selected_key = self
            .selected_event_key
            .clone()
            .or_else(|| self.timeline.get(self.selected).map(WatchEvent::event_key));

        self.timeline.append(&mut events);
        self.timeline
            .sort_by(|a, b| b.created_at.cmp(&a.created_at));
        self.timeline
            .dedup_by(|a, b| a.event_key() == b.event_key());
        self.timeline.truncate(self.limit);

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

    fn sync_selected_event_key(&mut self) {
        self.selected_event_key = self.timeline.get(self.selected).map(WatchEvent::event_key);
    }

    fn page_size(&self) -> usize {
        self.timeline_page_size.max(1)
    }
}

pub fn parse_input(key: KeyEvent) -> InputCommand {
    match key.code {
        KeyCode::Char('q') => InputCommand::Quit,
        KeyCode::Char('r') => InputCommand::Refresh,
        KeyCode::Char('?') => InputCommand::ToggleHelp,
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
        InputCommand::ScrollUp => {
            model.selected = model.selected.saturating_sub(1);
        }
        InputCommand::ScrollDown => {
            if !model.timeline.is_empty() {
                model.selected = (model.selected + 1).min(model.timeline.len() - 1);
            }
        }
        InputCommand::PageUp => {
            if !model.timeline.is_empty() {
                model.selected = model.selected.saturating_sub(model.page_size());
            }
        }
        InputCommand::PageDown => {
            if !model.timeline.is_empty() {
                model.selected = (model.selected + model.page_size()).min(model.timeline.len() - 1);
            }
        }
        InputCommand::JumpTop => {
            if !model.timeline.is_empty() {
                model.selected = 0;
            }
        }
        InputCommand::JumpBottom => {
            if !model.timeline.is_empty() {
                model.selected = model.timeline.len() - 1;
            }
        }
        InputCommand::SelectIndex(index) => {
            if !model.timeline.is_empty() {
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
    ) {
        model.sync_selected_event_key();
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
            Constraint::Length(7),
            Constraint::Min(5),
            Constraint::Length(5),
            Constraint::Length(3),
        ])
        .split(frame.area());
    let main_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(vertical_areas[1]);
    let timeline_inner = shrink_by_border(main_areas[0]);
    model.timeline_page_size = (timeline_inner.height as usize).saturating_sub(1).max(1);

    let last_success = format_status_time(model.last_success_at);
    let next_poll = format_status_time(model.next_poll_at);
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
            "status={} | failures={}",
            model.status_line, model.failure_count
        )),
        Line::from(build_loading_status_line(model, Utc::now())),
        Line::from(format!(
            "last_success={} | next_poll={}",
            last_success, next_poll
        )),
        Line::from(format!("latest_failure={latest_failure}")),
    ])
    .block(Block::default().borders(Borders::ALL).title("Status"));
    frame.render_widget(header, vertical_areas[0]);

    let rows = if model.timeline.is_empty() {
        vec![Row::new(vec![
            Cell::from("-"),
            Cell::from("-"),
            Cell::from("-"),
            Cell::from("-"),
            Cell::from("No events yet"),
        ])]
    } else {
        model.timeline.iter().map(timeline_row).collect()
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(22),
            Constraint::Length(16),
            Constraint::Min(12),
        ],
    )
    .header(
        Row::new(vec!["Time(UTC)", "Type", "Repo", "Actor", "Title"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title("Timeline"))
    .row_highlight_style(Style::default().bg(Color::DarkGray))
    .highlight_symbol(">> ");

    let mut state = TableState::default().with_offset(model.timeline_offset);
    if model.timeline.is_empty() {
        model.selected = 0;
        model.timeline_offset = 0;
        model.selected_event_key = None;
    } else {
        model.selected = model.selected.min(model.timeline.len() - 1);
        model.sync_selected_event_key();
        state.select(Some(model.selected));
    }
    frame.render_stateful_widget(table, main_areas[0], &mut state);
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

    let selected_text = model
        .timeline
        .get(model.selected)
        .map(|ev| {
            vec![
                Line::from(vec![
                    Span::styled(event_kind_label(&ev.kind), event_kind_style(&ev.kind)),
                    Span::raw(format!(" {}", truncate_tail(&ev.title, 72))),
                ]),
                Line::from(format!(
                    "repo={} | actor=@{} | at={}",
                    ev.repo,
                    ev.actor,
                    format_status_time(Some(ev.created_at))
                )),
                Line::from(ev.url.clone()),
            ]
        })
        .unwrap_or_else(|| {
            vec![
                Line::from("No selected event"),
                Line::from("-"),
                Line::from("-"),
            ]
        });
    let selected = Paragraph::new(selected_text)
        .block(Block::default().borders(Borders::ALL).title("Selected"));
    frame.render_widget(selected, vertical_areas[2]);

    let loading_hint = if model.is_polling {
        "loading: move/help/open available, r queues next refresh"
    } else {
        "loading: idle"
    };
    let keys = Paragraph::new(vec![
        Line::from(
            "q quit | r refresh | ? help | ↑/↓ or j/k move | PgUp/PgDn page | g/G top/bottom | Enter open",
        ),
        Line::from(loading_hint),
    ])
    .block(Block::default().borders(Borders::ALL).title("Keys"));
    frame.render_widget(keys, vertical_areas[3]);

    if model.help_visible {
        render_help_overlay(frame);
    }
}

fn format_status_time(dt: Option<DateTime<Utc>>) -> String {
    dt.map(|d| d.format("%Y-%m-%d %H:%M:%SZ").to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn format_timeline_time(dt: DateTime<Utc>) -> String {
    dt.format("%m-%d %H:%M:%S").to_string()
}

fn event_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::PrCreated => "PR",
        EventKind::IssueCreated => "ISSUE",
        EventKind::IssueCommentCreated => "I-CMT",
        EventKind::PrReviewCommentCreated => "PR-CMT",
    }
}

fn event_kind_style(kind: &EventKind) -> Style {
    match kind {
        EventKind::PrCreated => Style::default().fg(Color::Cyan),
        EventKind::IssueCreated => Style::default().fg(Color::Yellow),
        EventKind::IssueCommentCreated => Style::default().fg(Color::Green),
        EventKind::PrReviewCommentCreated => Style::default().fg(Color::Magenta),
    }
}

fn timeline_row(event: &WatchEvent) -> Row<'static> {
    Row::new(vec![
        Cell::from(format_timeline_time(event.created_at)),
        Cell::from(Span::styled(
            event_kind_label(&event.kind),
            event_kind_style(&event.kind),
        )),
        Cell::from(truncate_tail(&event.repo, 22)),
        Cell::from(truncate_tail(&format!("@{}", event.actor), 16)),
        Cell::from(truncate_tail(&event.title, 96)),
    ])
}

fn truncate_tail(raw: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let char_count = raw.chars().count();
    if char_count <= max_chars {
        return raw.to_string();
    }

    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let mut clipped = raw.chars().take(max_chars - 3).collect::<String>();
    clipped.push_str("...");
    clipped
}

fn build_loading_status_line(model: &TuiModel, now: DateTime<Utc>) -> String {
    if !model.is_polling {
        return "loading=off | refresh=none".to_string();
    }

    let elapsed_secs = model
        .poll_started_at
        .map(|started| (now - started).num_seconds().max(0))
        .unwrap_or(0);
    let refresh = if model.queued_refresh {
        "queued"
    } else {
        "none"
    };

    format!("loading=on {}s | refresh={refresh}", elapsed_secs)
}

fn summarize_failure(failure: &FailureRecord) -> String {
    let msg = failure.message.replace('\n', " ");
    let mut clipped = msg.chars().take(96).collect::<String>();
    if msg.chars().count() > 96 {
        clipped.push_str("...");
    }
    format!(
        "{} [{}:{}] {}",
        format_status_time(Some(failure.failed_at)),
        failure.kind,
        failure.repo,
        clipped
    )
}

fn timeline_inner_area(area: Rect) -> Rect {
    let vertical_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(5),
            Constraint::Length(5),
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

fn render_help_overlay(frame: &mut Frame<'_>) {
    let area = centered_rect(frame.area(), 80, 70);
    frame.render_widget(Clear, area);

    let help = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "Keyboard",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from("q: quit, r: refresh, ?: toggle help"),
        Line::from("up/down or j/k: move one row"),
        Line::from("page up/page down: move one page"),
        Line::from("g/home: top, G/end: bottom"),
        Line::from("enter: open selected URL"),
        Line::from("mouse: click to select, wheel to scroll"),
    ])
    .block(Block::default().borders(Borders::ALL).title("Help"))
    .wrap(Wrap { trim: true });

    frame.render_widget(help, area);
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(popup[1])[1]
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::{build_loading_status_line, truncate_tail, TuiModel};

    #[test]
    fn truncate_tail_adds_ellipsis_for_long_values() {
        assert_eq!(truncate_tail("abcdef", 5), "ab...");
        assert_eq!(truncate_tail("abc", 5), "abc");
        assert_eq!(truncate_tail("abc", 2), "..");
    }

    #[test]
    fn loading_status_line_reflects_queue_state() {
        let started_at = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 5).unwrap();

        let mut model = TuiModel::new(10);
        model.is_polling = true;
        model.poll_started_at = Some(started_at);
        model.queued_refresh = true;

        assert_eq!(
            build_loading_status_line(&model, now),
            "loading=on 5s | refresh=queued"
        );
    }
}
