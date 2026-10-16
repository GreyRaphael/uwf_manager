//! Tab 3：排除项管理（左文件 | 右注册表，当前/下次双视图）。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, List, ListItem};

use crate::app::{App, ExclusionFocus};
use crate::ui::{text, theme};

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let [files_area, registry_area] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);

    let session_label = if app.exclusion_session_current {
        "当前生效"
    } else {
        "下次生效"
    };
    let files_focused = app.exclusion_focus == ExclusionFocus::Files;
    let registry_focused = !files_focused;

    let file_items = items_or_empty(app.visible_file_exclusions().iter().map(|e| e.display()));
    let files = List::new(file_items)
        .block(
            Block::bordered()
                .title(format!("[文件排除项] 视图: {session_label}"))
                .border_style(theme::focus_border(files_focused)),
        )
        .highlight_style(theme::selected())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(files, files_area, &mut app.file_exclusion_state);

    let registry_items =
        items_or_empty(app.visible_registry_exclusions().iter().cloned());
    let registry = List::new(registry_items)
        .block(
            Block::bordered()
                .title("[注册表排除项]  [S] 切换当前/下次".to_string())
                .border_style(theme::focus_border(registry_focused)),
        )
        .highlight_style(theme::selected())
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(registry, registry_area, &mut app.registry_exclusion_state);
}

fn items_or_empty(iter: impl Iterator<Item = String>) -> Vec<ListItem<'static>> {
    let items: Vec<ListItem<'static>> = iter.map(ListItem::new).collect();
    if items.is_empty() {
        vec![ListItem::new(text::READY_EMPTY).style(theme::dim())]
    } else {
        items
    }
}
