use crate::{layout::View, terminal::TerminalSession, App};
use anyhow::Result;

impl App {
    // Keep the complete split tree and its view identities when hiding tools.
    // This also keeps terminal processes alive during an in-session collapse.
    pub(super) fn collapse_workspace(&mut self) {
        self.editor_only = true;
        self.select_editor_pane();
        self.status = "Editor only · expand the workspace to show tools".into();
    }

    pub(super) fn expand_workspace(&mut self) -> Result<()> {
        self.ensure_terminals()?;
        self.editor_only = false;
        self.status = "Workspace expanded".into();
        Ok(())
    }

    pub(super) fn ensure_terminals(&mut self) -> Result<()> {
        // Spawn lazily: an editor-only invocation must not start hidden shells,
        // including shells remembered by a recovered workspace.
        let pending = self.deferred_terminals.clone();
        for (id, cwd) in pending {
            let session = self.spawn_shell(&cwd)?;
            self.terminals.insert(id, session);
            self.deferred_terminals.remove(&id);
        }
        Ok(())
    }

    /// Start a shell in `cwd` (or the workspace root when it is gone) with
    /// the configured scrollback, wired to wake the event loop.
    pub(super) fn spawn_shell(&self, cwd: &std::path::Path) -> Result<TerminalSession> {
        let cwd = if cwd.is_dir() { cwd } else { &self.root };
        let mut session =
            TerminalSession::spawn_with(cwd, None, 24, 80, self.preferences.terminal_scrollback)?;
        session.set_events(self.events.clone());
        Ok(session)
    }

    /// Close the tabs of shells that exited, and pick up text that programs
    /// copied with OSC 52.
    pub(super) fn reap_terminals(&mut self) {
        let mut exited = Vec::new();
        for (id, terminal) in &mut self.terminals {
            if let Some(text) = terminal.take_clipboard() {
                self.clipboard = text;
                self.revision += 1;
            }
            if let Some(code) = terminal.exit_code() {
                exited.push((*id, code));
            }
        }
        for (id, code) in exited {
            self.terminals.remove(&id);
            self.remove_terminal_views(id);
            self.workspace_dirty = true;
            self.revision += 1;
            self.status = if code == 0 {
                format!("Terminal {id} exited")
            } else {
                format!("Terminal {id} exited with code {code}")
            };
        }
    }

    pub(super) fn select_editor_pane(&mut self) {
        if self.editor_only {
            // Recovery may have remembered focus on a tool or a tool tab.
            if !matches!(self.layout.view(self.focus), Some(View::Editor(_))) {
                for pane in self.layout.panes() {
                    let (tabs, active) = self.layout.pane_mut(pane).unwrap();
                    if let Some(index) = tabs.iter().position(|v| matches!(v, View::Editor(_))) {
                        *active = index;
                        self.focus = pane;
                        return;
                    }
                }
                self.editor_target();
            }
        }
    }
}
