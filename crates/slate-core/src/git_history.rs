//! Commit history for the Git pane: `git log` parsing and the commit graph's
//! lane layout. Layout is computed once per history read, so frontends only
//! draw the segments each row carries.
use serde::Serialize;

/// Commits read per page, and the most the pane will hold.
pub const PAGE: usize = 200;
pub const MAX: usize = 5000;
/// Lane colours, indexed by a row's colour numbers. The GUI painter keeps
/// the same list (`GraphLanes` in bridge.cpp).
pub const PALETTE: [&str; 8] = [
    "#4c8bf5", "#e5a03b", "#3fb27f", "#d65db1", "#ef6b5b", "#20b2c4", "#9b7be0", "#a3a33a",
];

/// A branch, remote branch or tag pointing at a commit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ref {
    pub name: String,
    /// `head` (the checked-out branch, or a detached HEAD), `branch`,
    /// `remote` or `tag`.
    pub kind: &'static str,
}

/// A line within one row of the graph, from lane `x1` to lane `x2`. Heights
/// are 0 (the row's top edge), 1 (its middle, where the commit's node is)
/// and 2 (its bottom edge). The last value is the palette colour.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Segment(pub u16, pub u8, pub u16, pub u8, pub u8);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HistoryRow {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, in Unix seconds.
    pub time: i64,
    pub refs: Vec<Ref>,
    pub subject: String,
    /// The commit's lane and colour, and how many lanes the row spans.
    pub lane: u16,
    pub color: u8,
    pub lanes: u16,
    pub segments: Vec<Segment>,
    /// The checked-out commit.
    pub head: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub refs: Vec<Ref>,
    pub subject: String,
}

/// The `git log` format [`parse_log`] reads: fields separated by US and
/// commits terminated by RS, which commit text does not use.
pub(crate) const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1e";

pub(crate) fn parse_log(output: &[u8]) -> Vec<Commit> {
    String::from_utf8_lossy(output)
        .split('\x1e')
        .filter_map(|record| {
            let fields: Vec<&str> = record
                .trim_start_matches(['\n', '\r'])
                .split('\x1f')
                .collect();
            let [hash, parents, author, email, time, refs, subject] = fields[..] else {
                return None;
            };
            if hash.len() < 4 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            Some(Commit {
                hash: hash.into(),
                parents: parents.split_whitespace().map(str::to_string).collect(),
                author: author.into(),
                email: email.into(),
                time: time.trim().parse().unwrap_or(0),
                refs: parse_refs(refs),
                subject: subject.into(),
            })
        })
        .collect()
}

/// Decorations as `git log --decorate=full` prints them (`%D`).
pub(crate) fn parse_refs(decorations: &str) -> Vec<Ref> {
    let mut refs = Vec::new();
    for item in decorations
        .split(", ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(branch) = item.strip_prefix("HEAD -> ") {
            let name = branch.strip_prefix("refs/heads/").unwrap_or(branch);
            refs.push(Ref {
                name: name.into(),
                kind: "head",
            });
        } else if item == "HEAD" {
            refs.push(Ref {
                name: "HEAD".into(),
                kind: "head",
            });
        } else if let Some(tag) = item.strip_prefix("tag: ") {
            refs.push(Ref {
                name: tag.strip_prefix("refs/tags/").unwrap_or(tag).into(),
                kind: "tag",
            });
        } else if let Some(branch) = item.strip_prefix("refs/heads/") {
            refs.push(Ref {
                name: branch.into(),
                kind: "branch",
            });
        } else if let Some(remote) = item.strip_prefix("refs/remotes/") {
            // A remote's default-branch pointer repeats one of its branches.
            if !remote.ends_with("/HEAD") {
                refs.push(Ref {
                    name: remote.into(),
                    kind: "remote",
                });
            }
        }
    }
    refs
}

/// Assign lanes to commits in topological order. A lane holds the commit it
/// leads to; lanes keep their position while unrelated commits pass, and a
/// freed lane is reused by the next new line.
pub(crate) fn layout(commits: Vec<Commit>, head: &str) -> Vec<HistoryRow> {
    let mut lanes: Vec<Option<(String, u8)>> = Vec::new();
    let mut next_color = 0u8;
    let mut new_color = || {
        let color = next_color;
        next_color = (next_color + 1) % PALETTE.len() as u8;
        color
    };
    let free_slot =
        |lanes: &mut Vec<Option<(String, u8)>>| match lanes.iter().position(Option::is_none) {
            Some(index) => index,
            None => {
                lanes.push(None);
                lanes.len() - 1
            }
        };
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let before = lanes.clone();
        let expected = |lanes: &[Option<(String, u8)>], hash: &str| {
            lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|(h, _)| h == hash))
        };
        let (lane, color) = match expected(&before, &commit.hash) {
            Some(index) => (index, before[index].as_ref().unwrap().1),
            None => (free_slot(&mut lanes), new_color()),
        };
        let mut segments = Vec::new();
        for (index, slot) in before.iter().enumerate() {
            if let Some((hash, slot_color)) = slot {
                segments.push(if *hash == commit.hash {
                    Segment(index as u16, 0, lane as u16, 1, *slot_color)
                } else {
                    Segment(index as u16, 0, index as u16, 2, *slot_color)
                });
            }
        }
        // Every line that led here ends at this commit.
        for slot in lanes.iter_mut() {
            if slot.as_ref().is_some_and(|(h, _)| *h == commit.hash) {
                *slot = None;
            }
        }
        for (index, parent) in commit.parents.iter().enumerate() {
            if let Some(target) = expected(&lanes, parent) {
                // The parent is already on its way down another lane.
                let target_color = lanes[target].as_ref().unwrap().1;
                segments.push(Segment(lane as u16, 1, target as u16, 2, target_color));
            } else if index == 0 && lanes[lane].is_none() {
                lanes[lane] = Some((parent.clone(), color));
                segments.push(Segment(lane as u16, 1, lane as u16, 2, color));
            } else {
                let target = free_slot(&mut lanes);
                let branch_color = new_color();
                lanes[target] = Some((parent.clone(), branch_color));
                segments.push(Segment(lane as u16, 1, target as u16, 2, branch_color));
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let width = before.len().max(lanes.len()).max(lane + 1);
        rows.push(HistoryRow {
            short: commit.hash.chars().take(7).collect(),
            head: !head.is_empty() && commit.hash == head,
            hash: commit.hash,
            parents: commit.parents,
            author: commit.author,
            email: commit.email,
            time: commit.time,
            refs: commit.refs,
            subject: commit.subject,
            lane: lane as u16,
            color,
            lanes: width as u16,
            segments,
        });
    }
    rows
}

/// A row's graph as text: two cells per lane (the lane, then the gap to the
/// next one), each with its palette colour. At most `max_lanes` lanes are
/// drawn; wider rows end with `…`.
pub fn glyphs(row: &HistoryRow, max_lanes: usize) -> Vec<(char, Option<u8>)> {
    let lanes = (row.lanes as usize).max(1);
    let shown = lanes.min(max_lanes.max(1));
    // Per lane: up, down, left, right connections and a colour.
    let mut cells = vec![(false, false, false, false, None::<u8>); lanes];
    let mut gaps = vec![None::<u8>; lanes];
    let node = row.lane as usize;
    for &Segment(x1, y1, x2, y2, color) in &row.segments {
        let (x1, x2) = (x1 as usize, x2 as usize);
        if x1 >= lanes || x2 >= lanes {
            continue;
        }
        if x1 == x2 {
            if x1 != node || (y1, y2) == (0, 2) {
                let cell = &mut cells[x1];
                cell.0 |= y1 == 0;
                cell.1 |= y2 == 2;
                cell.4.get_or_insert(color);
            }
            continue;
        }
        // A diagonal joins the node's lane to another lane. The far end turns
        // vertically: up for a line coming in, down for a line going out.
        let (far, incoming) = if y1 == 0 { (x1, true) } else { (x2, false) };
        let cell = &mut cells[far];
        if incoming {
            cell.0 = true;
        } else {
            cell.1 = true;
        }
        if far > node {
            cell.2 = true;
        } else {
            cell.3 = true;
        }
        cell.4.get_or_insert(color);
        let (low, high) = (far.min(node), far.max(node));
        for cell in &mut cells[low + 1..high] {
            cell.2 = true;
            cell.3 = true;
            cell.4.get_or_insert(color);
        }
        for gap in &mut gaps[low..high] {
            gap.get_or_insert(color);
        }
    }
    let mut out = Vec::with_capacity(shown * 2);
    for lane in 0..shown {
        let (up, down, left, right, color) = cells[lane];
        let glyph = if lane == node {
            '●'
        } else {
            match (up, down, left, right) {
                (true, true, false, false) => '│',
                (false, false, true, true) => '─',
                (true, false, true, false) => '╯',
                (true, false, false, true) => '╰',
                (false, true, true, false) => '╮',
                (false, true, false, true) => '╭',
                (true, true, true, false) => '┤',
                (true, true, false, true) => '├',
                (true, false, true, true) => '┴',
                (false, true, true, true) => '┬',
                (true, true, true, true) => '┼',
                (true, false, false, false) | (false, true, false, false) => '│',
                _ => ' ',
            }
        };
        out.push((glyph, if lane == node { Some(row.color) } else { color }));
        if lane + 1 < shown {
            out.push(match gaps[lane] {
                Some(color) => ('─', Some(color)),
                None => (' ', None),
            });
        }
    }
    if lanes > shown {
        out.push(('…', None));
    }
    out
}

use crate::{git::GitJob, services::Job, App, Command};
use anyhow::{bail, Result};

/// Commit documents are labelled `<short hash> · commit`; one is reused.
const COMMIT_LABEL: &str = " · commit";

impl App {
    /// The commit selected in the Git pane, whose list continues past its
    /// changes into the graph.
    pub(crate) fn selected_commit(&self) -> Option<&HistoryRow> {
        self.git
            .selected
            .checked_sub(self.git.entries.len())
            .and_then(|index| self.git.history.get(index))
    }
    /// Rows the Git pane lists: its changes, then its commits.
    pub(crate) fn git_rows(&self) -> usize {
        self.git.entries.len() + self.git.history.len()
    }
    /// More commits exist beyond those read.
    pub fn history_has_more(&self) -> bool {
        self.git.history.len() >= self.git.history_limit && self.git.history_limit < MAX
    }
    /// Forget the branch, remote and graph of a repository Git may not read.
    pub(crate) fn clear_repository_details(&mut self) {
        let git = &mut self.git;
        git.head.clear();
        git.upstream.clear();
        git.upstream_head.clear();
        git.ahead = 0;
        git.behind = 0;
        git.branches.clear();
        git.history_key = Default::default();
        git.history_stale = false;
        if !git.history.is_empty() || !git.history_error.is_empty() {
            git.history.clear();
            git.history_error.clear();
            git.history_revision += 1;
        }
    }
    /// Read the graph again. One read runs at a time; requests made meanwhile
    /// become a single follow-up read.
    pub(crate) fn refresh_history(&mut self) {
        let Some((context, true)) = self.git_context() else {
            return;
        };
        if self.git.history_running {
            self.git.history_again = true;
            return;
        }
        let job = Job::Git(GitJob::History {
            context,
            limit: self.git.history_limit,
            head: self.git.head.clone(),
        });
        if self.services.tx.send(job).is_ok() {
            self.git.history_running = true;
            self.git.history_stale = false;
            self.git.history_key = (self.git.head.clone(), self.git.upstream_head.clone());
        }
    }
    pub(crate) fn history_done(&mut self, result: Result<Vec<HistoryRow>, String>) {
        self.git.history_running = false;
        match result {
            Ok(rows) => {
                let commit = self.selected_commit().map(|c| c.hash.clone());
                if rows != self.git.history || !self.git.history_error.is_empty() {
                    self.git.history = rows;
                    self.git.history_error.clear();
                    self.git.history_revision += 1;
                }
                if let Some(hash) = commit {
                    let index = self.git.history.iter().position(|c| c.hash == hash);
                    self.git.selected = self.git.entries.len() + index.unwrap_or(0);
                }
                self.git.selected = self.git.selected.min(self.git_rows().saturating_sub(1));
            }
            Err(error) => {
                if self.git.history_error != error {
                    self.status = format!("Git history: {error}");
                    self.git.history_error = error;
                    self.git.history_revision += 1;
                }
            }
        }
        if std::mem::take(&mut self.git.history_again) {
            self.refresh_history();
        }
    }
    pub(crate) fn load_more_history(&mut self) -> Result<()> {
        if !self.history_has_more() {
            bail!("All commits are shown");
        }
        self.git.history_limit = (self.git.history_limit + PAGE).min(MAX);
        self.git.history_stale = true;
        self.refresh_history();
        Ok(())
    }
    /// Open a commit (the argument, else the selected one) read-only.
    pub(crate) fn show_commit(&mut self, argument: &str) -> Result<()> {
        let hash = match argument.trim() {
            "" => self
                .selected_commit()
                .map(|c| c.hash.clone())
                .unwrap_or_default(),
            hash => hash.to_string(),
        };
        if !crate::git::valid_hash(&hash) {
            bail!("Select a commit first");
        }
        let retry = Command::Action {
            name: "git-show".into(),
            argument: hash.clone(),
        };
        let Some(context) = self.git_ready(retry) else {
            return Ok(());
        };
        self.services
            .tx
            .send(Job::Git(GitJob::Show { context, hash }))?;
        self.status = "Loading commit…".into();
        Ok(())
    }
    pub(crate) fn commit_shown(&mut self, hash: String, result: Result<String, String>) {
        let text = match result {
            Ok(text) => text,
            Err(error) => {
                self.status = format!("Git: {error}");
                return;
            }
        };
        let label = format!("{}{COMMIT_LABEL}", hash.chars().take(7).collect::<String>());
        let existing = self
            .documents
            .iter()
            .find(|(_, d)| {
                d.read_only
                    && d.label
                        .as_deref()
                        .is_some_and(|l| l.ends_with(COMMIT_LABEL))
            })
            .map(|(id, _)| *id);
        let doc = match existing {
            Some(doc) => {
                let d = self.documents.get_mut(&doc).unwrap();
                let len = d.len();
                d.replace_inspection(0, len, &text);
                d.label = Some(label);
                d.generation += 1;
                // A different commit starts at its top.
                for view in self.views.values_mut().filter(|v| v.document == doc) {
                    view.cursor = 0;
                    view.anchor = None;
                    view.top = 0;
                    view.top_row = 0;
                    view.left = 0;
                    view.extra.clear();
                    view.folds.clear();
                }
                doc
            }
            None => {
                let doc = self.id();
                self.documents
                    .insert(doc, crate::document::Document::inspection(label, text));
                doc
            }
        };
        self.reveal_document(doc);
        self.status = "Commit · read-only".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: "A".into(),
            email: "a@example.invalid".into(),
            time: 0,
            refs: vec![],
            subject: hash.into(),
        }
    }
    fn text(row: &HistoryRow) -> String {
        glyphs(row, 8)
            .into_iter()
            .map(|(c, _)| c)
            .collect::<String>()
            .trim_end()
            .into()
    }

    #[test]
    fn log_records_and_decorations_parse() {
        let output = b"aaaa1111\x1fbbbb2222 cccc3333\x1fAda\x1fada@example.invalid\x1f1700000000\x1fHEAD -> refs/heads/main, refs/remotes/origin/main, refs/remotes/origin/HEAD, tag: refs/tags/v1\x1fMerge\x1e\nbbbb2222\x1f\x1fBo\x1fbo@example.invalid\x1f1600000000\x1f\x1fFirst \xe2\x80\x94 commit\x1e\n";
        let commits = parse_log(output);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, ["bbbb2222", "cccc3333"]);
        assert_eq!(commits[0].time, 1_700_000_000);
        assert_eq!(
            commits[0].refs,
            [
                Ref {
                    name: "main".into(),
                    kind: "head"
                },
                Ref {
                    name: "origin/main".into(),
                    kind: "remote"
                },
                Ref {
                    name: "v1".into(),
                    kind: "tag"
                },
            ]
        );
        assert!(commits[1].parents.is_empty());
        assert_eq!(commits[1].subject, "First — commit");
        assert_eq!(
            parse_refs("HEAD, refs/heads/feature/x"),
            [
                Ref {
                    name: "HEAD".into(),
                    kind: "head"
                },
                Ref {
                    name: "feature/x".into(),
                    kind: "branch"
                },
            ]
        );
        // Malformed records are skipped rather than misread.
        assert!(parse_log(b"not a record\x1e\x1f\x1f\x1e").is_empty());
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let rows = layout(
            vec![
                commit("c3", &["c2"]),
                commit("c2", &["c1"]),
                commit("c1", &[]),
            ],
            "c3",
        );
        assert!(rows
            .iter()
            .all(|r| r.lane == 0 && r.lanes == 1 && r.color == 0));
        assert!(rows[0].head && !rows[1].head);
        assert_eq!(rows[0].segments, [Segment(0, 1, 0, 2, 0)]);
        assert_eq!(
            rows[1].segments,
            [Segment(0, 0, 0, 1, 0), Segment(0, 1, 0, 2, 0)]
        );
        assert_eq!(rows[2].segments, [Segment(0, 0, 0, 1, 0)]);
        assert!(rows.iter().all(|r| text(r) == "●"));
    }

    #[test]
    fn a_merged_branch_gets_its_own_lane_and_rejoins() {
        // M merges B into C; both descend from A.
        let rows = layout(
            vec![
                commit("m", &["c", "b"]),
                commit("b", &["a"]),
                commit("c", &["a"]),
                commit("a", &[]),
            ],
            "m",
        );
        assert_eq!((rows[0].lane, rows[0].lanes), (0, 2));
        assert_eq!(
            rows[0].segments,
            [Segment(0, 1, 0, 2, 0), Segment(0, 1, 1, 2, 1)]
        );
        assert_eq!(text(&rows[0]), "●─╮");
        // B continues the second lane, then hands its parent to the first.
        assert_eq!((rows[1].lane, rows[1].color), (1, 1));
        assert_eq!(
            rows[1].segments,
            [
                Segment(0, 0, 0, 2, 0),
                Segment(1, 0, 1, 1, 1),
                Segment(1, 1, 1, 2, 1)
            ]
        );
        assert_eq!(text(&rows[1]), "│ ●");
        // C finds A already expected in lane 1 and joins it.
        assert_eq!(rows[2].lane, 0);
        assert_eq!(text(&rows[2]), "●─┤");
        assert_eq!(rows[3].lane, 1);
        assert_eq!(rows[3].lanes, 2);
        assert_eq!(text(&rows[3]), "  ●");
        assert_eq!(rows[3].segments, [Segment(1, 0, 1, 1, 1)]);
    }

    #[test]
    fn unrelated_tips_reuse_free_lanes_and_wide_rows_are_cut() {
        let rows = layout(vec![commit("x", &[]), commit("y", &[])], "");
        assert_eq!((rows[0].lane, rows[1].lane), (0, 0));
        assert_ne!(rows[0].color, rows[1].color);
        let wide = HistoryRow {
            lanes: 12,
            lane: 0,
            ..rows[0].clone()
        };
        let cells = glyphs(&wide, 4);
        assert_eq!(cells.len(), 4 * 2 - 1 + 1);
        assert_eq!(cells.last().unwrap().0, '…');
    }
}
