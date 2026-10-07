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

pub fn render_picker(frame: &mut Frame, s: &Snapshot, colors: ColorMode) -> bool {
    let Some(p) = &s.picker else {
        return false;
    };
    let size = frame.area();
    let height = size.height.saturating_sub(4).min(20);
    let width = size.width.saturating_sub(4).min(100);
    let area = Rect::new((size.width - width) / 2, 1, width, height);
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
                " {}{} · ↑↓ · Enter opens · Esc closes ",
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
    let mut state = ListState::default().with_selected((!p.items.is_empty()).then_some(p.selected));
    frame.render_stateful_widget(
        List::new(items).highlight_style(colors.highlight()),
        Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        ),
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
        .map(|i| i.label.chars().count() + i.detail.chars().count().min(30) + 4)
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
            ListItem::new(Line::from(vec![
                Span::raw(clean(&i.label)),
                Span::styled(format!("  {}", clean(&i.detail)), detail),
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

pub fn render_hover(frame: &mut Frame, s: &Snapshot) {
    let Some(h) = &s.hover else { return };
    let text = clean(&h.text);
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
        .min(12)
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

/// Problems, restricted mode and the diagnostic under the caret, for the
/// status line.
pub fn status_suffix(s: &Snapshot) -> String {
    let mut parts = Vec::new();
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
