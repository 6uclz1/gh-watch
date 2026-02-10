use std::{
    collections::HashSet,
    io::{stdout, Stdout},
};

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
        Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState, Tabs, Wrap,
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
    NextTab,
    PrevTab,
    EscapePressed,
    Quit,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Timeline,
    Repositories,
}

impl ActiveTab {
    fn next(self) -> Self {
        match self {
            Self::Timeline => Self::Repositories,
            Self::Repositories => Self::Timeline,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Timeline => Self::Repositories,
            Self::Repositories => Self::Timeline,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Timeline => 0,
            Self::Repositories => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlyphMode {
    Nerd,
    Ascii,
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
    if model.active_tab != ActiveTab::Timeline {
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
            model.active_tab = model.active_tab.next();
        }
        InputCommand::PrevTab => {
            model.active_tab = model.active_tab.prev();
        }
        InputCommand::ScrollUp => {
            if model.active_tab == ActiveTab::Timeline {
                model.selected = model.selected.saturating_sub(1);
            }
        }
        InputCommand::ScrollDown => {
            if model.active_tab == ActiveTab::Timeline && !model.timeline.is_empty() {
                model.selected = (model.selected + 1).min(model.timeline.len() - 1);
            }
        }
        InputCommand::PageUp => {
            if model.active_tab == ActiveTab::Timeline && !model.timeline.is_empty() {
                model.selected = model.selected.saturating_sub(model.page_size());
            }
        }
        InputCommand::PageDown => {
            if model.active_tab == ActiveTab::Timeline && !model.timeline.is_empty() {
                model.selected = (model.selected + model.page_size()).min(model.timeline.len() - 1);
            }
        }
        InputCommand::JumpTop => {
            if model.active_tab == ActiveTab::Timeline && !model.timeline.is_empty() {
                model.selected = 0;
            }
        }
        InputCommand::JumpBottom => {
            if model.active_tab == ActiveTab::Timeline && !model.timeline.is_empty() {
                model.selected = model.timeline.len() - 1;
            }
        }
        InputCommand::SelectIndex(index) => {
            if model.active_tab == ActiveTab::Timeline && !model.timeline.is_empty() {
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
    ) && model.active_tab == ActiveTab::Timeline
    {
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

#[derive(Debug, Clone, Copy)]
struct UiLayout {
    status: Rect,
    tabs: Rect,
    content: Rect,
    selected: Rect,
    keys: Rect,
}

fn render(frame: &mut Frame<'_>, model: &mut TuiModel) {
    let layout = ui_layout(frame.area());
    let glyph_mode = detect_glyph_mode_from_env();
    let status = build_status_line(model, Utc::now(), glyph_mode);
    let header = Paragraph::new(Line::from(status))
        .block(Block::default().borders(Borders::ALL).title("Stat"));
    frame.render_widget(header, layout.status);

    let tab_titles = ["Timeline", "Repositories"]
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    let tabs = Tabs::new(tab_titles)
        .select(model.active_tab.index())
        .block(Block::default().borders(Borders::ALL).title("View"))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, layout.tabs);

    match model.active_tab {
        ActiveTab::Timeline => render_timeline_panel(frame, model, layout.content),
        ActiveTab::Repositories => render_repositories_panel(frame, model, layout.content),
    }

    let selected_inner_width = shrink_by_border(layout.selected).width as usize;
    let [selected_summary, selected_url] =
        build_selected_lines(model, glyph_mode, selected_inner_width);
    let selected = Paragraph::new(vec![Line::from(selected_summary), Line::from(selected_url)])
        .block(Block::default().borders(Borders::ALL).title("Sel"));
    frame.render_widget(selected, layout.selected);

    let keys = Paragraph::new(Line::from(build_keys_line(glyph_mode)))
        .block(Block::default().borders(Borders::ALL).title("Keys"));
    frame.render_widget(keys, layout.keys);

    if model.help_visible {
        render_help_overlay(frame);
    }
}

fn render_timeline_panel(frame: &mut Frame<'_>, model: &mut TuiModel, area: Rect) {
    let timeline_inner = shrink_by_border(area);
    model.timeline_page_size = (timeline_inner.height as usize).saturating_sub(1).max(1);

    let rows = if model.timeline.is_empty() {
        vec![timeline_empty_row()]
    } else {
        model
            .timeline
            .iter()
            .map(|event| timeline_row(event, model.is_event_read(&event.event_key())))
            .collect()
    };

    let table = Table::new(rows, timeline_constraints())
        .header(timeline_header().style(Style::default().add_modifier(Modifier::BOLD)))
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
    frame.render_stateful_widget(table, area, &mut state);
    model.timeline_offset = if model.timeline.is_empty() {
        0
    } else {
        state.offset()
    };
}

fn render_repositories_panel(frame: &mut Frame<'_>, model: &mut TuiModel, area: Rect) {
    let repo_items = if model.watched_repositories.is_empty() {
        vec![ListItem::new("No enabled repositories")]
    } else {
        model
            .watched_repositories
            .iter()
            .map(|repo| ListItem::new(repo.clone()))
            .collect()
    };
    let repo_list =
        List::new(repo_items).block(Block::default().borders(Borders::ALL).title("Repositories"));
    frame.render_widget(repo_list, area);
}

fn ui_layout(area: Rect) -> UiLayout {
    let vertical_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(4),
            Constraint::Length(3),
        ])
        .split(area);

    let main_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(vertical_areas[1]);

    UiLayout {
        status: vertical_areas[0],
        tabs: main_areas[0],
        content: main_areas[1],
        selected: vertical_areas[2],
        keys: vertical_areas[3],
    }
}

#[cfg(test)]
fn timeline_column_count() -> usize {
    4
}

fn timeline_constraints() -> Vec<Constraint> {
    vec![
        Constraint::Length(1),
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Min(12),
    ]
}

fn timeline_header() -> Row<'static> {
    Row::new(vec!["N", "Time", "Type", "Title"])
}

fn timeline_empty_row() -> Row<'static> {
    Row::new(vec![
        Cell::from("-"),
        Cell::from("-"),
        Cell::from("-"),
        Cell::from("No events yet"),
    ])
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
        EventKind::PrReviewRequested => "PR-REQ",
        EventKind::PrReviewSubmitted => "PR-REV",
        EventKind::PrMerged => "PR-MRG",
    }
}

fn event_kind_style(kind: &EventKind) -> Style {
    match kind {
        EventKind::PrCreated => Style::default().fg(Color::Cyan),
        EventKind::IssueCreated => Style::default().fg(Color::Yellow),
        EventKind::IssueCommentCreated => Style::default().fg(Color::Green),
        EventKind::PrReviewCommentCreated => Style::default().fg(Color::Magenta),
        EventKind::PrReviewRequested => Style::default().fg(Color::Blue),
        EventKind::PrReviewSubmitted => Style::default().fg(Color::LightBlue),
        EventKind::PrMerged => Style::default().fg(Color::LightGreen),
    }
}

fn unread_marker(is_read: bool) -> &'static str {
    if is_read {
        " "
    } else {
        "*"
    }
}

fn timeline_row(event: &WatchEvent, is_read: bool) -> Row<'static> {
    Row::new(vec![
        Cell::from(unread_marker(is_read)),
        Cell::from(format_timeline_time(event.created_at)),
        Cell::from(Span::styled(
            event_kind_label(&event.kind),
            event_kind_style(&event.kind),
        )),
        Cell::from(truncate_tail(&event.title, 120)),
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

fn sanitize_single_line(raw: &str) -> String {
    let mut sanitized = String::with_capacity(raw.len());
    let mut pending_space = false;

    for ch in raw.chars() {
        let mapped = if ch == '\n' || ch == '\r' || ch == '\t' || ch.is_control() {
            ' '
        } else {
            ch
        };

        if mapped.is_whitespace() {
            pending_space = true;
            continue;
        }

        if pending_space && !sanitized.is_empty() {
            sanitized.push(' ');
        }
        pending_space = false;
        sanitized.push(mapped);
    }

    sanitized
}

fn build_status_line(model: &TuiModel, now: DateTime<Utc>, glyph_mode: GlyphMode) -> String {
    if model.is_polling {
        let elapsed_secs = model
            .poll_started_at
            .map(|started| (now - started).num_seconds().max(0))
            .unwrap_or(0);
        let spinner = spinner_frame(elapsed_poll_millis(model.poll_started_at, now), glyph_mode);
        let refresh = if model.queued_refresh {
            "queued"
        } else {
            "none"
        };
        return match glyph_mode {
            GlyphMode::Nerd => format!(
                "󰚩 {spinner} poll 󱑂 {elapsed_secs}s 󰏗 {refresh} 󰅚 {}",
                model.failure_count
            ),
            GlyphMode::Ascii => format!(
                "~ {spinner} poll t={elapsed_secs}s refresh={refresh} fail={}",
                model.failure_count
            ),
        };
    }

    if is_error_status(&model.status_line) {
        let detail = truncate_tail(&sanitize_single_line(&model.status_line), 48);
        return match glyph_mode {
            GlyphMode::Nerd => format!("󰅚 {detail} 󰅚 {}", model.failure_count),
            GlyphMode::Ascii => format!("! {detail} fail={}", model.failure_count),
        };
    }

    let status = if is_quit_armed_status(&model.status_line) {
        "quit armed"
    } else {
        "ready"
    };
    let next_poll = format_compact_status_time(model.next_poll_at);

    match glyph_mode {
        GlyphMode::Nerd => {
            let prefix = if is_quit_armed_status(&model.status_line) {
                "󰈆"
            } else {
                "󰄬"
            };
            format!("{prefix} {status} 󱑆 {next_poll} 󰅚 {}", model.failure_count)
        }
        GlyphMode::Ascii => {
            let prefix = if is_quit_armed_status(&model.status_line) {
                ">"
            } else {
                "+"
            };
            format!(
                "{prefix} {status} next={next_poll} fail={}",
                model.failure_count
            )
        }
    }
}

fn build_selected_lines(model: &TuiModel, glyph_mode: GlyphMode, max_width: usize) -> [String; 2] {
    let (summary_raw, url_raw) = if let Some(event) = model.timeline.get(model.selected) {
        match glyph_mode {
            GlyphMode::Nerd => (
                format!(
                    "󰀷 {} 󰳝 {} 󰀄 @{} 󰎚 {}",
                    event_kind_label(&event.kind),
                    event.repo,
                    event.actor,
                    event.title
                ),
                format!("󰌹 {}", event.url),
            ),
            GlyphMode::Ascii => (
                format!(
                    "{} | {} | @{} | {}",
                    event_kind_label(&event.kind),
                    event.repo,
                    event.actor,
                    event.title
                ),
                event.url.clone(),
            ),
        }
    } else {
        match glyph_mode {
            GlyphMode::Nerd => ("󰘕 no selection".to_string(), "-".to_string()),
            GlyphMode::Ascii => ("no selection".to_string(), "-".to_string()),
        }
    };

    [
        truncate_tail(&summary_raw, max_width),
        truncate_tail(&url_raw, max_width),
    ]
}

fn build_keys_line(_glyph_mode: GlyphMode) -> String {
    "q quit | Esc Esc quit | r refresh | Tab switch | ? help | Enter open".to_string()
}

fn format_compact_status_time(dt: Option<DateTime<Utc>>) -> String {
    dt.map(|d| d.format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn is_quit_armed_status(status: &str) -> bool {
    status.starts_with("press Esc again")
}

fn is_error_status(status: &str) -> bool {
    let lower = status.to_ascii_lowercase();
    lower.contains("failed") || lower.contains("error")
}

fn elapsed_poll_millis(started_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> i64 {
    started_at
        .map(|started| (now - started).num_milliseconds().max(0))
        .unwrap_or(0)
}

fn spinner_frame(elapsed_millis: i64, glyph_mode: GlyphMode) -> &'static str {
    const FRAME_INTERVAL_MS: i64 = 120;

    let frame_step = elapsed_millis.div_euclid(FRAME_INTERVAL_MS);
    match glyph_mode {
        GlyphMode::Nerd => {
            const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            FRAMES[(frame_step.rem_euclid(FRAMES.len() as i64)) as usize]
        }
        GlyphMode::Ascii => {
            const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
            FRAMES[(frame_step.rem_euclid(FRAMES.len() as i64)) as usize]
        }
    }
}

fn detect_glyph_mode_from_env() -> GlyphMode {
    let term = std::env::var("TERM").ok();
    let lc_all = std::env::var("LC_ALL").ok();
    let lc_ctype = std::env::var("LC_CTYPE").ok();
    let lang = std::env::var("LANG").ok();
    detect_glyph_mode(
        term.as_deref(),
        lc_all.as_deref(),
        lc_ctype.as_deref(),
        lang.as_deref(),
    )
}

fn detect_glyph_mode(
    term: Option<&str>,
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> GlyphMode {
    if term.is_some_and(|value| value.eq_ignore_ascii_case("dumb")) {
        return GlyphMode::Ascii;
    }

    let locale = lc_all
        .filter(|value| !value.is_empty())
        .or(lc_ctype.filter(|value| !value.is_empty()))
        .or(lang.filter(|value| !value.is_empty()));

    if let Some(locale) = locale {
        let normalized = locale.to_ascii_uppercase();
        if !normalized.contains("UTF-8") && !normalized.contains("UTF8") {
            return GlyphMode::Ascii;
        }
    }

    GlyphMode::Nerd
}

fn timeline_inner_area(area: Rect) -> Rect {
    let layout = ui_layout(area);
    shrink_by_border(layout.content)
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
        Line::from("q: quit immediately"),
        Line::from("Esc twice within 1.5s: quit"),
        Line::from("Tab / Shift+Tab: switch Timeline and Repositories"),
        Line::from("r: refresh, ?: toggle help, enter: open selected URL"),
        Line::from("up/down or j/k: move one row (Timeline tab)"),
        Line::from("page up/page down: move one page (Timeline tab)"),
        Line::from("g/home: top, G/end: bottom (Timeline tab)"),
        Line::from("mouse: click to select, wheel to scroll (Timeline tab)"),
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
    use std::collections::HashSet;

    use chrono::TimeZone;
    use ratatui::layout::Rect;

    use super::{
        build_keys_line, build_selected_lines, build_status_line, detect_glyph_mode,
        timeline_column_count, truncate_tail, ui_layout, unread_marker, GlyphMode, TuiModel,
    };
    use crate::domain::events::{EventKind, WatchEvent};

    fn event(id: &str, created_at: chrono::DateTime<chrono::Utc>) -> WatchEvent {
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
    fn truncate_tail_adds_ellipsis_for_long_values() {
        assert_eq!(truncate_tail("abcdef", 5), "ab...");
        assert_eq!(truncate_tail("abc", 5), "abc");
        assert_eq!(truncate_tail("abc", 2), "..");
    }

    #[test]
    fn loading_status_line_uses_nerd_spinner_and_shows_queued_while_polling() {
        let started_at = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()
            + chrono::Duration::milliseconds(360);

        let mut model = TuiModel::new(10);
        model.is_polling = true;
        model.poll_started_at = Some(started_at);
        model.queued_refresh = true;

        let line = build_status_line(&model, now, GlyphMode::Nerd);
        assert_eq!(line, "󰚩 ⠸ poll 󱑂 0s 󰏗 queued 󰅚 0");
    }

    #[test]
    fn loading_status_line_uses_next_poll_when_not_polling() {
        let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();

        let mut model = TuiModel::new(10);
        model.is_polling = false;
        model.next_poll_at = Some(chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 5, 0).unwrap());
        model.status_line = "ok (new=2)".to_string();

        let line = build_status_line(&model, now, GlyphMode::Ascii);
        assert_eq!(line, "+ ready next=01-01 00:05 fail=0");
    }

    #[test]
    fn loading_status_line_shows_error_detail_for_failure_states() {
        let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();

        let mut model = TuiModel::new(10);
        model.status_line = "open failed: browser missing".to_string();
        let line_error = build_status_line(&model, now, GlyphMode::Ascii);
        assert_eq!(line_error, "! open failed: browser missing fail=0");

        model.status_line = "opened: https://example.com/x".to_string();
        let line_ok = build_status_line(&model, now, GlyphMode::Ascii);
        assert_eq!(line_ok, "+ ready next=- fail=0");
    }

    #[test]
    fn loading_status_line_sanitizes_multiline_error_detail() {
        let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let mut model = TuiModel::new(10);
        model.status_line = "open failed:\nlauncher\tmissing\u{0007}".to_string();

        let line = build_status_line(&model, now, GlyphMode::Ascii);
        assert_eq!(line, "! open failed: launcher missing fail=0");
        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));
        assert!(!line.contains('\t'));
    }

    #[test]
    fn loading_status_line_shows_quit_armed_label() {
        let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let mut model = TuiModel::new(10);
        model.status_line = "press Esc again to quit (1.5s)".to_string();

        let line = build_status_line(&model, now, GlyphMode::Ascii);
        assert_eq!(line, "> quit armed next=- fail=0");
    }

    #[test]
    fn keys_line_is_single_dense_row() {
        let line = build_keys_line(GlyphMode::Ascii);
        assert_eq!(
            line,
            "q quit | Esc Esc quit | r refresh | Tab switch | ? help | Enter open"
        );
    }

    #[test]
    fn keys_line_uses_same_text_for_nerd_mode() {
        let ascii = build_keys_line(GlyphMode::Ascii);
        let nerd = build_keys_line(GlyphMode::Nerd);
        assert_eq!(nerd, ascii);
    }

    #[test]
    fn keys_line_omits_navigation_hints() {
        let ascii = build_keys_line(GlyphMode::Ascii);
        let nerd = build_keys_line(GlyphMode::Nerd);
        for line in [&ascii, &nerd] {
            assert!(!line.contains("jk/"));
            assert!(!line.contains("PgUpDn"));
            assert!(!line.contains("Pg↑↓"));
            assert!(!line.contains("g/G"));
        }
    }

    #[test]
    fn selected_lines_compact_event_detail_and_url_into_two_lines() {
        let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let mut model = TuiModel::new(10);
        model.replace_timeline(vec![event("a", now)]);
        let [line1, line2] = build_selected_lines(&model, GlyphMode::Ascii, 200);
        assert_eq!(line1, "I-CMT | acme/api | @dev | comment");
        assert_eq!(line2, "https://example.com/a");
    }

    #[test]
    fn selected_lines_truncate_to_available_width() {
        let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let mut model = TuiModel::new(10);
        let mut long_event = event("a", now);
        long_event.title = "a very very very long title that must be clipped".to_string();
        model.replace_timeline(vec![long_event]);
        let [line1, line2] = build_selected_lines(&model, GlyphMode::Ascii, 40);
        assert!(line1.chars().count() <= 40);
        assert!(line1.ends_with("..."));
        assert!(line2.chars().count() <= 40);
        assert!(!line2.is_empty());
    }

    #[test]
    fn ui_layout_uses_compact_panel_heights() {
        let layout = ui_layout(Rect::new(0, 0, 120, 40));
        assert_eq!(layout.status.height, 3);
        assert_eq!(layout.tabs.height, 3);
        assert_eq!(layout.selected.height, 4);
        assert_eq!(layout.keys.height, 3);
    }

    #[test]
    fn glyph_mode_uses_ascii_for_dumb_term() {
        let mode = detect_glyph_mode(
            Some("dumb"),
            Some("en_US.UTF-8"),
            Some("ja_JP.UTF-8"),
            Some("en_US.UTF-8"),
        );
        assert_eq!(mode, GlyphMode::Ascii);
    }

    #[test]
    fn glyph_mode_uses_ascii_for_non_utf8_locale() {
        let mode = detect_glyph_mode(None, None, Some("C"), None);
        assert_eq!(mode, GlyphMode::Ascii);
    }

    #[test]
    fn glyph_mode_prioritizes_lc_all_over_lang() {
        let mode = detect_glyph_mode(None, Some("C"), Some("ja_JP.UTF-8"), Some("en_US.UTF-8"));
        assert_eq!(mode, GlyphMode::Ascii);
    }

    #[test]
    fn glyph_mode_defaults_to_nerd_in_utf8_environment() {
        let mode = detect_glyph_mode(None, None, Some("ja_JP.UTF-8"), None);
        assert_eq!(mode, GlyphMode::Nerd);
    }

    #[test]
    fn unread_marker_shows_asterisk_only_for_unread_event() {
        assert_eq!(unread_marker(false), "*");
        assert_eq!(unread_marker(true), " ");
    }

    #[test]
    fn timeline_uses_minimum_column_set() {
        assert_eq!(timeline_column_count(), 4);
    }

    #[test]
    fn model_read_state_can_be_replaced_and_extended() {
        let mut model = TuiModel::new(10);
        let a = event(
            "a",
            chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        );
        let b = event(
            "b",
            chrono::Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap(),
        );
        model.replace_timeline(vec![a.clone(), b.clone()]);

        let key_a = a.event_key();
        let key_b = b.event_key();
        model.replace_read_event_keys(HashSet::from([key_a.clone()]));
        assert!(model.is_event_read(&key_a));
        assert!(!model.is_event_read(&key_b));

        model.mark_event_read(&key_b);
        assert!(model.is_event_read(&key_a));
        assert!(model.is_event_read(&key_b));
    }
}
