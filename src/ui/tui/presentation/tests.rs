use chrono::TimeZone;

use super::{build_selected_lines, build_status_line, detect_glyph_mode, truncate_tail, GlyphMode};
use crate::{
    domain::events::{EventKind, WatchEvent},
    ui::tui::TuiModel,
};

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
fn loading_status_line_sanitizes_multiline_error_detail() {
    let now = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let mut model = TuiModel::new(10);
    model.status_line = "open failed:\nlauncher\tmissing\u{0007}".to_string();

    let line = build_status_line(&model, now, GlyphMode::Ascii);
    assert_eq!(line, "! open failed: launcher missing fail=0");
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
fn glyph_mode_uses_ascii_for_dumb_term() {
    let mode = detect_glyph_mode(
        Some("dumb"),
        Some("en_US.UTF-8"),
        Some("ja_JP.UTF-8"),
        Some("en_US.UTF-8"),
    );
    assert_eq!(mode, GlyphMode::Ascii);
}
