//! Tab placement within the layout limits, and keeping every open document
//! and shell reachable when panes close or a different layout is applied.
use crate::{
    layout::{Axis, View, MAX_DEPTH, MAX_TABS},
    App,
};
use std::collections::BTreeSet;

const TOO_MANY_TABS: &str = "Too many tabs: close some before opening more";

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
            View::Editor(id) => self.forget_view(*id),
            _ => {}
        }
    }

    /// Drop an editor view and what was cached for it.
    pub(crate) fn forget_view(&mut self, view: u64) {
        self.views.remove(&view);
        self.fold_cache.lock().unwrap().remove(&view);
        self.render_cache.remove(&view);
    }

    /// Drop a closed document, its views and everything cached for it.
    /// Every path that closes documents goes through here, so per-document
    /// caches cannot outlive them.
    pub(crate) fn forget_document(&mut self, doc: u64) {
        let views: Vec<u64> = self
            .views
            .iter()
            .filter(|(_, v)| v.document == doc)
            .map(|(id, _)| *id)
            .collect();
        for view in views {
            self.forget_view(view);
        }
        self.documents.remove(&doc);
        self.diff_inspections.remove(&doc);
        self.highlights.remove(&doc);
        self.highlight_pending.remove(&doc);
        self.wrap_cache.lock().unwrap().forget(doc);
        self.overview_cache.remove(&doc);
        self.overview_pending.remove(&doc);
        self.overview_workers.remove(&doc);
        self.formatting.remove(&doc);
    }

    /// Show a view in the focused pane, spilling into other panes when it is
    /// full. The pane that received it becomes focused. Returns false (and
    /// releases the view) when the layout is saturated.
    pub(crate) fn show_view(&mut self, view: View) -> bool {
        match self.place_view(view.clone(), self.focus) {
            Some(pane) => {
                self.focus = pane;
                true
            }
            None => {
                self.discard_view(&view);
                self.status = TOO_MANY_TABS.into();
                false
            }
        }
    }

    /// Show the only view of a document created for it. When the layout has
    /// no room, the document closes again and the "Too many tabs" status
    /// stays.
    /// So does a document beyond the most a recovery checkpoint holds.
    pub(crate) fn show_new_document(&mut self, view: u64) -> bool {
        let doc = self.views[&view].document;
        if self.documents.len() > crate::workspace::MAX_DOCUMENTS {
            self.forget_document(doc);
            self.status = format!(
                "Too many open documents: close some before opening more (at most {})",
                crate::workspace::MAX_DOCUMENTS
            );
            return false;
        }
        if self.add_tab(View::Editor(view)) {
            return true;
        }
        self.forget_document(doc);
        false
    }

    /// Show an unsaved document's view even in a saturated layout, in place
    /// of a tab that loses nothing when it goes: a presentation pane, or a
    /// disposable document (which closes when that was its last view).
    fn place_unsaved(&mut self, view: u64, preferred: u64) -> bool {
        if self.place_view(View::Editor(view), preferred).is_some() {
            return true;
        }
        for pane in self.layout.panes() {
            let tabs = self.layout.tabs(pane).unwrap_or_default();
            let replaceable = tabs.iter().position(|tab| match tab {
                View::Terminal(_) => false,
                View::Editor(id) => self
                    .views
                    .get(id)
                    .is_some_and(|v| self.disposable(v.document)),
                _ => true,
            });
            let Some(index) = replaceable else {
                continue;
            };
            let old = std::mem::replace(
                &mut self.layout.pane_mut(pane).unwrap().0[index],
                View::Editor(view),
            );
            if let View::Editor(id) = old {
                let doc = self.views[&id].document;
                self.forget_view(id);
                let shown = self.layout.views();
                if !self
                    .views
                    .iter()
                    .any(|(other, v)| v.document == doc && shown.contains(&View::Editor(*other)))
                {
                    self.forget_document(doc);
                }
            }
            return true;
        }
        false
    }

    /// Whether closing a document loses nothing: it is saved (or empty and
    /// untitled), and no `--wait` caller is waiting for it to be closed.
    /// Read-only buffers never count as dirty, so a path-less one (standard
    /// input) only survives in its buffer.
    fn disposable(&self, doc: u64) -> bool {
        let Some(d) = self.documents.get(&doc) else {
            return true;
        };
        !d.dirty()
            && (d.path.is_some() || d.is_empty())
            && !self.saves.pending_save.contains(&doc)
            && !self.editor_waits.iter().any(|w| w.waits_for(doc))
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
        let mut unplaced = 0;
        for id in hidden {
            // Making room for unsaved work may have closed a clean document.
            let Some(doc) = self.views.get(&id).map(|v| v.document) else {
                continue;
            };
            if !self.documents.contains_key(&doc) || !visible_docs.insert(doc) {
                self.forget_view(id);
                continue;
            }
            if !self.place_orphan(id, doc, editor_pane) {
                unplaced += 1;
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
            if !self.documents.contains_key(&doc) {
                continue;
            }
            let id = self.id();
            self.views.insert(
                id,
                crate::EditorView {
                    document: doc,
                    ..Default::default()
                },
            );
            if !self.place_orphan(id, doc, editor_pane) {
                unplaced += 1;
            }
        }
        if unplaced > 0 {
            self.status = format!(
                "{TOO_MANY_TABS}: {} not shown",
                crate::counted(unplaced, "document", "documents")
            );
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

    /// Show a document's only view after the layout changed. A document
    /// that cannot be closed without loss always gets a tab; a saved one
    /// that does not fit closes.
    /// Returns false when the document was closed.
    fn place_orphan(&mut self, view: u64, doc: u64, preferred: u64) -> bool {
        let unsaved = !self.disposable(doc);
        if unsaved && self.place_unsaved(view, preferred)
            || !unsaved && self.place_view(View::Editor(view), preferred).is_some()
        {
            return true;
        }
        if unsaved {
            // Only a layout made entirely of terminals and unsaved documents
            // gets here; the document stays open (quitting still asks).
            self.forget_view(view);
            return false;
        }
        self.forget_document(doc);
        false
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
