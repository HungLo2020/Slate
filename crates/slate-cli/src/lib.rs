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

struct InputReader {
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for InputReader {
    fn drop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct Guard;
// Rendering and mouse hit testing share terminal-column geometry, including
// wide Unicode filenames and the space reserved for each close button.
struct TabRegion {
    index: usize,
    x: u16,
    width: u16,
    close: Option<u16>,
}
fn tab_regions(tabs: &[slate_core::Tab], width: u16) -> Vec<TabRegion> {
    let widths: Vec<usize> = tabs
        .iter()
        .map(|tab| {
            Span::raw(clean(&tab.title)).width() + 2 + if tab.close_id.is_some() { 3 } else { 0 }
        })
        .collect();
    let crowded = widths.iter().sum::<usize>() > width as usize;
    let mut x = 0u16;
    let mut regions = Vec::new();
    for (index, tab) in tabs.iter().enumerate() {
        if crowded && !tab.active {
            continue;
        }
        let size = widths[index].min(width.saturating_sub(x) as usize) as u16;
        if size == 0 {
            break;
        }
        regions.push(TabRegion {
            index,
            x,
            width: size,
            close: (tab.close_id.is_some() && size >= 3).then(|| x + size - 3),
        });
        x += size;
    }
    regions
}
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
    let events = app.events();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let reader_running = running.clone();
    let input_events = events.clone();
    let thread = std::thread::spawn(move || {
        while reader_running.load(std::sync::atomic::Ordering::Acquire) {
            match event::poll(Duration::from_millis(100)) {
                Ok(false) => continue,
                Ok(true) => {
                    let result = event::read().map_err(|e| e.to_string());
                    let failed = result.is_err();
                    if input_tx.send(result).is_err() {
                        break;
                    }
                    input_events.notify();
                    if failed {
                        break;
                    }
                }
                Err(e) => {
                    let _ = input_tx.send(Err(e.to_string()));
                    input_events.notify();
                    break;
                }
            }
        }
    });
    let _reader = InputReader {
        running,
        thread: Some(thread),
    };
    let mut palette: Option<String> = None;
    let mut palette_index = 0usize;
    let mut redraw = true;
    let mut prior_revision = 0;
    let mut prior_size = terminal.size()?;
    let mut snapshot = app.snapshot(
        prior_size.width,
        prior_size.height.saturating_sub(2),
        1,
        1,
        1,
        3,
    );
    let mut drag = None;
    let mut selecting = None;
    let mut terminal_capture = None;
    let mut clipboard = String::new();
    while !app.quit {
        let observed = events.generation();
        app.process_events();
        let size = terminal.size()?;
        if app.revision() != prior_revision || size != prior_size {
            let revision = app.revision();
            snapshot = app.snapshot(size.width, size.height.saturating_sub(2), 1, 1, 1, 3);
            prior_revision = revision;
            prior_size = size;
            redraw = true;
        }
        let catalog = app.command_catalog(palette.as_deref().unwrap_or_default());
        if redraw {
            terminal.draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    palette.as_deref(),
                    palette_index,
                    &catalog,
                )
            })?;
            redraw = false;
        }
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
        let input = match input_rx.try_recv() {
            Ok(event) => event.map_err(|e| anyhow::anyhow!(e))?,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                events.wait(observed, Duration::from_secs(1));
                continue;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                anyhow::bail!("Terminal input disconnected")
            }
        };
        redraw = true;
        match input {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                if app.prompt.as_ref().is_some_and(|p| p.kind == "close-tab") {
                    app.dispatch(Command::Key { key: translate(k) });
                    continue;
                }
                if let Some(line) = palette.as_mut() {
                    match k.code {
                        KeyCode::Esc => palette = None,
                        KeyCode::Enter => {
                            let query = line.clone();
                            if let Some(command) = query.strip_prefix(':') {
                                palette = None;
                                app.command_line(command);
                            } else if let Some(action) =
                                catalog.get(palette_index).filter(|c| c.enabled)
                            {
                                let id = action.id.clone();
                                palette = None;
                                app.dispatch(Command::InvokeAction {
                                    id,
                                    argument: String::new(),
                                });
                            }
                        }
                        KeyCode::Up => palette_index = palette_index.saturating_sub(1),
                        KeyCode::Down => {
                            palette_index = (palette_index + 1).min(catalog.len().saturating_sub(1))
                        }
                        KeyCode::Backspace => {
                            line.pop();
                            palette_index = 0;
                        }
                        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                            line.push(c);
                            palette_index = 0;
                        }
                        _ => {}
                    }
                } else if k.code == KeyCode::F(1)
                    || (k.code == KeyCode::Char('P') || k.code == KeyCode::Char('p'))
                        && k.modifiers
                            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                {
                    palette = Some(String::new());
                    palette_index = 0;
                } else {
                    app.dispatch(Command::Key { key: translate(k) });
                }
            }
            Event::Paste(text) => {
                if app.prompt.as_ref().is_some_and(|p| p.kind == "close-tab") {
                    continue;
                }
                if let Some(line) = palette.as_mut() {
                    line.push_str(&text.replace(['\n', '\r'], " "));
                } else {
                    app.dispatch(Command::Paste { text });
                }
            }
            Event::Mouse(m) => {
                if app.prompt.is_some() || palette.is_some() {
                    continue;
                }
                let x = m.column;
                let y = m.row;
                let target = snapshot.panes.iter().find(|p| {
                    p.kind == "terminal"
                        && (terminal_capture == Some(p.id)
                            || inside(x, y, p.rect) && y > p.rect.y + 1)
                });
                if drag.is_none() {
                    if let Some(p) = target {
                        let (kind, button) = match m.kind {
                            MouseEventKind::Down(b) => ("press", b),
                            MouseEventKind::Up(b) => ("release", b),
                            MouseEventKind::Drag(b) => ("drag", b),
                            MouseEventKind::Moved => ("move", event::MouseButton::Left),
                            MouseEventKind::ScrollUp => ("wheel_up", event::MouseButton::Left),
                            MouseEventKind::ScrollDown => ("wheel_down", event::MouseButton::Left),
                            _ => continue,
                        };
                        if kind == "press" {
                            terminal_capture = Some(p.id);
                        } else if kind == "release" {
                            terminal_capture = None;
                        }
                        let button = if kind == "move" {
                            3
                        } else {
                            match button {
                                event::MouseButton::Left => 0,
                                event::MouseButton::Middle => 1,
                                event::MouseButton::Right => 2,
                            }
                        };
                        app.dispatch(Command::Pointer {
                            pane: p.id,
                            row: y.saturating_sub(p.rect.y + 2) as usize,
                            col: x.saturating_sub(p.rect.x + 1) as usize,
                            kind: kind.into(),
                            button,
                            shift: m.modifiers.contains(KeyModifiers::SHIFT),
                            ctrl: m.modifiers.contains(KeyModifiers::CONTROL),
                            alt: m.modifiers.contains(KeyModifiers::ALT),
                        });
                        continue;
                    }
                }
                if matches!(m.kind, MouseEventKind::Down(_)) {
                    drag = snapshot
                        .handles
                        .iter()
                        .find(|h| inside(x, y, h.rect))
                        .cloned();
                    if drag.is_none() {
                        if let Some(pane) = snapshot.panes.iter().find(|p| inside(x, y, p.rect)) {
                            if y == pane.rect.y + 1 {
                                let column = x.saturating_sub(pane.rect.x + 1);
                                for region in
                                    tab_regions(&pane.tabs, pane.rect.width.saturating_sub(2))
                                {
                                    if column >= region.x && column < region.x + region.width {
                                        if region.close.is_some_and(|close| column >= close)
                                            && m.kind
                                                == MouseEventKind::Down(event::MouseButton::Left)
                                        {
                                            if let Some(view) = pane.tabs[region.index].close_id {
                                                app.dispatch(Command::CloseTab {
                                                    pane: pane.id,
                                                    view,
                                                    force: false,
                                                });
                                            }
                                        } else {
                                            app.dispatch(Command::SwitchTab {
                                                pane: pane.id,
                                                index: region.index,
                                            });
                                        }
                                        break;
                                    }
                                }
                            } else if y > pane.rect.y + 1 {
                                app.dispatch(Command::Focus { pane: pane.id });
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
fn render(
    frame: &mut Frame,
    s: &Snapshot,
    palette: Option<&str>,
    palette_index: usize,
    catalog: &[slate_core::commands::CommandInfo],
) {
    for p in &s.panes {
        let area = Rect::new(p.rect.x, p.rect.y, p.rect.width, p.rect.height);
        let focus = p.id == s.focus;
        let block = Block::default()
            .borders(Borders::ALL)
            .style(
                Style::default()
                    .bg(parse_color(&s.background))
                    .fg(parse_color(&s.foreground)),
            )
            .title(if p.kind == "git" {
                format!(
                    " Git: {} #{} ",
                    if s.git_repository {
                        &s.git_branch
                    } else {
                        "No repository"
                    },
                    p.id
                )
            } else {
                format!(" {} #{} ", p.kind, p.id)
            })
            .border_style(Style::default().fg(if focus { Color::Cyan } else { Color::DarkGray }));
        frame.render_widget(block, area);
        for region in tab_regions(&p.tabs, area.width.saturating_sub(2)) {
            let tab = &p.tabs[region.index];
            let label_width =
                region
                    .width
                    .saturating_sub(if region.close.is_some() { 3 } else { 0 });
            frame.render_widget(
                Paragraph::new(format!(" {} ", clean(&tab.title)))
                    .style(Style::default().fg(if tab.active { Color::Cyan } else { Color::Gray })),
                Rect::new(area.x + 1 + region.x, area.y + 1, label_width, 1),
            );
            if let Some(close) = region.close {
                frame.render_widget(
                    Paragraph::new("[x]").style(Style::default().fg(Color::Gray)),
                    Rect::new(area.x + 1 + close, area.y + 1, 3, 1),
                );
            }
        }
        let content = Rect::new(
            area.x + 1,
            area.y + 2,
            area.width.saturating_sub(2),
            area.height.saturating_sub(3),
        );
        if let Some(screen) = &p.screen {
            frame.render_widget(Grid(screen), content);
            if focus && palette.is_none() && s.prompt.is_none() {
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
                    .map(|e| {
                        ListItem::new(clean(&format!("[{}] {} {}", e.group, e.status, e.path)))
                    })
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
        Paragraph::new(clean(&format!("{} · {}", s.status, s.location)))
            .style(Style::default().fg(Color::White).bg(Color::DarkGray)),
        Rect::new(0, size.height.saturating_sub(2), size.width, 1),
    );
    frame.render_widget(
        Paragraph::new(s.hints.as_str()),
        Rect::new(0, size.height.saturating_sub(1), size.width, 1),
    );
    if let Some(prompt) = &s.prompt {
        let height = if prompt.kind == "settings" {
            (s.settings.menu_items().len() as u16 + 4).min(size.height.saturating_sub(2))
        } else if prompt.kind == "replace" {
            8
        } else {
            6
        };
        let area = Rect::new(
            1,
            size.height.saturating_sub(height + 1),
            size.width.saturating_sub(2),
            height,
        );
        frame.render_widget(Clear, area);
        if prompt.kind == "settings" {
            let items = s
                .settings
                .menu_items()
                .into_iter()
                .map(|(label, value)| ListItem::new(format!("{label}: {value}")))
                .collect::<Vec<_>>();
            frame.render_widget(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Settings · saved automatically"),
                area,
            );
            let mut state = ListState::default().with_selected(Some(prompt.field));
            frame.render_stateful_widget(
                List::new(items).highlight_style(Style::default().bg(Color::DarkGray)),
                Rect::new(
                    area.x + 1,
                    area.y + 1,
                    area.width.saturating_sub(2),
                    area.height.saturating_sub(4),
                ),
                &mut state,
            );
            frame.render_widget(
                Paragraph::new("↑↓ select · ←→ / Enter change · Escape closes"),
                Rect::new(
                    area.x + 1,
                    area.y + area.height.saturating_sub(2),
                    area.width.saturating_sub(2),
                    1,
                ),
            );
        } else {
            let text = if prompt.kind == "close-tab" {
                format!("Discard unsaved changes in {}?\nD: discard and close · Escape / Enter: cancel\nCancel to save the document first.", clean(&prompt.input))
            } else if prompt.kind == "replace" {
                format!("Find: {}\nWith: {}\nTab switches fields · Enter replaces next · Ctrl-Enter replaces all\nEscape closes",clean(&prompt.input),clean(&prompt.replacement))
            } else {
                format!("> {}\nEnter confirms · Escape closes", clean(&prompt.input))
            };
            let text = if ["find", "replace"].contains(&prompt.kind.as_str()) {
                format!(
                    "{text}\nAlt-C case: {} · Alt-W whole word: {}",
                    if prompt.case_sensitive { "on" } else { "off" },
                    if prompt.whole_word { "on" } else { "off" }
                )
            } else {
                text
            };
            frame.render_widget(
                Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(
                    match prompt.kind.as_str() {
                        "open" => "Open file or directory",
                        "save-as" => "Save to a new file path",
                        "close-tab" => "Unsaved changes",
                        "commit" => "Commit message",
                        "settings" => "Settings: option value (indent-width, theme, line-numbers…)",
                        "layout-save" => "Save layout name",
                        "layout-load" => "Load layout name",
                        "move-pane" => "Target pane number",
                        kind => kind,
                    },
                )),
                area,
            );
            let value = if prompt.field == 0 {
                &prompt.input
            } else {
                &prompt.replacement
            };
            let prefix = if prompt.kind == "replace" { 6 } else { 3 };
            frame.set_cursor_position((
                area.x
                    + (prefix + slate_core::document::display_width(value) as u16)
                        .min(area.width.saturating_sub(2)),
                area.y + 1 + prompt.field as u16,
            ));
        }
    }
    if let Some(text) = palette {
        let height = size.height.saturating_sub(4).min(16);
        let area = Rect::new(
            1,
            size.height.saturating_sub(height + 1),
            size.width.saturating_sub(2),
            height,
        );
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title("Commands · ↑↓ select · Enter runs · Escape closes"),
            area,
        );
        frame.render_widget(
            Paragraph::new(format!("> {}", clean(text))),
            Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1),
        );
        let items = catalog
            .iter()
            .map(|c| {
                ListItem::new(format!(
                    "{}  {}\n  {}",
                    c.name,
                    c.shortcut,
                    if c.enabled { &c.description } else { &c.reason }
                ))
                .style(Style::default().fg(if c.enabled {
                    Color::White
                } else {
                    Color::DarkGray
                }))
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default().with_selected(Some(palette_index));
        frame.render_stateful_widget(
            List::new(items).highlight_style(Style::default().bg(Color::DarkGray)),
            Rect::new(
                area.x + 1,
                area.y + 2,
                area.width.saturating_sub(2),
                area.height.saturating_sub(3),
            ),
            &mut state,
        );
        if catalog.is_empty() {
            frame.render_widget(
                Paragraph::new(if text.starts_with(':') {
                    "Enter executes the typed command"
                } else {
                    "No matching actions"
                }),
                Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1),
            );
        }
        frame.set_cursor_position((
            area.x
                + 3
                + (slate_core::document::display_width(text) as u16)
                    .min(area.width.saturating_sub(5)),
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
