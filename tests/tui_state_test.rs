use chrono::{TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use gh_watch::domain::events::{EventKind, WatchEvent};
use gh_watch::ui::tui::{
    handle_input, parse_input, parse_mouse_input, ActiveTab, InputCommand, TuiModel,
};
use ratatui::layout::Rect;

fn ev(id: &str, ts: chrono::DateTime<Utc>) -> WatchEvent {
    ev_with(
        id,
        ts,
        EventKind::IssueCommentCreated,
        "acme/api",
        "dev",
        &format!("comment {}", id),
    )
}

fn ev_with(
    id: &str,
    ts: chrono::DateTime<Utc>,
    kind: EventKind,
    repo: &str,
    actor: &str,
    title: &str,
) -> WatchEvent {
    WatchEvent {
        event_id: id.to_string(),
        repo: repo.to_string(),
        kind,
        actor: actor.to_string(),
        title: title.to_string(),
        url: format!("https://example.com/{}", id),
        created_at: ts,
        source_item_id: id.to_string(),
        subject_author: Some(actor.to_string()),
        requested_reviewer: None,
        mentions: Vec::new(),
    }
}

#[test]
fn timeline_keeps_newest_first() {
    let mut model = TuiModel::new(10);
    model.push_timeline(vec![ev(
        "old",
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    )]);
    model.push_timeline(vec![ev(
        "new",
        Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap(),
    )]);

    assert_eq!(model.timeline[0].event_id, "new");
    assert_eq!(model.timeline[1].event_id, "old");
}

#[test]
fn input_scroll_changes_selection() {
    let mut model = TuiModel::new(10);
    model.push_timeline(vec![ev(
        "1",
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    )]);
    model.push_timeline(vec![ev(
        "2",
        Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap(),
    )]);

    handle_input(&mut model, InputCommand::ScrollDown);
    assert_eq!(model.selected, 1);
    handle_input(&mut model, InputCommand::ScrollUp);
    assert_eq!(model.selected, 0);
}

#[test]
fn watched_repositories_start_empty_and_can_be_set() {
    let mut model = TuiModel::new(10);
    assert!(model.watched_repositories.is_empty());
    assert!(!model.is_polling);
    assert!(model.poll_started_at.is_none());
    assert!(!model.queued_refresh);

    model.watched_repositories = vec!["acme/api".to_string(), "acme/web".to_string()];
    assert_eq!(
        model.watched_repositories,
        vec!["acme/api".to_string(), "acme/web".to_string()]
    );
}

#[test]
fn enter_key_maps_to_open_selected_url() {
    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(parse_input(key), InputCommand::OpenSelectedUrl);
}

#[test]
fn extended_navigation_keys_map_to_commands() {
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
        InputCommand::ToggleHelp
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        InputCommand::ScrollDown
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)),
        InputCommand::ScrollUp
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
        InputCommand::PageDown
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
        InputCommand::PageUp
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)),
        InputCommand::JumpTop
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)),
        InputCommand::JumpBottom
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
        InputCommand::JumpTop
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
        InputCommand::JumpBottom
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        InputCommand::NextTab
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        InputCommand::PrevTab
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        InputCommand::EscapePressed
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE)),
        InputCommand::None
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)),
        InputCommand::None
    );
    assert_eq!(
        parse_input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        InputCommand::None
    );
}

#[test]
fn help_toggle_switches_visibility() {
    let mut model = TuiModel::new(10);
    assert!(!model.help_visible);

    handle_input(&mut model, InputCommand::ToggleHelp);
    assert!(model.help_visible);

    handle_input(&mut model, InputCommand::ToggleHelp);
    assert!(!model.help_visible);
}

#[test]
fn tab_switch_cycles_between_timeline_and_repositories() {
    let mut model = TuiModel::new(10);
    assert_eq!(model.active_tab, ActiveTab::Timeline);

    handle_input(&mut model, InputCommand::NextTab);
    assert_eq!(model.active_tab, ActiveTab::Repositories);

    handle_input(&mut model, InputCommand::NextTab);
    assert_eq!(model.active_tab, ActiveTab::Timeline);

    handle_input(&mut model, InputCommand::PrevTab);
    assert_eq!(model.active_tab, ActiveTab::Repositories);
}

#[test]
fn page_navigation_uses_page_size() {
    let mut model = TuiModel::new(10);
    for i in 0..8 {
        model.push_timeline(vec![ev(
            &format!("{i}"),
            Utc.with_ymd_and_hms(2025, 1, i + 1, 0, 0, 0).unwrap(),
        )]);
    }
    model.timeline_page_size = 3;
    model.selected = 0;

    handle_input(&mut model, InputCommand::PageDown);
    assert_eq!(model.selected, 3);

    handle_input(&mut model, InputCommand::PageUp);
    assert_eq!(model.selected, 0);
}

#[test]
fn jump_commands_move_to_edges() {
    let mut model = TuiModel::new(10);
    for i in 0..5 {
        model.push_timeline(vec![ev(
            &format!("{i}"),
            Utc.with_ymd_and_hms(2025, 1, i + 1, 0, 0, 0).unwrap(),
        )]);
    }
    model.selected = 2;

    handle_input(&mut model, InputCommand::JumpBottom);
    assert_eq!(model.selected, 4);

    handle_input(&mut model, InputCommand::JumpTop);
    assert_eq!(model.selected, 0);
}

#[test]
fn timeline_keeps_selected_event_after_refresh_when_event_survives() {
    let mut model = TuiModel::new(10);
    model.push_timeline(vec![
        ev("a", Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
        ev("b", Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap()),
    ]);
    model.selected = 1;
    model.selected_event_key = Some(model.timeline[1].event_key());

    model.push_timeline(vec![ev(
        "c",
        Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    )]);

    assert_eq!(model.timeline[model.selected].event_id, "a");
}

#[test]
fn timeline_selection_falls_back_when_selected_event_drops_out() {
    let mut model = TuiModel::new(2);
    model.push_timeline(vec![
        ev("a", Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
        ev("b", Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap()),
    ]);
    model.selected = 1;
    model.selected_event_key = Some(model.timeline[1].event_key());

    model.push_timeline(vec![ev(
        "c",
        Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    )]);

    assert_eq!(model.timeline.len(), 2);
    assert_eq!(model.selected, 1);
    assert!(model
        .selected_event_key
        .as_ref()
        .is_some_and(|key| key == &model.timeline[1].event_key()));
}

#[test]
fn non_timeline_tab_ignores_timeline_navigation_input() {
    let mut model = TuiModel::new(10);
    model.push_timeline(vec![
        ev("a", Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
        ev("b", Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap()),
    ]);
    model.selected = 0;
    model.active_tab = ActiveTab::Repositories;

    handle_input(&mut model, InputCommand::ScrollDown);
    assert_eq!(model.selected, 0);
}

#[test]
fn mouse_click_in_timeline_selects_row_using_offset() {
    let mut model = TuiModel::new(10);
    model.push_timeline(vec![ev(
        "1",
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    )]);
    model.push_timeline(vec![ev(
        "2",
        Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap(),
    )]);
    model.push_timeline(vec![ev(
        "3",
        Utc.with_ymd_and_hms(2025, 1, 3, 0, 0, 0).unwrap(),
    )]);
    model.timeline_offset = 1;

    let area = Rect::new(0, 0, 100, 30);
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 8,
        modifiers: KeyModifiers::NONE,
    };

    let cmd = parse_mouse_input(click, area, &model);
    assert_eq!(cmd, InputCommand::SelectIndex(1));
}

#[test]
fn polling_state_fields_are_mutable_for_loading_transitions() {
    let mut model = TuiModel::new(10);
    let started_at = Utc.with_ymd_and_hms(2025, 1, 5, 12, 30, 0).unwrap();

    model.is_polling = true;
    model.poll_started_at = Some(started_at);
    model.queued_refresh = true;

    assert!(model.is_polling);
    assert_eq!(model.poll_started_at, Some(started_at));
    assert!(model.queued_refresh);
}

#[test]
fn mouse_click_outside_timeline_returns_none() {
    let mut model = TuiModel::new(10);
    model.push_timeline(vec![ev(
        "1",
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    )]);

    let area = Rect::new(0, 0, 100, 30);
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 95,
        row: 4,
        modifiers: KeyModifiers::NONE,
    };

    let cmd = parse_mouse_input(click, area, &model);
    assert_eq!(cmd, InputCommand::None);
}

#[test]
fn mouse_wheel_maps_to_scroll_commands() {
    let mut model = TuiModel::new(10);
    model.push_timeline(vec![ev(
        "1",
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    )]);

    let area = Rect::new(0, 0, 100, 30);
    let wheel_up = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 2,
        row: 13,
        modifiers: KeyModifiers::NONE,
    };
    let wheel_down = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 2,
        row: 13,
        modifiers: KeyModifiers::NONE,
    };

    assert_eq!(
        parse_mouse_input(wheel_up, area, &model),
        InputCommand::ScrollUp
    );
    assert_eq!(
        parse_mouse_input(wheel_down, area, &model),
        InputCommand::ScrollDown
    );
}

#[test]
fn mouse_input_is_disabled_on_repositories_tab() {
    let mut model = TuiModel::new(10);
    model.push_timeline(vec![ev(
        "1",
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    )]);
    model.active_tab = ActiveTab::Repositories;

    let area = Rect::new(0, 0, 100, 30);
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 2,
        row: 13,
        modifiers: KeyModifiers::NONE,
    };
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 13,
        modifiers: KeyModifiers::NONE,
    };

    assert_eq!(parse_mouse_input(wheel, area, &model), InputCommand::None);
    assert_eq!(parse_mouse_input(click, area, &model), InputCommand::None);
}
