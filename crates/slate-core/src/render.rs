//! Presentation: snapshots for the frontends and the drawn editor screen
//! (text, selections, carets, search matches, diagnostics, folds and
//! debugger marks).
use super::*;

impl App {
    pub fn snapshot(
        &mut self,
        width: u16,
        height: u16,
        gap: u16,
        cell_width: u16,
        cell_height: u16,
        header: u16,
    ) -> Snapshot {
        self.snapshot_with_minimum(
            Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            gap,
            (cell_width, cell_height),
            header,
            (0, 0),
        )
    }
    pub fn snapshot_with_minimum(
        &mut self,
        area: Rect,
        gap: u16,
        cell: (u16, u16),
        header: u16,
        minimum: (u16, u16),
    ) -> Snapshot {
        self.snapshot_presentation(area, gap, cell, header, minimum, None, false)
    }
    pub fn gui_snapshot(
        &mut self,
        area: Rect,
        gap: u16,
        cell: (u16, u16),
        header: u16,
        minimum: (u16, u16),
        known: Option<(u64, u64)>,
    ) -> Snapshot {
        self.snapshot_presentation(area, gap, cell, header, minimum, known, true)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn snapshot_presentation(
        &mut self,
        area: Rect,
        gap: u16,
        cell: (u16, u16),
        header: u16,
        minimum: (u16, u16),
        known: Option<(u64, u64)>,
        graphical: bool,
    ) -> Snapshot {
        let Rect { width, height, .. } = area;
        let (cell_width, cell_height) = cell;
        self.process_events();
        self.render_cache
            .retain(|id, _| self.views.contains_key(id) || self.terminals.contains_key(id));
        let cw = cell_width.max(1);
        let ch = cell_height.max(1);
        let required = self.layout.minimum_size(gap, minimum);
        let (placements, handles) = if self.editor_only
            || width / cw < 70
            || height / ch < 10
            || width < required.0
            || height < required.1
        {
            (
                vec![layout::Placement {
                    id: self.focus,
                    rect: area,
                }],
                vec![],
            )
        } else {
            self.layout.arrange_constrained(area, gap, minimum)
        };
        let mut panes = vec![];
        for placement in placements {
            let id = placement.id;
            let r = placement.rect;
            let Some(view) = self.layout.view(id).cloned() else {
                continue;
            };
            let tabs = match self.layout.pane_mut(id) {
                Some((tabs, active)) => {
                    let active = *active;
                    tabs.clone()
                        .iter()
                        .enumerate()
                        .map(|(index, v)| Tab {
                            title: match v {
                                View::Files => "Files".into(),
                                View::Git => "Git".into(),
                                View::Editor(id) => self
                                    .views
                                    .get(id)
                                    .and_then(|v| self.documents.get(&v.document))
                                    .map(Document::title)
                                    .unwrap_or_else(|| "Editor".into()),
                                View::Terminal(id) => self
                                    .terminals
                                    .get(id)
                                    .and_then(TerminalSession::program_title)
                                    .unwrap_or_else(|| format!("Terminal {id}")),
                            },
                            active: index == active,
                            editor_id: match v {
                                View::Editor(id) => Some(*id),
                                _ => None,
                            },
                            close_id: match v {
                                View::Editor(id) | View::Terminal(id) => Some(*id),
                                _ => None,
                            },
                        })
                        .collect()
                }
                None => vec![],
            };
            let rows = r
                .height
                .saturating_sub(header)
                .checked_div(ch)
                .unwrap_or(1)
                .max(1);
            let cols = r
                .width
                .saturating_sub(2)
                .checked_div(cw)
                .unwrap_or(1)
                .max(1);
            let mut text = None;
            let (kind, screen, selected) = match view {
                View::Editor(view) => {
                    if graphical {
                        text = Some(self.editor_text(view, rows, cols));
                        ("editor", None, 0)
                    } else {
                        let screen = self.cached_editor_screen(view, rows, cols);
                        ("editor", Some(screen), 0)
                    }
                }
                View::Terminal(term) => {
                    let screen = self.terminals.get_mut(&term).map(|t| {
                        let _ = t.resize(rows, cols);
                        // The signature is checked below before the parser grid is copied.
                        t.revision()
                    });
                    let screen = screen.map(|revision| self.cached_terminal_screen(term, revision));
                    ("terminal", screen, 0)
                }
                View::Files => ("files", None, self.selected),
                View::Git => ("git", None, self.git.selected),
            };
            panes.push(PaneSnapshot {
                id,
                rect: r,
                kind: kind.into(),
                tabs,
                revision: self
                    .render_cache
                    .get(&match view {
                        View::Editor(v) | View::Terminal(v) => v,
                        _ => 0,
                    })
                    .map(|(_, revision, _)| *revision)
                    .unwrap_or(0),
                focused: id == self.focus,
                terminal_mouse_motion: match view {
                    View::Terminal(v) => self.terminals[&v].mouse_motion(),
                    _ => false,
                },
                read_only: match view {
                    View::Editor(v) => self.documents[&self.views[&v].document].read_only,
                    _ => false,
                },
                editor: match view {
                    View::Editor(v) => Some(self.editor_presentation(v)),
                    _ => None,
                },
                screen,
                selected,
                rows,
                cols,
                text,
            });
        }
        Snapshot {
            panes,
            handles,
            focus: self.focus,
            files: if known.is_some_and(|(files, _)| files == self.files_revision) {
                Default::default()
            } else {
                let (revision, files) = &mut self.shared_files;
                if *revision != self.files_revision {
                    *revision = self.files_revision;
                    *files = std::sync::Arc::new(self.files.clone());
                }
                files.clone()
            },
            git: if known.is_some_and(|(_, git)| git == self.git.revision) {
                Default::default()
            } else {
                let (revision, entries) = &mut self.shared_git;
                if *revision != self.git.revision {
                    *revision = self.git.revision;
                    *entries = std::sync::Arc::new(self.git.entries.clone());
                }
                entries.clone()
            },
            browser: self.browser.to_string_lossy().into_owned(),
            git_root: self.git.top.clone(),
            search: self.search.clone(),
            status: self.visible_status(),
            dirty: self.dirty(),
            quit: self.quit,
            layouts: self.presets.keys().cloned().collect(),
            prompt: self.prompt.clone(),
            location: self.location(),
            hints: self.hints(),
            foreground: self.colors.0.clone(),
            terminal_background: self.terminal_colors.background.clone(),
            terminal_foreground: self.terminal_colors.foreground.clone(),
            terminal_selection: self.terminal_colors.selection.clone(),
            background: self.colors.1.clone(),
            accent: self.colors.3.clone(),
            selection: self.colors.2.clone(),
            settings: self.preferences.clone(),
            profiles: self.preference_layers.names.clone(),
            setting_sources: self.preference_sources(),
            editor_only: self.editor_only,
            title: self.window_title(),
            recent: self.recent.clone(),
            recent_projects: self.recent_projects.clone(),
            project_search_policy: self.project_search_policy.clone(),
            git_branch: self.git.branch.clone(),
            git_repository: self.git.repository,
            git_busy: self.git.jobs > 0,
            git_error: self.git.error.clone(),
            files_revision: self.files_revision,
            git_revision: self.git.revision,
            picker: self.picker_view(),
            trusted: self.trusted,
            git_restricted: self.git.restricted,
            completion: self.completion_view(),
            hover: self.hover_view(),
            problems: self.problem_counts(),
            problem: self.problem_here().unwrap_or_default(),
            activity: self.activity(),
            debugging: self.debugging(),
            debug_paused: self.debug_paused(),
        }
    }
    pub fn editor_input_context(&self, pane: u64) -> Option<EditorPresentation> {
        match self.layout.view(pane) {
            Some(View::Editor(id)) if self.views.contains_key(id) => {
                Some(self.editor_presentation(*id))
            }
            _ => None,
        }
    }
    pub(crate) fn editor_presentation(&self, id: u64) -> EditorPresentation {
        let v = &self.views[&id];
        let doc = &self.documents[&v.document];
        let start = doc.next_char_boundary_from(v.cursor.saturating_sub(2048));
        let end = doc.floor_boundary((v.cursor + 2048).min(doc.len()));
        let anchor = v.anchor.unwrap_or(v.cursor).clamp(start, end);
        let selected = v
            .anchor
            .map(|a| {
                let (a, b) = (a.min(v.cursor), a.max(v.cursor));
                doc.slice(a, b.min(a + 8192)).into_owned()
            })
            .unwrap_or_default();
        EditorPresentation {
            surrounding: doc.slice(start, end).into(),
            cursor: doc.slice(start, v.cursor).encode_utf16().count(),
            anchor: doc.slice(start, anchor).encode_utf16().count(),
            selection: selected.chars().take(2048).collect(),
            line_count: doc.line_count(),
            top: v.top,
            left: v.left,
            document: v.document,
            generation: doc.generation,
            overview_revision: self.overview_revision,
        }
    }
    /// A digest of everything an editor pane's screen is drawn from, taken
    /// every frame, so it hashes the fields rather than serializing them.
    pub(crate) fn editor_signature(&self, id: u64) -> u64 {
        use std::hash::{Hash, Hasher};
        let v = &self.views[&id];
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            v.document,
            v.cursor,
            v.anchor,
            v.top,
            v.left,
            v.rows,
            v.cols,
            v.manual_scroll,
            v.top_row,
            v.mark,
            &v.folds,
        )
            .hash(&mut h);
        v.extra.len().hash(&mut h);
        for caret in &v.extra {
            (caret.cursor, caret.anchor).hash(&mut h);
        }
        let preferences = self.preferences_for_document(v.document);
        let d = &self.documents[&v.document];
        // Diagnostics are found by path, so a Save As redraws them.
        (&d.path, self.diff_inspections.contains_key(&v.document)).hash(&mut h);
        (
            self.lsp_revision(),
            self.debug_marks(v.document),
            d.generation,
            self.highlights.get(&v.document).map(|c| c.revision),
            &self.colors,
            &self.selection_foreground,
            (
                &self.search.query,
                self.search.case_sensitive,
                self.search.whole_word,
                self.search.regex_mode,
                self.search.selection_only,
                self.search.scope,
            ),
            self.preferences.line_numbers,
            preferences.tab_width,
            preferences.soft_wrap,
            self.preferences.show_whitespace,
        )
            .hash(&mut h);
        h.finish()
    }
    pub(crate) fn cached_editor_screen(
        &mut self,
        id: u64,
        rows: u16,
        cols: u16,
    ) -> std::sync::Arc<Screen> {
        let v = self.views.get_mut(&id).unwrap();
        v.rows = rows;
        v.cols = cols;
        let key = self.editor_signature(id);
        if let Some((prior, _, screen)) = self.render_cache.get(&id) {
            if *prior == key {
                return screen.clone();
            }
        }
        let screen = std::sync::Arc::new(self.render_editor(id, rows, cols, false).0);
        self.screen_builds += 1;
        // Rendering may scroll the view; key the screen by the view it shows.
        let key = self.editor_signature(id);
        self.render_cache
            .insert(id, (key, self.screen_builds, screen.clone()));
        screen
    }
    pub(crate) fn cached_terminal_screen(
        &mut self,
        id: u64,
        revision: u64,
    ) -> std::sync::Arc<Screen> {
        let key = {
            use std::hash::{Hash, Hasher};
            let colors = &self.terminal_colors;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (
                revision,
                &colors.foreground,
                &colors.background,
                &colors.selection,
                &colors.selection_foreground,
                &colors.accent,
                &colors.ansi,
            )
                .hash(&mut h);
            h.finish()
        };
        if let Some((prior, _, screen)) = self.render_cache.get(&id) {
            if *prior == key {
                return screen.clone();
            }
        }
        let screen =
            std::sync::Arc::new(self.terminals[&id].screen_with_palette(&self.terminal_colors));
        self.screen_builds += 1;
        self.render_cache
            .insert(id, (key, self.screen_builds, screen.clone()));
        screen
    }
    pub(crate) fn render_editor(
        &mut self,
        id: u64,
        rows: u16,
        cols: u16,
        graphical: bool,
    ) -> (Screen, Vec<Vec<text_presentation::TextSpan>>) {
        let options = self.preferences_for_document(self.views[&id].document);
        let tab = options.tab_width;
        let wrapping = options.soft_wrap;
        {
            let v = self.views.get_mut(&id).unwrap();
            v.rows = rows;
            v.cols = cols;
        }
        let doc_id = self.views[&id].document;
        let gutter = self.gutter_width(doc_id, cols);
        let usable = (cols as usize).saturating_sub(gutter).max(1);
        self.ensure_visible(id);
        let (cursor_line, cursor_row, cursor_col) =
            self.visual_position(id, self.views[&id].cursor);
        if !wrapping {
            // Horizontal scrolling keeps the cursor column visible, unless the
            // user scrolled away deliberately (wheel or scrollbar).
            let v = self.views.get_mut(&id).unwrap();
            if v.manual_scroll {
            } else if cursor_col < v.left {
                v.left = cursor_col;
            } else if cursor_col >= v.left + usable {
                v.left = cursor_col + 1 - usable;
            }
        } else {
            self.views.get_mut(&id).unwrap().left = 0;
        }
        // Graphical views draw one extra row for pixel-smooth scrolling.
        let render_rows = rows as usize + usize::from(graphical);
        let visible = self.visible_rows(id, render_rows);
        let show_whitespace = self.preferences.show_whitespace;
        let whitespace_color = desktop::blend(&self.colors.0, &self.colors.1, 0.6);
        // Fold state is computed before borrowing the view for drawing.
        let folded: Vec<bool> = visible
            .iter()
            .map(|(line, index, _)| *index == 0 && self.folded_header(id, *line))
            .collect();
        let foldable: Vec<bool> = {
            let rope = self.documents[&doc_id].rope();
            visible
                .iter()
                .map(|(line, index, _)| *index == 0 && smart::foldable(rope, *line, tab))
                .collect()
        };
        let v = &self.views[&id];
        let document = &self.documents[&doc_id];
        let left = if wrapping { 0 } else { v.left };
        let selection = v.anchor.map(|a| (a.min(v.cursor), a.max(v.cursor)));
        // Further carets and their selections, sorted for binary search.
        let mut extra_carets: Vec<usize> = v.extra.iter().map(|s| s.cursor).collect();
        extra_carets.sort_unstable();
        let mut extra_selections: Vec<(usize, usize)> = v
            .extra
            .iter()
            .map(|s| s.range())
            .filter(|(a, b)| a < b)
            .collect();
        extra_selections.sort_unstable();
        let in_extra = |offset: usize| {
            let i = extra_selections.partition_point(|(_, b)| *b <= offset);
            extra_selections.get(i).is_some_and(|(a, _)| *a <= offset)
        };
        let brackets = (selection.is_none() && v.extra.is_empty())
            .then(|| smart::matching_bracket(document.rope(), v.cursor))
            .flatten();
        let bracket_bg = desktop::blend(&self.colors.3, &self.colors.1, 0.35);
        // Language-server and task diagnostics: underlined ranges in the
        // severity colour and a mark in the gutter.
        let (breakpoints, paused_line) = self.debug_marks(doc_id);
        let paused_bg = desktop::blend(&self.colors.3, &self.colors.1, 0.18);
        let problems = self.diagnostic_spans(doc_id);
        let problem_lines = self.diagnostic_lines(doc_id);
        let light = self.light_theme();
        let severity_color = |severity: u8| -> String {
            match (severity, light) {
                (1, false) => "#ef6b73",
                (1, true) => "#c4242e",
                (2, false) => "#e5c07b",
                (2, true) => "#9a6700",
                (_, false) => "#7fb2e5",
                (_, true) => "#2264a8",
            }
            .into()
        };
        // Spans are sorted by start and may nest; the running maximum of
        // their ends bounds how far back an enclosing span can begin.
        let reach: Vec<usize> = problems
            .iter()
            .scan(0, |max, (_, end, _)| {
                *max = (*max).max(*end);
                Some(*max)
            })
            .collect();
        let problem_at = |offset: usize| -> Option<u8> {
            let j = problems.partition_point(|(start, _, _)| *start <= offset);
            let mut best: Option<u8> = None;
            for k in (0..j).rev() {
                if reach[k] <= offset {
                    break;
                }
                let (_, end, severity) = problems[k];
                if offset < end {
                    best = Some(best.map_or(severity, |b| b.min(severity)));
                }
            }
            best
        };
        let caret_fg = self.colors.1.clone();
        let accent = self.colors.3.clone();
        let mut blank = Cell::text(" ");
        blank.fg = self.colors.0.clone();
        blank.bg = self.colors.1.clone();
        let mut cells = if graphical {
            vec![vec![]; render_rows]
        } else {
            vec![vec![blank; cols as usize]; rows as usize]
        };
        let mut decorations = vec![vec![]; render_rows];
        let matches = self.search.regex().ok();
        let match_margin = self.search.query.len() * 4 + 4;
        let mut cursor = None;
        for (y, (line, row_index, row)) in visible.iter().enumerate() {
            let (line_start_byte, line_end_byte) = document.line_range(*line);
            let line_slice = document.rope().byte_slice(line_start_byte..line_end_byte);
            let spans = self.line_spans(doc_id, *line).map(|(spans, _)| spans);
            let spans: &[highlight::Span] = spans.as_deref().map_or(&[], |s| s.as_slice());
            if graphical {
                cells[y].resize_with(gutter, || Cell::text(" "));
            }
            if gutter > 0 {
                let number = if row.start == 0 {
                    format!(
                        "{:>width$}{} ",
                        line + 1,
                        if folded[y] { '▸' } else { ' ' },
                        width = gutter.saturating_sub(2)
                    )
                } else {
                    " ".repeat(gutter)
                };
                for (x, c) in number.chars().take(gutter).enumerate() {
                    cells[y][x] = Cell::text(c.to_string());
                    cells[y][x].fg = self.colors.0.clone();
                    cells[y][x].bg = self.colors.1.clone();
                }
                // One mark per line: paused here, breakpoint, fold, problem.
                if row.start == 0 && gutter >= 2 {
                    let mark = &mut cells[y][gutter - 2];
                    if paused_line == Some(*line) {
                        mark.text = "▶".into();
                        mark.fg = self.colors.3.clone();
                    } else if breakpoints.contains(line) {
                        // A diamond, so breakpoints never look like errors.
                        mark.text = "◆".into();
                        mark.fg = severity_color(1);
                    } else if let (Some(severity), false) = (problem_lines.get(line), folded[y]) {
                        mark.text = "●".into();
                        mark.fg = severity_color(*severity);
                    } else if !folded[y] && foldable[y] {
                        // Click to fold.
                        mark.text = "▾".into();
                        mark.fg = whitespace_color.clone();
                    }
                }
            }
            // The first byte to draw: the row start, or the horizontal scroll column.
            let first = if wrapping {
                row.start
            } else {
                document::slice_column_offset(line_slice, left, tab)
            };
            let mut x = if wrapping {
                row.col
            } else {
                document::slice_width(line_slice.byte_slice(..first), tab)
            };
            let origin = if wrapping { row.col } else { left };
            let segment_end = if wrapping {
                row.end
            } else {
                line_slice.len_bytes()
            };
            // Only the part of a line that can be visible is copied out of the
            // rope (plus context for search matches), so long lines stay cheap.
            let floor = |b: usize| line_slice.char_to_byte(line_slice.byte_to_char(b));
            let ceil = |b: usize| {
                let f = floor(b);
                if f < b {
                    line_slice.char_to_byte(line_slice.byte_to_char(f) + 1)
                } else {
                    f
                }
            };
            let window_start = floor(first.saturating_sub(match_margin));
            let window_end = ceil((first + usable * 8 + 64 + match_margin).min(segment_end));
            let window = line_slice.byte_slice(window_start..window_end).to_string();
            let ranges = matches
                .as_ref()
                .map(|r| {
                    r.find_iter(&window)
                        .map(|m| (window_start + m.start(), window_start + m.end()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut match_index = 0;
            let mut token_index = spans.partition_point(|t| (t.end as usize) <= first);
            let visible_text =
                &window[first - window_start..window.len().min(segment_end - window_start)];
            for (offset_in_segment, g) in visible_text.grapheme_indices(true) {
                let i = first + offset_in_segment;
                if g == "\r" || g == "\r\n" {
                    continue;
                }
                let width = document::grapheme_width(g, x, tab);
                for offset in 0..width {
                    let column = x + offset;
                    if column >= origin && column - origin + gutter < cols as usize {
                        let mut c = Cell::text(if g == "\t" || offset > 0 {
                            " ".to_string()
                        } else {
                            g.chars()
                                .map(|c| if c.is_control() { '\u{fffd}' } else { c })
                                .collect()
                        });
                        c.fg = self.colors.0.clone();
                        c.bg = self.colors.1.clone();
                        let absolute = line_start_byte + i;
                        while token_index < spans.len() && (spans[token_index].end as usize) <= i {
                            token_index += 1;
                        }
                        if let Some(span) = spans.get(token_index).filter(|t| t.start as usize <= i)
                        {
                            c.fg = format!("#{:06x}", span.fg);
                            c.bold = span.bold;
                            c.italic = span.italic;
                        }
                        if self.diff_inspections.contains_key(&doc_id) {
                            let prefix = line_slice.chars().next();
                            if prefix == Some('+') {
                                c.fg = "#268b47".into();
                            }
                            if prefix == Some('-') {
                                c.fg = "#d14b4b".into();
                            }
                        }
                        while match_index < ranges.len() && ranges[match_index].1 <= i {
                            match_index += 1;
                        }
                        let matched = ranges
                            .get(match_index)
                            .is_some_and(|(a, b)| i >= *a && i < *b);
                        if show_whitespace && (g == " " || g == "\t") {
                            c.text = match (g, offset) {
                                (" ", _) => "·",
                                (_, 0) => "→",
                                _ => " ",
                            }
                            .into();
                            c.fg = whitespace_color.clone();
                        }
                        c.wide = width == 2 && g != "\t" && offset == 0;
                        c.continuation = width == 2 && g != "\t" && offset == 1;
                        let selected = selection
                            .map(|(a, b)| absolute >= a && absolute < b)
                            .unwrap_or(false)
                            || (!extra_selections.is_empty() && in_extra(absolute));
                        let caret = !extra_carets.is_empty()
                            && offset == 0
                            && extra_carets.binary_search(&absolute).is_ok();
                        let bracket = offset == 0
                            && brackets.is_some_and(|(a, b)| absolute == a || absolute == b);
                        let position = column - origin + gutter;
                        if paused_line == Some(*line) {
                            c.bg = paused_bg.clone();
                        }
                        if !problems.is_empty() {
                            if let Some(severity) = problem_at(absolute) {
                                c.fg = severity_color(severity);
                                c.underline = true;
                            }
                        }
                        let style = if caret {
                            Some((accent.clone(), caret_fg.clone(), false))
                        } else if matched || selected {
                            Some((
                                self.colors.2.clone(),
                                self.selection_foreground.clone(),
                                matched,
                            ))
                        } else if bracket {
                            Some((bracket_bg.clone(), c.fg.clone(), false))
                        } else {
                            None
                        };
                        if let Some((bg, fg, underline)) = style {
                            if graphical {
                                let mut decoration = c.clone();
                                decoration.bg = bg;
                                decoration.fg = fg;
                                decoration.underline = underline;
                                let row: &mut Vec<text_presentation::TextSpan> =
                                    &mut decorations[y];
                                if let Some(last) = row.last_mut().filter(|s| {
                                    s.start + s.length == position
                                        && s.underline == underline
                                        && s.bg == decoration.bg
                                }) {
                                    last.length += 1;
                                } else {
                                    row.push(text_presentation::TextSpan::cell(
                                        position,
                                        1,
                                        &decoration,
                                    ));
                                }
                            } else {
                                c.bg = bg;
                                c.fg = fg;
                                c.underline = underline;
                            }
                        }
                        if graphical && cells[y].len() <= position {
                            cells[y].resize_with(position + 1, || Cell::text(" "));
                        }
                        cells[y][position] = c;
                    }
                }
                x += width;
                if x >= origin + usable {
                    break;
                }
            }
            // Past the last character: a caret at the end of the line, and
            // the marker of a folded region.
            let end_of_text = line_start_byte + segment_end;
            let mut trailing: Vec<Cell> = Vec::new();
            if x >= origin && x < origin + usable {
                let at_end = extra_carets.binary_search(&end_of_text).is_ok()
                    || (end_of_text > 0
                        && document.rope().byte(end_of_text - 1) == b'\r'
                        && extra_carets.binary_search(&(end_of_text - 1)).is_ok());
                if at_end && (!wrapping || row.end == line_slice.len_bytes()) {
                    let mut c = Cell::text(" ");
                    c.bg = accent.clone();
                    c.fg = caret_fg.clone();
                    trailing.push(c);
                }
                if folded[y] {
                    for glyph in [" ", "⋯"] {
                        let mut c = Cell::text(glyph);
                        c.fg = accent.clone();
                        c.bg = self.colors.1.clone();
                        trailing.push(c);
                    }
                }
            }
            for (i, c) in trailing.into_iter().enumerate() {
                let position = x - origin + gutter + i;
                if position >= cols as usize {
                    break;
                }
                if graphical {
                    if cells[y].len() <= position {
                        cells[y].resize_with(position + 1, || Cell::text(" "));
                    }
                    if c.bg != self.colors.1 {
                        decorations[y].push(text_presentation::TextSpan::cell(position, 1, &c));
                        cells[y][position] = Cell::text(" ");
                        cells[y][position].fg = self.colors.0.clone();
                        cells[y][position].bg = self.colors.1.clone();
                        continue;
                    }
                }
                cells[y][position] = c;
            }
            if *line == cursor_line && *row_index == cursor_row {
                let column = if wrapping {
                    cursor_col
                } else {
                    cursor_col.saturating_sub(left)
                };
                if wrapping || cursor_col >= left {
                    cursor = Some((
                        y as u16,
                        (column + gutter).min((cols as usize).saturating_sub(1)) as u16,
                    ));
                }
            }
        }
        if graphical {
            cells.truncate(visible.len());
            decorations.truncate(visible.len());
        }
        (
            Screen {
                cells,
                cursor,
                rows,
                cols,
            },
            decorations,
        )
    }
}
