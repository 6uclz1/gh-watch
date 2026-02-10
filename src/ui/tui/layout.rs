use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UiLayout {
    pub(crate) status: Rect,
    pub(crate) tabs: Rect,
    pub(crate) content: Rect,
    pub(crate) selected: Rect,
    pub(crate) keys: Rect,
}

pub(crate) fn ui_layout(area: Rect) -> UiLayout {
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

pub(crate) fn timeline_inner_area(area: Rect) -> Rect {
    let layout = ui_layout(area);
    shrink_by_border(layout.content)
}

pub(crate) fn shrink_by_border(area: Rect) -> Rect {
    if area.width <= 2 || area.height <= 2 {
        return Rect::new(area.x, area.y, 0, 0);
    }

    Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2)
}

pub(crate) fn contains_point(area: Rect, x: u16, y: u16) -> bool {
    if area.width == 0 || area.height == 0 {
        return false;
    }

    let x_end = area.x.saturating_add(area.width);
    let y_end = area.y.saturating_add(area.height);
    x >= area.x && x < x_end && y >= area.y && y < y_end
}

pub(crate) fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
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
    use ratatui::layout::Rect;

    use super::ui_layout;

    #[test]
    fn ui_layout_uses_compact_panel_heights() {
        let layout = ui_layout(Rect::new(0, 0, 120, 40));
        assert_eq!(layout.status.height, 3);
        assert_eq!(layout.tabs.height, 3);
        assert_eq!(layout.selected.height, 4);
        assert_eq!(layout.keys.height, 3);
    }
}
