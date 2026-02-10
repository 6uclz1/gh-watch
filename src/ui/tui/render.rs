use std::io::{stdout, Stdout};

use anyhow::Result;
use chrono::Utc;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Table, TableState, Tabs, Wrap},
    Frame, Terminal,
};

use super::{
    layout::{centered_rect, shrink_by_border, ui_layout},
    model::{ActiveTab, TuiModel},
    presentation::{
        build_keys_line, build_selected_lines, build_status_line, detect_glyph_mode_from_env,
        timeline_constraints, timeline_empty_row, timeline_header, timeline_row,
    },
};

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

    pub fn terminal_area(&self) -> Result<ratatui::layout::Rect> {
        let size = self.terminal.size()?;
        Ok(ratatui::layout::Rect::new(0, 0, size.width, size.height))
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

    let keys = Paragraph::new(Line::from(build_keys_line()))
        .block(Block::default().borders(Borders::ALL).title("Keys"));
    frame.render_widget(keys, layout.keys);

    if model.help_visible {
        render_help_overlay(frame);
    }
}

fn render_timeline_panel(frame: &mut Frame<'_>, model: &mut TuiModel, area: ratatui::layout::Rect) {
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

fn render_repositories_panel(
    frame: &mut Frame<'_>,
    model: &mut TuiModel,
    area: ratatui::layout::Rect,
) {
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
