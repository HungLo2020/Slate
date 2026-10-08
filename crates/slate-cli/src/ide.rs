//! Terminal drawing of the IDE overlays: the picker (files, symbols,
//! search, problems…), the completion list and hover text.
use crate::{clean, colors::ColorMode};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use slate_core::Snapshot;

/// The text area of a pane (inside its border and tab row).
fn text_origin(s: &Snapshot, pane: u64) -> Option<(u16, u16, u16, u16)> {
    let p = s.panes.iter().find(|p| p.id == pane)?;
    Some((
        p.rect.x + 1,
        p.rect.y + 2,
        p.rect.width.saturating_sub(2),
        p.rect.height.saturating_sub(3),
    ))
}

/// A label with the characters at `positions` emphasised.
fn marked(label: &str, positions: &[u32], emphasis: Style) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, c) in clean(label).chars().enumerate() {
        let style = if positions.binary_search(&(i as u32)).is_ok() {
            emphasis
        } else {
            Style::default()
        };
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(c),
            _ => spans.push(Span::styled(c.to_string(), style)),
        }
    }
    Line::from(spans)
}

/// The picker's box on a screen of `size`.
fn picker_area(size: Rect) -> Rect {
    let height = size.height.saturating_sub(4).min(20);
    let width = size.width.saturating_sub(4).min(100);
    Rect::new((size.width - width) / 2, 1, width, height)
}

/// The picker's list rows and the first item (counted over all items)
/// drawn at the top, so clicks map to the items drawn.
fn picker_rows(size: Rect, p: &slate_core::picker::PickerView) -> (Rect, usize) {
    let area = picker_area(size);
    let rows = Rect::new(
        area.x + 1,
        area.y + 2,
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    let selected = p.selected.saturating_sub(p.first);
    let height = rows.height.max(1) as usize;
    let offset = (selected + 1).saturating_sub(height);
    (rows, p.first + offset)
}

/// The picker command for a mouse event at a screen cell, if any.
pub fn picker_mouse(
    size: Rect,
    p: &slate_core::picker::PickerView,
    kind: crossterm::event::MouseEventKind,
    x: u16,
    y: u16,
) -> Option<slate_core::Command> {
    use crossterm::event::{MouseButton, MouseEventKind};
    use slate_core::Command;
    match kind {
        MouseEventKind::ScrollUp => Some(Command::PickerMove { delta: -3 }),
        MouseEventKind::ScrollDown => Some(Command::PickerMove { delta: 3 }),
        MouseEventKind::Down(MouseButton::Left) => {
            let (rows, top) = picker_rows(size, p);
            if !rows.contains(Position::new(x, y)) {
                // A click outside the box closes it, as Escape does.
                let outside = !picker_area(size).contains(Position::new(x, y));
                return outside.then_some(Command::PickerClose);
            }
            let index = top + (y - rows.y) as usize;
            (index < p.first + p.items.len())
                .then_some(Command::PickerAccept { index: Some(index) })
        }
        _ => None,
    }
}

pub fn render_picker(frame: &mut Frame, s: &Snapshot, colors: ColorMode) -> bool {
    let Some(p) = &s.picker else {
        return false;
    };
    let size = frame.area();
    let area = picker_area(size);
    frame.render_widget(Clear, area);
    let mut title = format!(" {} ", p.title);
    if p.kind == "search" {
        title.push_str(&format!(
            "· Alt-C case {} · Alt-W word {} · Alt-R regex {} · Ctrl-H replace ",
            if p.case_sensitive { "on" } else { "off" },
            if p.whole_word { "on" } else { "off" },
            if p.regex { "on" } else { "off" }
        ));
    }
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_bottom(format!(
                " {}{} · ↑↓ · Enter or click opens · Esc closes ",
                if p.busy { "… " } else { "" },
                if p.message.is_empty() {
                    format!("{} item{}", p.total, if p.total == 1 { "" } else { "s" })
                } else {
                    clean(&p.message)
                }
            )),
        area,
    );
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    frame.render_widget(
        Paragraph::new(format!("> {}", clean(&p.query))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let emphasis = Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let detail_style = colors.palette_item(false);
    let items: Vec<ListItem> = p
        .items
        .iter()
        .map(|item| {
            let mut line = marked(&item.label, &item.positions, emphasis);
            if !item.detail.is_empty() {
                line.spans.push(Span::styled(
                    format!("  {}", clean(&item.detail)),
                    detail_style,
                ));
            }
            ListItem::new(line)
        })
        .collect();
    // Items are a window of the list starting at `first`.
    let (rows, top) = picker_rows(size, p);
    let mut state = ListState::default()
        .with_offset(top - p.first)
        .with_selected((!p.items.is_empty()).then_some(p.selected.saturating_sub(p.first)));
    frame.render_stateful_widget(
        List::new(items).highlight_style(colors.highlight()),
        rows,
        &mut state,
    );
    frame.set_cursor_position((
        inner.x
            + 2
            + (slate_core::document::display_width(&p.query) as u16)
                .min(inner.width.saturating_sub(3)),
        inner.y,
    ));
    true
}

/// A box at a cell of a pane's text, below it when there is room.
fn popup_area(
    s: &Snapshot,
    pane: u64,
    row: usize,
    col: usize,
    width: u16,
    height: u16,
) -> Option<Rect> {
    let (x, y, w, h) = text_origin(s, pane)?;
    let size_w = x + w;
    let anchor_y = y + row as u16;
    let width = width.min(w);
    let below = anchor_y + 1;
    let top = if below + height <= y + h {
        below
    } else {
        anchor_y.saturating_sub(height).max(y)
    };
    let left = (x + col as u16).min(size_w.saturating_sub(width));
    Some(Rect::new(left, top, width, height.min(h)))
}

pub fn render_completion(frame: &mut Frame, s: &Snapshot, colors: ColorMode) {
    let Some(c) = &s.completion else { return };
    let shown = c.items.len().min(10);
    let widest = c
        .items
        .iter()
        .map(|i| {
            let info = if i.detail.is_empty() {
                &i.kind
            } else {
                &i.detail
            };
            i.label.chars().count() + info.chars().count().min(30) + 4
        })
        .max()
        .unwrap_or(10)
        .clamp(22, 60) as u16;
    let Some(area) = popup_area(s, c.pane, c.row, c.col, widest + 2, shown as u16 + 2) else {
        return;
    };
    frame.render_widget(Clear, area);
    let detail = colors.palette_item(false);
    let items: Vec<ListItem> = c
        .items
        .iter()
        .map(|i| {
            // The type or signature when the server sends one, else the kind.
            let info = if i.detail.is_empty() {
                &i.kind
            } else {
                &i.detail
            };
            ListItem::new(Line::from(vec![
                Span::raw(clean(&i.label)),
                Span::styled(format!("  {}", clean(info)), detail),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(c.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Tab/Enter accepts "),
            )
            .highlight_style(colors.highlight()),
        area,
        &mut state,
    );
}

/// Hover lines shown before the text is cut short.
const HOVER_LINES: usize = 20;

pub fn render_hover(frame: &mut Frame, s: &Snapshot) {
    let Some(h) = &s.hover else { return };
    let mut text = clean(&h.text);
    if text.lines().count() > HOVER_LINES {
        text = text
            .lines()
            .take(HOVER_LINES)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n…";
    }
    let width = text
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(10)
        .clamp(10, 70) as u16
        + 2;
    let lines = text
        .lines()
        .map(|l| {
            (l.chars().count() as u16)
                .div_ceil(width.saturating_sub(2).max(1))
                .max(1)
        })
        .sum::<u16>()
        .min(HOVER_LINES as u16 + 1)
        + 2;
    let Some(area) = popup_area(s, h.pane, h.row, h.col, width, lines) else {
        return;
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

/// Debugging, language-server activity, the branch, problems, restricted
/// mode and the diagnostic under the caret, for the status line.
pub fn status_suffix(s: &Snapshot) -> String {
    let mut parts = Vec::new();
    if s.debugging {
        parts.push(
            if s.debug_paused {
                "Debug: paused"
            } else {
                "Debug: running"
            }
            .into(),
        );
    }
    if !s.activity.is_empty() {
        parts.push(clean(&s.activity));
    }
    if !s.git_branch.is_empty() {
        parts.push(format!("⎇ {}", clean(&s.git_branch)));
    }
    let (errors, warnings) = s.problems;
    if errors + warnings > 0 {
        parts.push(format!("✖ {errors} ⚠ {warnings}"));
    }
    if !s.problem.is_empty() {
        parts.push(s.problem.clone());
    }
    if !s.trusted {
        parts.push("Restricted".into());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{MouseButton, MouseEventKind};
    use slate_core::{
        picker::{PickerItem, PickerView},
        Command,
    };

    fn picker(first: usize, selected: usize, shown: usize) -> PickerView {
        PickerView {
            kind: "files".into(),
            title: "Files".into(),
            query: String::new(),
            items: (first..first + shown)
                .map(|i| PickerItem {
                    label: format!("file{i}"),
                    detail: String::new(),
                    kind: String::new(),
                    positions: Vec::new(),
                })
                .collect(),
            first,
            selected,
            total: 500,
            busy: false,
            message: String::new(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
        }
    }

    #[test]
    fn clicks_choose_the_item_drawn_under_the_mouse() {
        let size = Rect::new(0, 0, 80, 30);
        let click = MouseEventKind::Down(MouseButton::Left);
        // The first list row is below the border and the query line.
        let accept = |p: &PickerView, row| match picker_mouse(size, p, click, 10, 3 + row) {
            Some(Command::PickerAccept { index }) => index,
            other => panic!("{other:?}"),
        };
        assert_eq!(accept(&picker(0, 0, 40), 0), Some(0));
        assert_eq!(accept(&picker(0, 0, 40), 4), Some(4));
        // A window further down the list, scrolled to keep the selection in view.
        let p = picker(200, 230, 40);
        let (rows, top) = picker_rows(size, &p);
        assert_eq!(top + rows.height as usize - 1, 230);
        assert_eq!(accept(&p, rows.height - 1), Some(230));
        assert!(matches!(
            picker_mouse(size, &p, click, 0, 29),
            Some(Command::PickerClose)
        ));
        assert!(matches!(
            picker_mouse(size, &p, MouseEventKind::ScrollDown, 10, 5),
            Some(Command::PickerMove { delta: 3 })
        ));
    }
}
