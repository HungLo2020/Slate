//! Build and run tasks. Tasks come from `.slate/tasks.toml` in the
//! workspace, or are detected from the project (Cargo, Make, CMake, Go,
//! npm). Output streams into a read-only document, and a problem matcher
//! turns compiler messages into diagnostics.
//!
//! ```toml
//! [[task]]
//! name = "build"
//! command = "cargo build"
//! group = "build"      # the default task for run-build-task
//! matcher = "rustc"    # gcc, rustc, tsc, python, generic or none
//! cwd = "crates/app"   # relative to the workspace
//! ```
use crate::{ide::Event, lsp::Diagnostic, services::Reply, App};
use anyhow::{bail, Result};
use regex::Regex;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

/// Output kept per task run.
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// Output without a line break is delivered in pieces of this size, so it
/// cannot grow without bound before reaching the output document.
const CHUNK: usize = 64 * 1024;
/// How long output may still arrive after a task has exited: a process it
/// left running may hold its output open indefinitely.
const DRAIN: Duration = Duration::from_millis(300);

/// Take the output at the start of `pending` that is ready to show: whole
/// lines, or everything when `all`, or a piece of `CHUNK` bytes cut at a
/// character boundary when no line ends that soon. A lone carriage return
/// (progress output redrawing a line) ends a line too, so that output reads
/// as successive lines rather than one run-on line.
fn take_output(pending: &mut Vec<u8>, all: bool) -> Option<String> {
    let end = if all {
        pending.len()
    } else if let Some(i) = (0..pending.len()).rev().find(|&i| {
        // A trailing `\r` waits: a `\n` may follow.
        pending[i] == b'\n' || (pending[i] == b'\r' && i + 1 < pending.len())
    }) {
        i + 1
    } else if pending.len() >= CHUNK {
        chunk_end(&pending[..CHUNK])
    } else {
        0
    };
    if end == 0 {
        return None;
    }
    let mut bytes: Vec<u8> = pending.drain(..end).collect();
    // Also a carriage return that ends the output: nothing follows it.
    let next = pending.first().copied();
    for i in 0..bytes.len() {
        let following = bytes.get(i + 1).copied().or(next);
        if bytes[i] == b'\r' && following != Some(b'\n') {
            bytes[i] = b'\n';
        }
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// How much of `piece` to deliver without splitting a character: all of
/// it, unless it ends inside a UTF-8 sequence, which then waits.
fn chunk_end(piece: &[u8]) -> usize {
    let len = piece.len();
    // The last character's first byte: back over continuation bytes.
    let Some(start) = (len.saturating_sub(4)..len)
        .rev()
        .find(|&i| piece[i] & 0xC0 != 0x80)
    else {
        return len;
    };
    let width = match piece[start] {
        b if b & 0xE0 == 0xC0 => 2,
        b if b & 0xF0 == 0xE0 => 3,
        b if b & 0xF8 == 0xF0 => 4,
        _ => 1,
    };
    if start > 0 && start + width > len {
        start
    } else {
        len
    }
}

/// A stream's output not yet delivered. Its reader delivers as output
/// arrives; once the task has exited and `DRAIN` has passed, the waiting
/// thread delivers what is left and closes it.
#[derive(Default)]
struct Stream {
    pending: Vec<u8>,
    closed: bool,
}

/// A task's process group. Signalled only while its leader is unreaped:
/// until then the group's ID cannot belong to anyone else.
struct Group {
    pid: u32,
    reaped: bool,
    /// A stop was requested: what is left of the group when the leader
    /// exits is killed.
    stopping: bool,
}
impl Group {
    fn signal(&self, signal: i32) {
        if !self.reaped {
            crate::process::kill_group(self.pid, signal);
        }
    }
}

/// Lock state shared with a task's threads. A thread that panicked while
/// holding it must not keep the task from ending or being stopped.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Task {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub matcher: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    task: Vec<Task>,
}

fn task(name: &str, command: &str, group: &str, matcher: &str) -> Task {
    Task {
        name: name.into(),
        command: command.into(),
        group: group.into(),
        matcher: matcher.into(),
        cwd: None,
    }
}

/// Tasks for a workspace: configured ones, else detected ones. A
/// `tasks.toml` that does not parse is added to `errors`.
pub fn tasks(root: &Path, errors: &mut Vec<String>) -> Vec<Task> {
    let configured = crate::config::list(&root.join(".slate/tasks.toml"), |f: File| f.task, errors);
    if !configured.is_empty() {
        return configured;
    }
    let mut detected = Vec::new();
    if root.join("Cargo.toml").exists() {
        detected.extend([
            task("cargo build", "cargo build", "build", "rustc"),
            task("cargo test", "cargo test", "test", "rustc"),
            task("cargo run", "cargo run", "", "rustc"),
            task("cargo clippy", "cargo clippy", "", "rustc"),
        ]);
    }
    if root.join("Makefile").exists() || root.join("makefile").exists() {
        detected.push(task("make", "make", "build", "gcc"));
        detected.push(task("make test", "make test", "test", "gcc"));
    }
    if root.join("CMakeLists.txt").exists() {
        detected.push(task(
            "cmake build",
            "cmake -S . -B build && cmake --build build",
            "build",
            "gcc",
        ));
    }
    if root.join("go.mod").exists() {
        detected.push(task("go build", "go build ./...", "build", "generic"));
        detected.push(task("go test", "go test ./...", "test", "generic"));
    }
    // Detection runs before trust: never block on a FIFO or read a huge file.
    if let Ok(package) =
        crate::fsio::read_regular(&root.join("package.json"), crate::config::MAX_CONFIG_BYTES)
    {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&package) {
            for (name, _) in json["scripts"].as_object().into_iter().flatten() {
                let group = match name.as_str() {
                    "build" => "build",
                    "test" => "test",
                    _ => "",
                };
                detected.push(task(
                    &format!("npm run {name}"),
                    &format!("npm run {name}"),
                    group,
                    "tsc",
                ));
            }
        }
    }
    if root.join("pyproject.toml").exists() || root.join("setup.py").exists() {
        detected.push(task("pytest", "python3 -m pytest", "test", "python"));
    }
    detected
}

/// A regular expression compiled once.
macro_rules! regex {
    ($pattern:expr) => {{
        static REGEX: OnceLock<Regex> = OnceLock::new();
        REGEX.get_or_init(|| Regex::new($pattern).unwrap())
    }};
}

fn ansi() -> &'static Regex {
    regex!(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\r")
}

/// Remove colour codes and carriage returns from tool output.
pub fn plain(text: &str) -> String {
    ansi().replace_all(text, "").into_owned()
}

fn severity(word: &str) -> u8 {
    match word.to_ascii_lowercase().as_str() {
        "error" | "fatal error" => 1,
        "warning" => 2,
        "note" | "info" => 3,
        _ => 1,
    }
}

fn diagnostic(line: usize, column: usize, severity: u8, message: &str) -> Diagnostic {
    Diagnostic {
        start: (line.saturating_sub(1), column.saturating_sub(1)),
        end: (line.saturating_sub(1), column.saturating_sub(1)),
        severity,
        message: message.trim().to_string(),
        source: "task".into(),
        raw: serde_json::Value::Null,
    }
}

/// Problems in tool output, by file (relative paths resolve against `cwd`).
pub fn problems(matcher: &str, output: &str, cwd: &Path) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
    let mut found: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
    let mut add = |file: &str, d: Diagnostic| {
        let path = Path::new(file.trim());
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let path = path.canonicalize().unwrap_or(path);
        let list = found.entry(path).or_default();
        if !list.contains(&d) {
            list.push(d);
        }
    };
    let lines: Vec<&str> = output.lines().collect();
    match matcher {
        "rustc" => {
            let head = regex!(r"^(error|warning)(?:\[\w+\])?: (.+)$");
            let arrow = regex!(r"^\s*--> (.+?):(\d+):(\d+)$");
            for (i, line) in lines.iter().enumerate() {
                let Some(h) = head.captures(line) else {
                    continue;
                };
                // The location follows within a few lines, before the next
                // message (summaries such as "generated 2 warnings" have none).
                if let Some(a) = lines[i + 1..]
                    .iter()
                    .take(4)
                    .take_while(|l| !head.is_match(l))
                    .find_map(|l| arrow.captures(l))
                {
                    add(
                        &a[1],
                        diagnostic(
                            a[2].parse().unwrap_or(1),
                            a[3].parse().unwrap_or(1),
                            severity(&h[1]),
                            &h[2],
                        ),
                    );
                }
            }
        }
        "gcc" => {
            let re = regex!(
                r"^([^:\s][^:]*):(\d+):(?:(\d+):)?\s+(fatal error|error|warning|note):\s+(.*)$"
            );
            for line in &lines {
                if let Some(c) = re.captures(line) {
                    if &c[4] == "note" {
                        continue;
                    }
                    let column = c.get(3).map_or(1, |m| m.as_str().parse().unwrap_or(1));
                    add(
                        &c[1],
                        diagnostic(c[2].parse().unwrap_or(1), column, severity(&c[4]), &c[5]),
                    );
                }
            }
        }
        "tsc" => {
            let re = regex!(r"^(.+?)\((\d+),(\d+)\): (error|warning) \w*\d*:? ?(.*)$");
            let colon = regex!(r"^(.+?):(\d+):(\d+) - (error|warning) \w*\d*:? ?(.*)$");
            for line in &lines {
                if let Some(c) = re.captures(line).or_else(|| colon.captures(line)) {
                    add(
                        &c[1],
                        diagnostic(
                            c[2].parse().unwrap_or(1),
                            c[3].parse().unwrap_or(1),
                            severity(&c[4]),
                            &c[5],
                        ),
                    );
                }
            }
        }
        "python" => {
            let frame = regex!(r#"^\s*File "(.+)", line (\d+)"#);
            let mut last: Option<(String, usize)> = None;
            for line in &lines {
                if let Some(c) = frame.captures(line) {
                    last = Some((c[1].to_string(), c[2].parse().unwrap_or(1)));
                } else if let Some((file, number)) = last.as_ref() {
                    // The exception line ends a traceback.
                    let trimmed = line.trim();
                    if !line.starts_with(' ')
                        && trimmed.contains(':')
                        && !trimmed.starts_with("Traceback")
                    {
                        add(file, diagnostic(*number, 1, 1, trimmed));
                        last = None;
                    }
                }
            }
        }
        "generic" => {
            let re = regex!(r"^([^:\s][^:]*\.\w+):(\d+):(?:(\d+):)?\s*(.+)$");
            for line in &lines {
                if let Some(c) = re.captures(line) {
                    let column = c.get(3).map_or(1, |m| m.as_str().parse().unwrap_or(1));
                    let message = &c[4];
                    let level = if message.to_lowercase().starts_with("warning") {
                        2
                    } else {
                        1
                    };
                    add(
                        &c[1],
                        diagnostic(c[2].parse().unwrap_or(1), column, level, message),
                    );
                }
            }
        }
        _ => {}
    }
    found
}

pub(crate) struct Running {
    task: Task,
    group: Arc<Mutex<Group>>,
    doc: u64,
    output: String,
    cwd: PathBuf,
    /// Output reached the limit; the rest is dropped.
    truncated: bool,
    /// When a stop was requested.
    stopping: Option<std::time::Instant>,
    /// Forced after ignoring the stop request.
    killed: bool,
}

impl App {
    pub fn tasks_running(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// The workspace's tasks; a broken `tasks.toml` is an error rather
    /// than silently replaced by detected tasks.
    pub(crate) fn workspace_tasks(&self) -> Result<Vec<Task>> {
        let mut errors = Vec::new();
        let found = tasks(&self.root, &mut errors);
        match errors.into_iter().next() {
            Some(error) => bail!("{error}"),
            None => Ok(found),
        }
    }

    /// Run a task by name (empty: the default build task).
    pub(crate) fn run_task(&mut self, name: &str) -> Result<()> {
        let all = self.workspace_tasks()?;
        let chosen = if name.is_empty() {
            all.iter()
                .find(|t| t.group == "build")
                .or(all.first())
                .cloned()
        } else {
            all.iter().find(|t| t.name == name).cloned()
        };
        let Some(task) = chosen else {
            bail!(
                "No tasks. Add .slate/tasks.toml or open a Cargo, Make, CMake, Go or npm project"
            );
        };
        if !self.trusted {
            self.ask_trust(
                "Tasks run commands from this folder",
                crate::Command::Action {
                    name: "run-task".into(),
                    argument: task.name.clone(),
                },
            );
            return Ok(());
        }
        if self.tasks.values().any(|r| r.task.name == task.name) {
            bail!("{} is already running", task.name);
        }
        let cwd = task
            .cwd
            .as_ref()
            .map(|c| self.root.join(c))
            .unwrap_or_else(|| self.root.clone());
        if !self.trusted_path(&cwd) {
            self.ask_trust_for(
                &cwd,
                "Tasks run commands from this folder",
                crate::Command::Action {
                    name: "run-task".into(),
                    argument: task.name.clone(),
                },
            );
            return Ok(());
        }
        let mut command = Command::new("sh");
        crate::process::sanitize(&mut command)
            .args(["-c", &task.command])
            .current_dir(&cwd)
            .env("CARGO_TERM_COLOR", "never")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::isolate(&mut command);
        let mut child = command.spawn()?;
        let id = self.id();
        let output = self.services.reply_sender();
        let pid = child.id();
        let group = Arc::new(Mutex::new(Group {
            pid,
            reaped: false,
            stopping: false,
        }));
        let (finished, readers_done) = std::sync::mpsc::channel::<()>();
        // Each stream is delivered in whole lines (or bounded pieces cut at
        // character boundaries), so stdout and stderr interleave only
        // between lines and UTF-8 is never split.
        let streams: Vec<Arc<Mutex<Stream>>> = [
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .map(|mut pipe| {
            let stream = Arc::new(Mutex::new(Stream::default()));
            let (output, shared, finished) = (output.clone(), stream.clone(), finished.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let send = |text: String| {
                    output
                        .send(Reply::Ide(Event::TaskOutput { task: id, text }))
                        .is_ok()
                };
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut stream = lock(&shared);
                    if stream.closed {
                        return;
                    }
                    stream.pending.extend_from_slice(&buf[..n]);
                    while let Some(text) = take_output(&mut stream.pending, false) {
                        if !send(text) {
                            return;
                        }
                    }
                }
                let mut stream = lock(&shared);
                if !stream.closed {
                    stream.closed = true;
                    if let Some(text) = take_output(&mut stream.pending, true) {
                        send(text);
                    }
                }
                drop(stream);
                let _ = finished.send(());
            });
            stream
        })
        .collect();
        drop(finished);
        let exit = output.clone();
        let waiter = group.clone();
        std::thread::spawn(move || {
            // Exited, but not reaped while the group is still signalled.
            #[cfg(unix)]
            crate::process::wait_exited(pid);
            #[cfg(not(unix))]
            let _ = child.wait();
            let status = {
                let mut group = lock(&waiter);
                #[cfg(unix)]
                if group.stopping {
                    group.signal(libc::SIGKILL);
                }
                let status = child.wait();
                group.reaped = true;
                status
            };
            // Output read before the exit arrives first. A process the task
            // left running may keep its output open: it does not hold the
            // exit back.
            let deadline = Instant::now() + DRAIN;
            for _ in &streams {
                let left = deadline.saturating_duration_since(Instant::now());
                if readers_done.recv_timeout(left).is_err() {
                    break;
                }
            }
            for stream in &streams {
                let mut stream = lock(stream);
                if !stream.closed {
                    stream.closed = true;
                    if let Some(text) = take_output(&mut stream.pending, true) {
                        let _ = exit.send(Reply::Ide(Event::TaskOutput { task: id, text }));
                    }
                }
            }
            let (code, error) = match status {
                Ok(s) => (s.code(), String::new()),
                Err(e) => (None, e.to_string()),
            };
            let _ = exit.send(Reply::Ide(Event::TaskExited {
                task: id,
                code,
                error,
            }));
        });
        // One output document per task, reused by later runs, shown next to
        // the terminals without taking focus from the editor.
        let label = format!("Task: {}", task.name);
        let start = format!("$ {}\n", task.command);
        let existing = self
            .documents
            .iter()
            .find(|(_, d)| {
                d.label
                    .as_deref()
                    .is_some_and(|l| l == label || l.starts_with(&format!("{label} · ")))
            })
            .map(|(id, _)| *id);
        let doc = match existing {
            Some(doc) => {
                let d = self.documents.get_mut(&doc).unwrap();
                let len = d.len();
                d.replace_inspection(0, len, &start);
                doc
            }
            None => {
                let doc = self.id();
                self.documents.insert(
                    doc,
                    crate::document::Document::inspection(label.clone(), start),
                );
                doc
            }
        };
        if let Some(d) = self.documents.get_mut(&doc) {
            d.label = Some(format!("{label} · running"));
            d.generation += 1;
        }
        self.clamp_views(doc);
        self.show_output(doc);
        self.status = format!("Running {}…", task.name);
        self.tasks.insert(
            id,
            Running {
                task,
                group,
                doc,
                output: String::new(),
                cwd,
                truncated: false,
                stopping: None,
                killed: false,
            },
        );
        Ok(())
    }

    /// Show an output document (task or debugger) beside the terminals, or
    /// in a split below the editor, keeping focus and the editor visible.
    pub(crate) fn show_output(&mut self, doc: u64) {
        let shown = self.layout.views().iter().any(|v| {
            matches!(v, crate::layout::View::Editor(id) if self.views.get(id).is_some_and(|v| v.document == doc))
        });
        if shown {
            return;
        }
        let view = self.id();
        self.views.insert(
            view,
            crate::EditorView {
                document: doc,
                ..Default::default()
            },
        );
        let terminals = self.layout.panes().into_iter().find(|p| {
            self.layout.tabs(*p).is_some_and(|t| {
                t.iter()
                    .any(|v| matches!(v, crate::layout::View::Terminal(_)))
            })
        });
        let focus = self.focus;
        let placed = match terminals {
            Some(pane) => self.place_view(crate::layout::View::Editor(view), pane),
            None if self.can_split() && !self.editor_only => {
                let (pane, split) = (self.id(), self.id());
                self.layout.split(
                    focus,
                    crate::layout::Axis::Vertical,
                    pane,
                    split,
                    crate::layout::View::Editor(view),
                );
                Some(pane)
            }
            None => self.place_view_background(crate::layout::View::Editor(view), focus),
        };
        if placed.is_none() {
            self.views.remove(&view);
        }
        self.focus = focus;
    }

    pub(crate) fn task_event(&mut self, event: Event) {
        match event {
            Event::TaskOutput { task, text } => {
                let Some(running) = self.tasks.get_mut(&task) else {
                    return;
                };
                if running.truncated {
                    return;
                }
                let mut text = plain(&text);
                if running.output.len() + text.len() > OUTPUT_LIMIT {
                    running.truncated = true;
                    text = format!(
                        "\n[Output beyond {} MiB is not shown]\n",
                        OUTPUT_LIMIT / (1024 * 1024)
                    );
                } else {
                    running.output.push_str(&text);
                }
                let doc = running.doc;
                let Some(d) = self.documents.get_mut(&doc) else {
                    return;
                };
                let old_len = d.len();
                d.append_inspection(&text);
                let len = d.len();
                // Follow the output only in views already at its end, so
                // reading or selecting earlier output is not disturbed.
                for v in self
                    .views
                    .values_mut()
                    .filter(|v| v.document == doc && v.cursor == old_len && v.anchor.is_none())
                {
                    v.cursor = len;
                    v.manual_scroll = false;
                }
            }
            Event::TaskExited { task, code, error } => {
                let Some(running) = self.tasks.remove(&task) else {
                    return;
                };
                let found = problems(&running.task.matcher, &running.output, &running.cwd);
                let (errors, warnings) = found.values().flatten().fold((0, 0), |(e, w), d| match d
                    .severity
                {
                    1 => (e + 1, w),
                    2 => (e, w + 1),
                    _ => (e, w),
                });
                self.set_task_diagnostics(found);
                let outcome = match code {
                    Some(0) => "succeeded".to_string(),
                    Some(code) => format!("failed (exit {code})"),
                    None if error.is_empty() => "stopped".to_string(),
                    None => format!("could not run: {error}"),
                };
                if let Some(d) = self.documents.get_mut(&running.doc) {
                    d.append_inspection(&format!("\n[{} {outcome}]\n", running.task.name));
                    d.label = Some(format!("Task: {} · {outcome}", running.task.name));
                    d.generation += 1;
                }
                self.status = format!(
                    "{} {outcome}{}",
                    running.task.name,
                    if errors + warnings > 0 {
                        format!(
                            " · {}, {} (problems)",
                            crate::counted(errors, "error", "errors"),
                            crate::counted(warnings, "warning", "warnings")
                        )
                    } else {
                        String::new()
                    }
                );
            }
            _ => {}
        }
    }

    /// Stop running tasks: ask them to end, then force them (see
    /// `expire_tasks`).
    pub(crate) fn stop_tasks(&mut self) -> Result<()> {
        if self.tasks.is_empty() {
            bail!("No task is running");
        }
        for running in self.tasks.values_mut() {
            let mut group = lock(&running.group);
            group.stopping = true;
            #[cfg(unix)]
            group.signal(libc::SIGTERM);
            running.stopping.get_or_insert_with(std::time::Instant::now);
        }
        self.status = "Stopping tasks…".into();
        Ok(())
    }

    /// Force tasks that ignored a stop request, once.
    pub(crate) fn expire_tasks(&mut self) {
        for running in self.tasks.values_mut() {
            if !running.killed
                && running
                    .stopping
                    .is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(3))
            {
                running.killed = true;
                #[cfg(unix)]
                lock(&running.group).signal(libc::SIGKILL);
            }
        }
    }

    /// Stop every task now (when Slate exits).
    pub(crate) fn kill_tasks(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        for running in self.tasks.values() {
            let mut group = lock(&running.group);
            group.stopping = true;
            #[cfg(unix)]
            group.signal(libc::SIGTERM);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        for running in self.tasks.values() {
            #[cfg(unix)]
            lock(&running.group).signal(libc::SIGKILL);
        }
        self.tasks.clear();
    }

    /// The task picker.
    pub(crate) fn task_picker(&mut self) -> Result<()> {
        let tasks = self.workspace_tasks()?;
        if tasks.is_empty() {
            bail!(
                "No tasks. Add .slate/tasks.toml or open a Cargo, Make, CMake, Go or npm project"
            );
        }
        let mut picker = crate::picker::Picker::new("tasks", "Run task", true);
        picker.entries = tasks
            .into_iter()
            .map(|t| crate::picker::Entry {
                label: t.name.clone(),
                detail: if t.group.is_empty() {
                    t.command.clone()
                } else {
                    format!("{} · {}", t.group, t.command)
                },
                kind: "task".into(),
                target: crate::picker::Target::Command(crate::Command::Action {
                    name: "run-task".into(),
                    argument: t.name,
                }),
            })
            .collect();
        picker.filter();
        self.picker = Some(picker);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matchers_read_common_compiler_output() {
        let cwd = Path::new("/w");
        let rust = "warning: unused variable: `x`\n --> src/main.rs:3:9\n  |\nerror[E0425]: cannot find value `y` in this scope\n  --> src/lib.rs:10:5\nerror: could not compile `app`\n";
        let found = problems("rustc", rust, cwd);
        let main = &found[&PathBuf::from("/w/src/main.rs")];
        assert_eq!((main[0].start, main[0].severity), ((2, 8), 2));
        let lib = &found[&PathBuf::from("/w/src/lib.rs")];
        assert_eq!(lib[0].message, "cannot find value `y` in this scope");
        assert_eq!(found.len(), 2);

        let gcc = "main.c: In function 'main':\nmain.c:4:5: error: unknown type name 'foo'\nmain.c:9:1: warning: control reaches end\nmain.c:4:5: note: here\n";
        let found = problems("gcc", gcc, cwd);
        let list = &found[&PathBuf::from("/w/main.c")];
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].start, list[0].severity), ((3, 4), 1));

        let tsc = "src/a.ts(2,7): error TS2322: Type 'string' is not assignable\nsrc/b.ts:5:1 - warning TS6133: unused\n";
        let found = problems("tsc", tsc, cwd);
        assert_eq!(found[&PathBuf::from("/w/src/a.ts")][0].start, (1, 6));
        assert_eq!(found[&PathBuf::from("/w/src/b.ts")][0].severity, 2);

        let py = "Traceback (most recent call last):\n  File \"/w/app.py\", line 3, in <module>\n    main()\n  File \"/w/lib.py\", line 7, in main\n    x = y\nNameError: name 'y' is not defined\n";
        let found = problems("python", py, cwd);
        assert_eq!(
            found[&PathBuf::from("/w/lib.py")][0].message,
            "NameError: name 'y' is not defined"
        );

        let go = "./pkg/x.go:12:2: undefined: foo\n";
        assert_eq!(
            problems("generic", go, cwd)[&PathBuf::from("/w/./pkg/x.go")][0].start,
            (11, 1)
        );
        assert!(problems("none", go, cwd).is_empty());
        assert_eq!(plain("\x1b[1m\x1b[31merror\x1b[0m: x\r\n"), "error: x\n");
    }

    #[test]
    fn output_is_delivered_in_lines_or_bounded_pieces() {
        let mut pending = b"one\ntwo\r\nthr".to_vec();
        assert_eq!(take_output(&mut pending, false).unwrap(), "one\ntwo\r\n");
        assert_eq!(pending, b"thr");
        assert_eq!(take_output(&mut pending, false), None);
        // Progress output: a lone carriage return ends a line, a trailing
        // one waits for what follows.
        let mut pending = b"\r10%\r20%\r".to_vec();
        assert_eq!(take_output(&mut pending, false).unwrap(), "\n10%\n");
        assert_eq!(pending, b"20%\r");
        pending.extend_from_slice(b"\ndone\n");
        assert_eq!(take_output(&mut pending, false).unwrap(), "20%\r\ndone\n");
        // No line break at all: pieces of at most CHUNK bytes, never
        // splitting a character.
        let mut pending = b"a".to_vec();
        pending.extend("é".repeat(CHUNK).as_bytes());
        let mut text = String::new();
        while let Some(piece) = take_output(&mut pending, false) {
            assert!(piece.len() <= CHUNK);
            text.push_str(&piece);
        }
        assert!(pending.len() < CHUNK);
        text.push_str(&take_output(&mut pending, true).unwrap());
        assert!(!text.contains('\u{fffd}'));
        assert_eq!(text.chars().filter(|c| *c == 'é').count(), CHUNK);
    }

    #[test]
    fn pieces_around_the_chunk_size_never_split_characters() {
        // Reads are 8 KiB, so exactly CHUNK pending bytes is common.
        for len in [CHUNK - 1, CHUNK, CHUNK + 1] {
            let mut pending = vec![b'x'; len];
            let taken = take_output(&mut pending, false);
            assert_eq!(taken.map(|t| t.len()), (len >= CHUNK).then_some(CHUNK));
        }
        // Characters of every width straddling the cut at each offset.
        for c in ['é', '€', '😀'] {
            for offset in 0..4 {
                let mut pending = vec![b'x'; CHUNK - offset];
                pending.extend(c.to_string().repeat(4).as_bytes());
                let original = pending.clone();
                let mut text = take_output(&mut pending, false).unwrap_or_default();
                assert!(!text.contains('\u{fffd}'), "{c} at {offset}");
                text.push_str(&take_output(&mut pending, true).unwrap_or_default());
                assert_eq!(text.as_bytes(), &original[..], "{c} at {offset}");
            }
        }
        // Exactly CHUNK bytes ending inside a character: it waits.
        let mut pending = vec![b'x'; CHUNK - 1];
        pending.push("é".as_bytes()[0]);
        assert_eq!(take_output(&mut pending, false).unwrap().len(), CHUNK - 1);
        assert_eq!(pending, ["é".as_bytes()[0]]);
        // A carriage return that ends the output ends a line.
        let mut pending = b"50%\r".to_vec();
        assert_eq!(take_output(&mut pending, false), None);
        assert_eq!(take_output(&mut pending, true).unwrap(), "50%\n");
    }

    #[test]
    fn tasks_are_configured_or_detected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts": {"build": "tsc", "lint": "eslint ."}}"#,
        )
        .unwrap();
        let names: Vec<String> = tasks(dir.path(), &mut Vec::new())
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(
            names.contains(&"cargo build".into()) && names.contains(&"npm run lint".into()),
            "{names:?}"
        );
        std::fs::create_dir(dir.path().join(".slate")).unwrap();
        std::fs::write(
            dir.path().join(".slate/tasks.toml"),
            "[[task]]\nname = \"all\"\ncommand = \"make all\"\ngroup = \"build\"\nmatcher = \"gcc\"\n",
        )
        .unwrap();
        let configured = tasks(dir.path(), &mut Vec::new());
        assert_eq!(configured.len(), 1);
        assert_eq!(configured[0].matcher, "gcc");
    }
}
