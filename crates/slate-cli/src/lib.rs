use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    prelude::*,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use slate_core::{layout::Axis, terminal::Screen, App, Command, Key, Snapshot};
use std::{
    io::{self, Write},
    time::Duration,
};

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}
pub fn run(mut app: App) -> Result<()> {
    enable_raw_mode()?;
    let _guard = Guard;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut palette: Option<String> = None;
    let mut drag = None;
    let mut selecting = None;
    let mut clipboard = String::new();
    while !app.quit {
        let size = terminal.size()?;
        let snapshot = app.snapshot(size.width, size.height.saturating_sub(2), 1, 1, 1, 3);
        terminal.draw(|frame| render(frame, &snapshot, palette.as_deref()))?;
        if app.clipboard != clipboard {
            clipboard = app.clipboard.clone();
            if !clipboard.is_empty() {
                use base64::Engine;
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(clipboard.as_bytes());
                write!(io::stdout(), "\x1b]52;c;{encoded}\x07")?;
                io::stdout().flush()?;
            }
        }
        if !event::poll(Duration::from_millis(33))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                if let Some(line) = palette.as_mut() {
                    match k.code {
                        KeyCode::Esc => palette = None,
                        KeyCode::Enter => {
                            let command = line.clone();
                            palette = None;
                            app.command_line(&command);
                        }
                        KeyCode::Backspace => {
                            line.pop();
                        }
                        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                            line.push(c)
                        }
                        _ => {}
                    }
                } else if k.code == KeyCode::F(1)
                    || (k.code == KeyCode::Char('P') || k.code == KeyCode::Char('p'))
                        && k.modifiers
                            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                {
                    palette = Some(String::new());
                } else {
                    app.dispatch(Command::Key { key: translate(k) });
                }
            }
            Event::Paste(text) => {
                if let Some(line) = palette.as_mut() {
                    line.push_str(&text.replace(['\n', '\r'], " "));
                } else {
                    app.dispatch(Command::Paste { text });
                }
            }
            Event::Mouse(m) => {
                let x = m.column;
                let y = m.row;
                if matches!(m.kind, MouseEventKind::Down(_)) {
                    drag = snapshot
                        .handles
                        .iter()
                        .find(|h| inside(x, y, h.rect))
                        .cloned();
                    if drag.is_none() {
                        if let Some(pane) = snapshot.panes.iter().find(|p| inside(x, y, p.rect)) {
                            app.dispatch(Command::Focus { pane: pane.id });
                            if y == pane.rect.y + 1 {
                                let mut offset = pane.rect.x + 1;
                                for (index, tab) in pane.tabs.iter().enumerate() {
                                    let width = tab.title.chars().count() as u16 + 3;
                                    if x >= offset && x < offset + width {
                                        app.dispatch(Command::SwitchTab {
                                            pane: pane.id,
                                            index,
                                        });
                                        break;
                                    }
                                    offset += width;
                                }
                            } else if y > pane.rect.y + 1 {
                                if pane.kind == "editor" {
                                    selecting = Some(pane.id);
                                    app.dispatch(Command::Click {
                                        pane: pane.id,
                                        row: y.saturating_sub(pane.rect.y + 2) as usize,
                                        col: x.saturating_sub(pane.rect.x + 1) as usize,
                                        shift: m.modifiers.contains(KeyModifiers::SHIFT),
                                    });
                                } else if pane.kind == "files" || pane.kind == "git" {
                                    let len = if pane.kind == "files" {
                                        snapshot.files.len()
                                    } else {
                                        snapshot.git.len()
                                    };
                                    let height = pane.rect.height.saturating_sub(3) as usize;
                                    let top = pane
                                        .selected
                                        .saturating_sub(height.saturating_sub(1))
                                        .min(len.saturating_sub(height));
                                    let row = top + y.saturating_sub(pane.rect.y + 2) as usize;
                                    app.dispatch(Command::Click {
                                        pane: pane.id,
                                        row,
                                        col: 0,
                                        shift: false,
                                    });
                                    app.dispatch(Command::Key {
                                        key: Key {
                                            key: "Enter".into(),
                                            ..Default::default()
                                        },
                                    });
                                }
                            }
                        }
                    }
                } else if matches!(m.kind, MouseEventKind::Drag(_)) {
                    if let Some(h) = drag.as_ref() {
                        let ratio = if h.axis == Axis::Horizontal {
                            (x.saturating_sub(h.parent.x)) as f32 / h.parent.width.max(1) as f32
                        } else {
                            (y.saturating_sub(h.parent.y)) as f32 / h.parent.height.max(1) as f32
                        };
                        app.dispatch(Command::ResizeSplit { id: h.id, ratio });
                    } else if let Some(id) = selecting {
                        if let Some(p) = snapshot.panes.iter().find(|p| p.id == id) {
                            app.dispatch(Command::Click {
                                pane: id,
                                row: y.saturating_sub(p.rect.y + 2) as usize,
                                col: x.saturating_sub(p.rect.x + 1) as usize,
                                shift: true,
                            });
                        }
                    }
                } else if matches!(m.kind, MouseEventKind::Up(_)) {
                    drag = None;
                    selecting = None;
                } else if matches!(
                    m.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) {
                    if let Some(p) = snapshot.panes.iter().find(|p| inside(x, y, p.rect)) {
                        let delta = if m.kind == MouseEventKind::ScrollUp {
                            3
                        } else {
                            -3
                        };
                        app.dispatch(Command::Scroll { pane: p.id, delta });
                    }
                }
            }
            _ => {}
        }
    }
    terminal.show_cursor()?;
    Ok(())
}
fn inside(x: u16, y: u16, r: slate_core::layout::Rect) -> bool {
    x >= r.x && y >= r.y && x < r.x + r.width && y < r.y + r.height
}
pub fn translate(k: KeyEvent) -> Key {
    let (key, text) = match k.code {
        KeyCode::Char(c) => (c.to_string(), c.to_string()),
        KeyCode::Enter => ("Enter".into(), String::new()),
        KeyCode::Backspace => ("Backspace".into(), String::new()),
        KeyCode::Delete => ("Delete".into(), String::new()),
        KeyCode::Insert => ("Insert".into(), String::new()),
        KeyCode::Tab => ("Tab".into(), String::new()),
        KeyCode::BackTab => ("Tab".into(), String::new()),
        KeyCode::Esc => ("Escape".into(), String::new()),
        KeyCode::Up => ("Up".into(), String::new()),
        KeyCode::Down => ("Down".into(), String::new()),
        KeyCode::Left => ("Left".into(), String::new()),
        KeyCode::Right => ("Right".into(), String::new()),
        KeyCode::Home => ("Home".into(), String::new()),
        KeyCode::End => ("End".into(), String::new()),
        KeyCode::PageUp => ("PageUp".into(), String::new()),
        KeyCode::PageDown => ("PageDown".into(), String::new()),
        KeyCode::F(n) => (format!("F{n}"), String::new()),
        _ => (String::new(), String::new()),
    };
    Key {
        key,
        text,
        ctrl: k.modifiers.contains(KeyModifiers::CONTROL),
        alt: k.modifiers.contains(KeyModifiers::ALT),
        shift: k.modifiers.contains(KeyModifiers::SHIFT) || k.code == KeyCode::BackTab,
    }
}
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}
fn render(frame: &mut Frame, s: &Snapshot, palette: Option<&str>) {
    for p in &s.panes {
        let area = Rect::new(p.rect.x, p.rect.y, p.rect.width, p.rect.height);
        let focus = p.id == s.focus;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} #{} ", p.kind, p.id))
            .border_style(Style::default().fg(if focus { Color::Cyan } else { Color::DarkGray }));
        frame.render_widget(block, area);
        let tabs = p
            .tabs
            .iter()
            .map(|t| {
                Span::styled(
                    format!(" {} ", clean(&t.title)),
                    Style::default().fg(if t.active { Color::Cyan } else { Color::Gray }),
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(Line::from(tabs)),
            Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1),
        );
        let content = Rect::new(
            area.x + 1,
            area.y + 2,
            area.width.saturating_sub(2),
            area.height.saturating_sub(3),
        );
        if let Some(screen) = &p.screen {
            frame.render_widget(Grid(screen), content);
            if focus && palette.is_none() {
                if let Some((y, x)) = screen.cursor {
                    if x < content.width && y < content.height {
                        frame.set_cursor_position((content.x + x, content.y + y));
                    }
                }
            }
        } else {
            let items: Vec<ListItem> = if p.kind == "files" {
                s.files
                    .iter()
                    .map(|e| {
                        ListItem::new(clean(&format!(
                            "{} {}",
                            if e.directory { "▸" } else { " " },
                            e.name
                        )))
                    })
                    .collect()
            } else {
                s.git
                    .iter()
                    .map(|e| ListItem::new(clean(&format!("{} {}", e.status, e.path))))
                    .collect()
            };
            let mut state = ListState::default().with_selected(Some(p.selected));
            frame.render_stateful_widget(
                List::new(items).highlight_style(Style::default().bg(Color::DarkGray)),
                content,
                &mut state,
            );
        }
    }
    let size = frame.area();
    frame.render_widget(
        Paragraph::new(clean(&s.status))
            .style(Style::default().fg(Color::White).bg(Color::DarkGray)),
        Rect::new(0, size.height.saturating_sub(2), size.width, 1),
    );
    frame.render_widget(
        Paragraph::new("F1 commands · F6 pane · F7 tab · F8 terminal · F9 split · Ctrl-S save"),
        Rect::new(0, size.height.saturating_sub(1), size.width, 1),
    );
    if let Some(text) = palette {
        let area = Rect::new(
            1,
            size.height.saturating_sub(6),
            size.width.saturating_sub(2),
            5,
        );
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(format!("> {text}\n{}", slate_core::COMMAND_HELP))
                .wrap(ratatui::widgets::Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Command · Enter executes · Escape cancels"),
                ),
            area,
        );
        frame.set_cursor_position((
            area.x + 3 + (text.chars().count() as u16).min(area.width.saturating_sub(5)),
            area.y + 1,
        ));
    }
}
struct Grid<'a>(&'a Screen);
impl Widget for Grid<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (row, line) in self.0.cells.iter().take(area.height as usize).enumerate() {
            for (col, c) in line.iter().take(area.width as usize).enumerate() {
                let x = area.x + col as u16;
                let y = area.y + row as u16;
                let cell = &mut buf[(x, y)];
                cell.set_style(
                    Style::default()
                        .fg(parse_color(&c.fg))
                        .bg(parse_color(&c.bg))
                        .add_modifier(
                            if c.bold {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            } | if c.italic {
                                Modifier::ITALIC
                            } else {
                                Modifier::empty()
                            } | if c.underline {
                                Modifier::UNDERLINED
                            } else {
                                Modifier::empty()
                            },
                        ),
                );
                if !c.continuation {
                    cell.set_symbol(if c.text.is_empty() { " " } else { &c.text });
                }
            }
        }
    }
}
fn parse_color(s: &str) -> Color {
    let value = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
    Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}
