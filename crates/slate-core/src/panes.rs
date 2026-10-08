//! Tab placement within the layout limits, and keeping every open document
//! and shell reachable when panes close or a different layout is applied.
use crate::{
    layout::{Axis, View, MAX_DEPTH, MAX_TABS},
    App,
};
use std::collections::BTreeSet;

impl App {
    fn has_room(&self, pane: u64) -> bool {
        self.layout
            .tabs(pane)
            .is_some_and(|tabs| tabs.len() < MAX_TABS)
    }

    /// Put a view in `preferred` when it has room, otherwise in another pane
    /// with room, otherwise in a new split. Returns the pane, or `None` when
    /// the layout is saturated and the view could not be shown.
    pub(crate) fn place_view(&mut self, view: View, preferred: u64) -> Option<u64> {
        let mut candidates = vec![preferred];
        candidates.extend(self.layout.panes().into_iter().filter(|p| *p != preferred));
        let pane = match candidates.iter().copied().find(|p| self.has_room(*p)) {
            Some(pane) => pane,
            None => {
                // Split the shallowest pane that may still be split.
                let target = candidates
                    .iter()
                    .copied()
                    .filter_map(|p| self.layout.depth_of(p).map(|d| (d, p)))
                    .filter(|(d, _)| *d < MAX_DEPTH)
                    .min()?
                    .1;
                let (pane, split) = (self.id(), self.id());
                self.layout.split(target, Axis::Vertical, pane, split, view);
                return Some(pane);
            }
        };
        let (tabs, active) = self.layout.pane_mut(pane)?;
        tabs.push(view);
        *active = tabs.len() - 1;
        Some(pane)
    }

    /// Place a view without changing what any pane shows: the pane keeps
    /// its active tab.
    pub(crate) fn place_view_background(&mut self, view: View, preferred: u64) -> Option<u64> {
        let before: Vec<(u64, usize)> = self
            .layout
            .panes()
            .into_iter()
            .filter_map(|p| self.layout.pane_mut(p).map(|(_, active)| (p, *active)))
            .collect();
        let pane = self.place_view(view, preferred)?;
        if let Some((_, active)) = before.iter().find(|(p, _)| *p == pane) {
            if let Some((_, current)) = self.layout.pane_mut(pane) {
                *current = *active;
            }
        }
        Some(pane)
    }

    /// Release a view that could not be placed anywhere.
    fn discard_view(&mut self, view: &View) {
        match view {
            View::Terminal(id) => {
                self.terminals.remove(id);
                self.deferred_terminals.remove(id);
            }
            View::Editor(id) => {
                self.views.remove(id);
            }
            _ => {}
        }
    }

    /// Show a view in the focused pane, spilling into other panes when it is
    /// full. The pane that received it becomes focused.
    pub(crate) fn show_view(&mut self, view: View) {
        match self.place_view(view.clone(), self.focus) {
            Some(pane) => self.focus = pane,
            None => {
                self.discard_view(&view);
                self.status = "Too many tabs: close some before opening more".into();
            }
        }
    }

    /// Whether the focused pane may be split again.
    pub(crate) fn can_split(&self) -> bool {
        self.layout
            .depth_of(self.focus)
            .is_some_and(|d| d < MAX_DEPTH)
    }

    /// Make sure every document and shell still has a tab after the layout
    /// changed shape. Documents keep one view; hidden extra views of a
    /// document that is visible elsewhere are dropped.
    pub(crate) fn adopt_orphans(&mut self) {
        let shown = self.layout.views();
        let shown_editors: BTreeSet<u64> = shown
            .iter()
            .filter_map(|v| match v {
                View::Editor(id) => Some(*id),
                _ => None,
            })
            .collect();
        let shown_terminals: BTreeSet<u64> = shown
            .iter()
            .filter_map(|v| match v {
                View::Terminal(id) => Some(*id),
                _ => None,
            })
            .collect();
        let mut visible_docs: BTreeSet<u64> = shown_editors
            .iter()
            .filter_map(|id| self.views.get(id).map(|v| v.document))
            .collect();
        let hidden: Vec<u64> = self
            .views
            .keys()
            .copied()
            .filter(|id| !shown_editors.contains(id))
            .collect();
        let editor_pane = self.pane_showing(|v| matches!(v, View::Editor(_)));
        for id in hidden {
            let doc = self.views[&id].document;
            if !self.documents.contains_key(&doc) || !visible_docs.insert(doc) {
                self.views.remove(&id);
                continue;
            }
            if self.place_view(View::Editor(id), editor_pane).is_none() {
                self.views.remove(&id);
            }
        }
        // Documents without any view (for example after a pane closed while
        // its views were dropped elsewhere) are reachable again too.
        let orphan_docs: Vec<u64> = self
            .documents
            .keys()
            .copied()
            .filter(|d| !visible_docs.contains(d))
            .collect();
        for doc in orphan_docs {
            let id = self.id();
            self.views.insert(
                id,
                crate::EditorView {
                    document: doc,
                    ..Default::default()
                },
            );
            if self.place_view(View::Editor(id), editor_pane).is_none() {
                self.views.remove(&id);
            }
        }
        let terminal_pane = self.pane_showing(|v| matches!(v, View::Terminal(_)));
        let terminals: Vec<u64> = self
            .terminals
            .keys()
            .chain(self.deferred_terminals.keys())
            .copied()
            .filter(|id| !shown_terminals.contains(id))
            .collect();
        for id in terminals {
            if self.place_view(View::Terminal(id), terminal_pane).is_none() {
                self.discard_view(&View::Terminal(id));
            }
        }
        if !self.layout.panes().contains(&self.focus) {
            self.focus = self.layout.panes()[0];
        }
    }

    /// The pane where new documents open: one already showing an editor.
    pub(crate) fn pane_showing_editor(&self) -> u64 {
        self.pane_showing(|v| matches!(v, View::Editor(_)))
    }

    /// The first pane with a tab of the given kind, else the focused pane.
    fn pane_showing(&self, kind: impl Fn(&View) -> bool) -> u64 {
        let panes = self.layout.panes();
        panes
            .iter()
            .copied()
            .find(|p| self.layout.tabs(*p).is_some_and(|t| t.iter().any(&kind)))
            .unwrap_or(if panes.contains(&self.focus) {
                self.focus
            } else {
                panes[0]
            })
    }
}
