//! Construct a session and resolve startup presentation before recovery.
use crate::*;

impl App {
    pub(crate) fn open_workspace(&mut self, path: PathBuf) -> Result<()> {
        let path = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        }
        .canonicalize()?;
        if !path.is_dir() {
            bail!("Select a workspace folder");
        }
        if !self.terminal_frontend {
            self.frontend_requests
                .push(format!("open-workspace:{}", path.display()));
        } else if path == self.root {
            self.expand_workspace()?;
            self.browse(path)?;
        } else if !self.saves.pending_save.is_empty() {
            bail!("Wait for saves to finish before switching workspaces");
        } else if self.dirty() {
            self.pending_workspace = Some(path.clone());
            self.prompt = Some(search::Prompt {
                kind: "switch-workspace".into(),
                input: path.display().to_string(),
                replacement: String::new(),
                field: 0,
                case_sensitive: false,
                whole_word: false,
            });
        } else {
            self.switch_workspace(&path)?;
        }
        Ok(())
    }
    pub(crate) fn switch_workspace(&mut self, path: &Path) -> Result<()> {
        let launch = cli::Launch {
            directory: Some(path.to_path_buf()),
            recover: true,
            startup: Some(StartupMode::Workspace),
            ..Default::default()
        };
        let mut next = Self::launch_with_events(&launch, None, self.events.clone())?;
        next.terminal_frontend = self.terminal_frontend;
        next.elevation_mode = self.elevation_mode;
        next.inbox = self.inbox.clone();
        next.enable_workspace(true)?;
        self.flush_workspace()?;
        *self = next;
        self.revision += 1;
        self.events.notify();
        Ok(())
    }
    pub fn new(path: &Path) -> Result<Self> {
        Self::new_with_startup(path, None)
    }
    /// Open one file (created on first save if missing) or directory.
    pub fn new_with_startup(path: &Path, mode: Option<StartupMode>) -> Result<Self> {
        let mut launch = cli::Launch {
            recover: true,
            startup: mode,
            ..Default::default()
        };
        if path.is_dir() {
            launch.directory = Some(path.to_path_buf());
        } else {
            launch.files.push(cli::LaunchFile {
                path: path.to_path_buf(),
                ..Default::default()
            });
        }
        Self::launch(&launch, None)
    }
    /// Build the initial session from parsed command-line arguments.
    /// `stdin` holds the bytes read for a `-` argument.
    pub fn launch(launch: &cli::Launch, stdin: Option<Vec<u8>>) -> Result<Self> {
        Self::launch_with_events(launch, stdin, events::Events::default())
    }
    pub(crate) fn launch_with_events(
        launch: &cli::Launch,
        stdin: Option<Vec<u8>>,
        events: events::Events,
    ) -> Result<Self> {
        let directory = launch
            .directory
            .as_ref()
            .map(|d| {
                d.canonicalize()
                    .with_context(|| format!("Cannot open {}", d.display()))
            })
            .transpose()?;
        let mut opened: Vec<(Document, Option<usize>, Option<usize>)> = vec![];
        if let (Some(spec), Some(bytes)) = (&launch.stdin, stdin) {
            let mut document = Document::from_bytes(&bytes, "standard input")?;
            document.label = None;
            opened.push((document, spec.line, spec.column));
        }
        // Recovery checkpoints hold a bounded number of documents; opening
        // more would leave the whole session without crash recovery.
        let room = workspace::MAX_DOCUMENTS.saturating_sub(opened.len());
        let skipped = launch.files.len().saturating_sub(room);
        for file in launch.files.iter().take(room) {
            let document = if file.path.exists() {
                Document::open(&file.path)
                    .with_context(|| format!("Cannot open {}", file.path.display()))?
            } else {
                Document::new_file(&file.path)?
            };
            opened.push((document, file.line, file.column));
        }
        let file_mode = directory.is_none() && (!launch.files.is_empty() || launch.stdin.is_some());
        let root = match &directory {
            Some(directory) => directory.clone(),
            None => opened
                .iter()
                .find_map(|(d, _, _)| d.path.as_ref()?.parent().map(Path::to_path_buf))
                .filter(|p| p.is_dir())
                .map_or_else(std::env::current_dir, Ok)?,
        };
        let recovery_key = match (&directory, opened.first()) {
            (None, Some((d, _, _))) => d.path.clone().unwrap_or_else(|| root.clone()),
            _ => root.clone(),
        };
        let mut notices = vec![];
        let mut documents = BTreeMap::new();
        let mut views = BTreeMap::new();
        let mut positions = vec![];
        let mut ids = 20;
        if opened.is_empty() {
            opened.push((Document::scratch(), None, None));
        }
        let mut tabs = vec![];
        for (index, (mut document, line, column)) in opened.into_iter().enumerate() {
            let (doc, view) = if index == 0 {
                (10, 11)
            } else {
                ids += 2;
                (ids - 1, ids)
            };
            if launch.read_only {
                document.read_only = true;
            }
            if let Some(notice) = document.notice.take() {
                notices.push(notice);
            }
            documents.insert(doc, document);
            views.insert(
                view,
                EditorView {
                    document: doc,
                    ..Default::default()
                },
            );
            tabs.push(View::Editor(view));
            positions.push((view, line, column));
        }
        let mut layout = Node::default_layout(11, 12);
        // Beyond one pane's tab limit, further files open in editor panes
        // split beside it.
        let mut chunks = tabs.chunks(crate::layout::MAX_TABS);
        if let Some((pane_tabs, _)) = layout.pane_mut(2) {
            *pane_tabs = chunks.next().unwrap_or_default().to_vec();
        }
        let mut last = 2;
        for chunk in chunks {
            ids += 2;
            let (pane, split) = (ids - 1, ids);
            layout.split(last, Axis::Vertical, pane, split, chunk[0].clone());
            if let Some((pane_tabs, _)) = layout.pane_mut(pane) {
                *pane_tabs = chunk.to_vec();
            }
            last = pane;
        }
        if skipped > 0 {
            notices.push(format!(
                "Opened {} of {} files: a session holds at most {} documents",
                launch.files.len() - skipped,
                launch.files.len(),
                workspace::MAX_DOCUMENTS
            ));
        }
        let mut app = Self {
            browser: root.clone(),
            pending_workspace: None,
            root: root.clone(),
            documents,
            views,
            terminals: BTreeMap::new(),
            deferred_terminals: BTreeMap::from([(12, root.clone())]),
            editor_only: false,
            layout,
            focus: 2,
            status: String::new(),
            quit: false,
            clipboard: String::new(),
            files: vec![],
            expanded_folders: Default::default(),
            pending_file: None,
            pending_reveal: None,
            browse_requests: 0,
            browse_replies: 0,
            pending_path_dialog: None,
            git: git::Panel::default(),
            selected: 0,
            services: Services::new(events.clone()),
            events,
            revision: 1,
            files_revision: 1,
            render_cache: BTreeMap::new(),
            shared_files: Default::default(),
            shared_git: Default::default(),
            shared_history: Default::default(),
            screen_builds: 0,
            typing: false,
            ids,
            presets: BTreeMap::new(),
            layouts_unreadable: None,
            preferences: Preferences::default(),
            preference_layers: Default::default(),
            prompt: None,
            search: Search::default(),
            highlights: BTreeMap::new(),
            highlight_pending: BTreeMap::new(),
            pending_open: BTreeMap::new(),
            saves: Default::default(),
            recovery: workspace::RecoveryState::new(recovery_key),
            pending_close: None,
            selection_foreground: "#ffffff".into(),
            colors: (
                "#d8dee9".into(),
                "#20242c".into(),
                "#425b78".into(),
                "#88c0d0".into(),
            ),
            system_colors: None,
            terminal_colors: theme::Palette::dark(true),
            files_colors: theme::Palette::dark(false),
            git_colors: theme::Palette::dark(false),
            file_mode,
            elevation_mode: ElevationMode::None,
            last_cut: None,
            misspellings: vec![],
            suspend_requested: false,
            terminal_frontend: true,
            document_search_state: Default::default(),
            disk_check_at: Instant::now(),
            disk_check_pending: false,
            disk_conflicts: Default::default(),
            recent: desktop::load_recent(),
            recent_projects: crate::desktop::load_projects(),
            diff_inspections: Default::default(),
            history: Default::default(),
            diagnostics: Default::default(),
            file_locks: Default::default(),
            launch_options: launch.options.clone(),
            ignore_rc: launch.ignore_rc,
            overview_cache: BTreeMap::new(),
            overview_pending: Default::default(),
            overview_workers: Default::default(),
            overview_revision: 0,
            frontend_requests: vec![],
            inbox: None,
            editor_waits: Vec::new(),
            pending_positions: BTreeMap::new(),
            back: Vec::new(),
            forward: Vec::new(),
            picker: None,
            index: Default::default(),
            index_at: None,
            index_pending: false,
            search_cancel: Default::default(),
            last_search: None,
            project_search_policy: Default::default(),
            last_search_files: Vec::new(),
            replace_backups: Vec::new(),
            replace_undoing: false,
            lsp: Default::default(),
            formatting: Default::default(),
            tasks: BTreeMap::new(),
            debug: Default::default(),
            tools: Vec::new(),
            trusted: trust::is_trusted(&root),
            pending_trust: None,
            trust_cache: Default::default(),
            trust_generation: 0,
            session_trust: Vec::new(),
            fold_cache: Default::default(),
            wrap_cache: Default::default(),
            fuzzy_worker: None,
            search_worker: None,
            outline_worker: None,
            outline_ticket: Default::default(),
        };
        app.read_layouts();
        app.load_preferences();
        if let Some(layout) = app.preference_layers.layout.clone() {
            app.restore_layout(layout)?;
        }
        app.reload_tools();
        app.editor_only =
            launch
                .startup
                .unwrap_or(if let Some(editor_only) = app.preference_layers.startup {
                    if editor_only {
                        StartupMode::EditorOnly
                    } else {
                        StartupMode::Workspace
                    }
                } else if file_mode {
                    app.preferences.file_startup
                } else {
                    app.preferences.directory_startup
                })
                == StartupMode::EditorOnly;
        for (view, line, column) in positions {
            if line.is_some() || column.is_some() {
                let doc = app.views[&view].document;
                let cursor = app.documents[&doc].at_line_col(
                    line.unwrap_or(1).saturating_sub(1),
                    column.unwrap_or(1).saturating_sub(1),
                    app.preferences.indent_width,
                );
                app.views.get_mut(&view).unwrap().cursor = cursor;
            }
        }
        if !app.editor_only {
            app.ensure_terminals()?;
        }
        if !notices.is_empty() {
            app.status = notices.join(" · ");
        }
        if launch.directory.is_some() {
            app.remember_project();
        }
        app.update_file_locks();
        app.refresh();
        Ok(app)
    }
}
