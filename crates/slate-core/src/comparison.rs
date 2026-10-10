//! Diff inspection, guarded hunk staging, and undoable conflict choices.
use crate::{
    document::Document,
    git::{Context, GitJob},
    layout::{Axis, View},
    services::Job,
    App, Command, EditorView,
};
use anyhow::{bail, Context as _, Result};
use std::{ffi::OsString, time::Duration};
#[derive(Clone)]
pub struct Preview {
    pub context: Context,
    pub path: OsString,
    pub display: String,
    pub staged: bool,
    pub untracked: bool,
    pub patch: String,
    pub side_by_side: bool,
}
#[derive(Clone)]
pub(crate) struct Inspection {
    preview: std::sync::Arc<Preview>,
    rows: Vec<(usize, usize)>,
}
fn hunks(patch: &str) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, a)| (*a, starts.get(i + 1).copied().unwrap_or(patch.len())))
        .collect()
}
pub(crate) fn apply_hunk(preview: &Preview, index: usize) -> Result<String, String> {
    let current = crate::git::load_patch(
        &preview.context,
        &preview.path,
        preview.staged,
        preview.untracked,
    )?;
    if current != preview.patch {
        return Err("Diff changed; reopen it before staging a hunk".into());
    }
    if preview.untracked
        || preview.patch.contains("[Diff preview truncated")
        || preview.patch.lines().any(|l| {
            l.starts_with("new file mode ")
                || l.starts_with("deleted file mode ")
                || l.starts_with("rename ")
                || l.starts_with("Binary files ")
        })
    {
        return Err("This file requires whole-file staging".into());
    }
    let ranges = hunks(&preview.patch);
    let (a, b) = ranges.get(index).copied().ok_or("No hunk at the cursor")?;
    let first = ranges.first().ok_or("No text hunks")?.0;
    let patch = format!("{}{}", &preview.patch[..first], &preview.patch[a..b]);
    for check in [true, false] {
        let mut command = crate::git::command(&preview.context.root);
        command.args(["apply", "--cached", "--whitespace=nowarn"]);
        if preview.staged {
            command.arg("--reverse");
        }
        if check {
            command.arg("--check");
        }
        let output = crate::process::run(
            &mut command,
            Some(patch.as_bytes().to_vec()),
            Duration::from_secs(30),
            "git apply hunk",
        )?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().into());
        }
    }
    Ok(if preview.staged {
        "Unstaged hunk"
    } else {
        "Staged hunk"
    }
    .into())
}
impl App {
    pub(crate) fn request_comparison(&mut self) -> Result<()> {
        let path = self.selected_path().context("Select a Git file first")?;
        let staged = self
            .git
            .entries
            .get(self.git.selected)
            .is_some_and(|e| e.staged);
        let retry = Command::Action {
            name: "diff-side-by-side".into(),
            argument: String::new(),
        };
        let Some(context) = self.git_ready(retry) else {
            return Ok(());
        };
        let untracked = self
            .git
            .entries
            .iter()
            .any(|e| e.path == path && e.untracked);
        self.services.tx.send(Job::Git(GitJob::Comparison {
            context,
            path: crate::git::raw_path(&self.git.entries, &path),
            display: path,
            staged,
            untracked,
        }))?;
        self.git.jobs += 1;
        self.status = "Loading comparison…".into();
        Ok(())
    }
    pub(crate) fn show_comparison(&mut self, preview: Preview) {
        self.git.jobs = self.git.jobs.saturating_sub(1);
        if self.quit {
            return;
        }
        let preview = std::sync::Arc::new(preview);
        let _ = self.editor_target();
        if preview.side_by_side {
            let mut left = String::new();
            let mut right = String::new();
            let mut rows = Vec::new();
            let mut hunk = 0;
            let mut row = 0;
            for line in preview.patch.lines() {
                if line.starts_with("@@ ") {
                    if !rows.is_empty() {
                        hunk += 1;
                    }
                    rows.push((row, hunk));
                    left.push_str(line);
                    right.push_str(line);
                } else if line.starts_with("--- ")
                    || line.starts_with("+++ ")
                    || line.starts_with("diff ")
                    || line.starts_with("index ")
                {
                    continue;
                } else if let Some(content) = line.strip_prefix('-') {
                    left.push('-');
                    left.push_str(content);
                } else if let Some(content) = line.strip_prefix('+') {
                    right.push('+');
                    right.push_str(content);
                } else {
                    left.push_str(line);
                    right.push_str(line);
                }
                left.push('\n');
                right.push('\n');
                row += 1;
            }
            let title = format!(
                "{} · {}",
                if preview.staged {
                    "HEAD → index"
                } else {
                    "index → working copy"
                },
                preview.display
            );
            let l = self.inspect_diff(
                &format!("Before · {title}"),
                left,
                preview.clone(),
                rows.clone(),
            );
            if !self.show_new_document(l) {
                return;
            }
            let r = self.inspect_diff(&format!("After · {title}"), right, preview, rows);
            let pane = self.id();
            let split = self.id();
            if self.can_split() {
                self.layout
                    .split(self.focus, Axis::Horizontal, pane, split, View::Editor(r));
                self.focus = pane;
            } else if !self.show_new_document(r) {
                return;
            }
        } else {
            let rows = preview
                .patch
                .lines()
                .enumerate()
                .filter(|(_, l)| l.starts_with("@@ "))
                .enumerate()
                .map(|(h, (r, _))| (r, h))
                .collect();
            let title = format!(
                "{} · {}",
                if preview.staged {
                    "Staged diff"
                } else {
                    "Working diff"
                },
                preview.display
            );
            let view = self.inspect_diff(&title, preview.patch.clone(), preview, rows);
            if !self.show_new_document(view) {
                return;
            }
        }
        self.status = "Diff · read-only · stage-hunk / unstage-hunk at the cursor".into();
    }
    fn inspect_diff(
        &mut self,
        title: &str,
        text: String,
        preview: std::sync::Arc<Preview>,
        rows: Vec<(usize, usize)>,
    ) -> u64 {
        let doc = self.id();
        self.documents
            .insert(doc, Document::inspection(title.into(), text));
        self.diff_inspections
            .insert(doc, Inspection { preview, rows });
        let view = self.id();
        self.views.insert(
            view,
            EditorView {
                document: doc,
                ..Default::default()
            },
        );
        view
    }
    pub(crate) fn can_stage_diff_hunk(&self, reverse: bool) -> bool {
        self.active_editor()
            .and_then(|id| self.views.get(&id))
            .and_then(|v| self.diff_inspections.get(&v.document))
            .is_some_and(|inspection| {
                inspection.preview.staged == reverse
                    && !inspection.preview.untracked
                    && !inspection.rows.is_empty()
            })
    }
    pub(crate) fn stage_diff_hunk(&mut self, reverse: bool) -> Result<()> {
        let view = self.active_editor().context("Focus a diff first")?;
        let v = &self.views[&view];
        let inspection = self
            .diff_inspections
            .get(&v.document)
            .context("Open a Git diff first")?
            .clone();
        anyhow::ensure!(
            inspection.preview.staged == reverse,
            "Use stage-hunk for working diffs and unstage-hunk for staged diffs"
        );
        anyhow::ensure!(
            self.trusted_path(&inspection.preview.context.root),
            "Trust the repository first"
        );
        let line = self.documents[&v.document].line_of(v.cursor);
        let index = inspection
            .rows
            .iter()
            .rev()
            .find(|(row, _)| *row <= line)
            .context("Place the cursor in a hunk")?
            .1;
        self.services
            .tx
            .send(Job::Git(GitJob::Hunk(inspection.preview, index)))?;
        self.git.jobs += 1;
        self.status = "Applying hunk…".into();
        Ok(())
    }
    pub(crate) fn resolve_conflict(&mut self, choice: &str) -> Result<()> {
        let view = self
            .active_editor()
            .context("Focus a conflicted document first")?;
        let v = &self.views[&view];
        let d = &self.documents[&v.document];
        anyhow::ensure!(
            d.rope().len_bytes() <= 16 * 1024 * 1024,
            "Conflict resolution is limited to 16 MiB documents; resolve a larger file manually"
        );
        let mut start = None;
        let mut divider = None;
        let mut base = None;
        let mut finish = None;
        for line in 0..d.line_count() {
            // Inspect only the marker prefix; ordinary lines may be very long.
            let text: String = d.rope().line(line).chars().take(64).collect();
            if text.starts_with("<<<<<<< ") {
                if start.is_some() {
                    bail!("Nested conflict markers; resolve manually");
                }
                start = Some(line);
                divider = None;
                base = None;
            } else if text.starts_with("||||||| ") && start.is_some() {
                anyhow::ensure!(
                    base.is_none() && divider.is_none(),
                    "Malformed conflict base marker; resolve manually"
                );
                base = Some(line);
            } else if text.trim_end_matches('\n') == "=======" && start.is_some() {
                anyhow::ensure!(
                    divider.is_none(),
                    "Repeated conflict divider; resolve manually"
                );
                divider = Some(line);
            } else if text.starts_with(">>>>>>> ") && start.is_some() {
                let a = start.unwrap();
                if d.line_of(v.cursor) >= a && d.line_of(v.cursor) <= line {
                    finish = Some((a, divider.context("Missing conflict divider")?, line, base));
                    break;
                }
                start = None;
            }
        }
        let (a, b, c, base) = finish.context("Place the cursor inside a complete conflict")?;
        let ours = d
            .slice(d.line_offset(a + 1), d.line_offset(base.unwrap_or(b)))
            .into_owned();
        let theirs = d.slice(d.line_offset(b + 1), d.line_offset(c)).into_owned();
        let text = match choice {
            "ours" => ours,
            "theirs" => theirs,
            "both" => format!("{ours}{theirs}"),
            _ => bail!("Unknown conflict choice"),
        };
        let end = if c + 1 < d.line_count() {
            d.line_offset(c + 1)
        } else {
            d.len()
        };
        self.replace_in_document(v.document, &[(d.line_offset(a), end, text)])?;
        self.status = "Conflict choice applied · undo available · save and stage when ready".into();
        Ok(())
    }
}
