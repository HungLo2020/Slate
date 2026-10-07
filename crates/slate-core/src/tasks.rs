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
    sync::OnceLock,
};

/// Output kept per task run.
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

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

/// Tasks for a workspace: configured ones, else detected ones.
pub fn tasks(root: &Path) -> Vec<Task> {
    let configured = std::fs::read_to_string(root.join(".slate/tasks.toml"))
        .ok()
        .and_then(|s| toml::from_str::<File>(&s).ok())
        .map(|f| f.task)
        .unwrap_or_default();
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
    if let Ok(package) = std::fs::read_to_string(root.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&package) {
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

fn ansi() -> &'static Regex {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\r").unwrap())
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
            let head = Regex::new(r"^(error|warning)(?:\[\w+\])?: (.+)$").unwrap();
            let arrow = Regex::new(r"^\s*--> (.+?):(\d+):(\d+)$").unwrap();
            for (i, line) in lines.iter().enumerate() {
                let Some(h) = head.captures(line) else {
                    continue;
                };
                // The location follows within a few lines.
                if let Some(a) = lines[i + 1..]
                    .iter()
                    .take(4)
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
            let re = Regex::new(
                r"^([^:\s][^:]*):(\d+):(?:(\d+):)?\s+(fatal error|error|warning|note):\s+(.*)$",
            )
            .unwrap();
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
            let re = Regex::new(r"^(.+?)\((\d+),(\d+)\): (error|warning) \w*\d*:? ?(.*)$").unwrap();
            let colon =
                Regex::new(r"^(.+?):(\d+):(\d+) - (error|warning) \w*\d*:? ?(.*)$").unwrap();
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
            let frame = Regex::new(r#"^\s*File "(.+)", line (\d+)"#).unwrap();
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
            let re = Regex::new(r"^([^:\s][^:]*\.\w+):(\d+):(?:(\d+):)?\s*(.+)$").unwrap();
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
    pid: u32,
    doc: u64,
    output: String,
    cwd: PathBuf,
}

impl App {
    pub(crate) fn workspace_tasks(&self) -> Vec<Task> {
        tasks(&self.root)
    }

    /// Run a task by name (empty: the default build task).
    pub(crate) fn run_task(&mut self, name: &str) -> Result<()> {
        let all = self.workspace_tasks();
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
        let mut command = Command::new("sh");
        command
            .args(["-c", &task.command])
            .current_dir(&cwd)
            .env("CARGO_TERM_COLOR", "never")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let id = self.id();
        let output = self.services.reply_sender();
        for pipe in [
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
        {
            let output = output.clone();
            let mut pipe = pipe;
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if output
                        .send(Reply::Ide(Event::TaskOutput { task: id, text }))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
        let pid = child.id();
        let exit = output.clone();
        std::thread::spawn(move || {
            let status = child.wait();
            // Let the output readers deliver what remains.
            std::thread::sleep(std::time::Duration::from_millis(50));
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
        // The output document opens next to the terminals.
        let doc = self.id();
        self.documents.insert(
            doc,
            crate::document::Document::inspection(
                format!("Task: {} · running", task.name),
                format!("$ {}\n", task.command),
            ),
        );
        let view = self.id();
        self.views.insert(
            view,
            crate::EditorView {
                document: doc,
                ..Default::default()
            },
        );
        let preferred = self
            .layout
            .panes()
            .into_iter()
            .find(|p| {
                self.layout.tabs(*p).is_some_and(|t| {
                    t.iter()
                        .any(|v| matches!(v, crate::layout::View::Terminal(_)))
                })
            })
            .unwrap_or(self.focus);
        if let Some(pane) = self.place_view(crate::layout::View::Editor(view), preferred) {
            self.focus = pane;
        }
        self.status = format!("Running {}…", task.name);
        self.tasks.insert(
            id,
            Running {
                task,
                pid,
                doc,
                output: String::new(),
                cwd,
            },
        );
        Ok(())
    }

    pub(crate) fn task_event(&mut self, event: Event) {
        match event {
            Event::TaskOutput { task, text } => {
                let Some(running) = self.tasks.get_mut(&task) else {
                    return;
                };
                let text = plain(&text);
                if running.output.len() + text.len() > OUTPUT_LIMIT {
                    return;
                }
                running.output.push_str(&text);
                let doc = running.doc;
                if let Some(d) = self.documents.get_mut(&doc) {
                    d.append_inspection(&text);
                }
                // Follow the output in views that are at its end.
                let len = self.documents.get(&doc).map_or(0, |d| d.len());
                for v in self.views.values_mut().filter(|v| v.document == doc) {
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
                        format!(" · {errors} errors, {warnings} warnings (problems)")
                    } else {
                        String::new()
                    }
                );
            }
            _ => {}
        }
    }

    /// Stop running tasks.
    pub(crate) fn stop_tasks(&mut self) -> Result<()> {
        if self.tasks.is_empty() {
            bail!("No task is running");
        }
        for running in self.tasks.values() {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(running.pid as i32), libc::SIGTERM);
            }
        }
        self.status = "Stopping tasks…".into();
        Ok(())
    }

    /// The task picker.
    pub(crate) fn task_picker(&mut self) -> Result<()> {
        let tasks = self.workspace_tasks();
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
    fn tasks_are_configured_or_detected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts": {"build": "tsc", "lint": "eslint ."}}"#,
        )
        .unwrap();
        let names: Vec<String> = tasks(dir.path()).into_iter().map(|t| t.name).collect();
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
        let configured = tasks(dir.path());
        assert_eq!(configured.len(), 1);
        assert_eq!(configured[0].matcher, "gcc");
    }
}
