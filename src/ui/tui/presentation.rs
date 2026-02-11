use chrono::{DateTime, Utc};
use ratatui::{
    layout::Constraint,
    style::{Color, Style},
    text::Span,
    widgets::{Cell, Row},
};

use crate::domain::events::{EventKind, WatchEvent};

use super::model::TuiModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlyphMode {
    Nerd,
    Ascii,
}

pub(crate) fn timeline_constraints() -> Vec<Constraint> {
    vec![
        Constraint::Length(1),
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Min(12),
    ]
}

pub(crate) fn timeline_header() -> Row<'static> {
    Row::new(vec!["N", "Time", "Type", "Title"])
}

pub(crate) fn timeline_empty_row() -> Row<'static> {
    timeline_empty_row_with_message("No events yet")
}

pub(crate) fn timeline_empty_row_with_message(message: &str) -> Row<'static> {
    Row::new(vec![
        Cell::from("-"),
        Cell::from("-"),
        Cell::from("-"),
        Cell::from(message.to_string()),
    ])
}

pub(crate) fn timeline_row(event: &WatchEvent, is_read: bool) -> Row<'static> {
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

pub(crate) fn build_status_line(
    model: &TuiModel,
    now: DateTime<Utc>,
    glyph_mode: GlyphMode,
) -> String {
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

pub(crate) fn build_selected_lines(
    model: &TuiModel,
    glyph_mode: GlyphMode,
    max_width: usize,
) -> [String; 2] {
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

pub(crate) fn build_keys_line() -> String {
    "q quit | Esc Esc quit | r refresh | Tab switch | ? help | Enter open".to_string()
}

pub(crate) fn detect_glyph_mode_from_env() -> GlyphMode {
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

#[cfg(test)]
mod tests;
