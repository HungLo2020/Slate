//! Debugging through the Debug Adapter Protocol. GDB (`gdb -i dap`) is the
//! default adapter; `.slate/launch.toml` can name programs and adapters:
//!
//! ```toml
//! [[launch]]
//! name = "app"
//! program = "build/app"          # relative to the workspace
//! args = ["--verbose"]
//! adapter = ["gdb", "-i", "dap"] # or another DAP adapter
//! stop-at-entry = false
//! ```
//!
//! While a session runs, a "Debug" document shows where the program
//! stopped, the call stack, local variables and program output; the editor
//! marks breakpoints and the current line.
use crate::{
    ide::Event,
    navigation::{Column, Location},
    services::Reply,
    App,
};
use anyhow::{bail, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

const CONSOLE_LIMIT: usize = 256 * 1024;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct Launch {
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub adapter: Vec<String>,
    #[serde(default)]
    pub stop_at_entry: bool,
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    launch: Vec<Launch>,
}

pub fn launches(root: &Path) -> Vec<Launch> {
    std::fs::read_to_string(root.join(".slate/launch.toml"))
        .ok()
        .and_then(|s| toml::from_str::<File>(&s).ok())
        .map(|f| f.launch)
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
struct Frame {
    id: i64,
    name: String,
    path: Option<PathBuf>,
    line: usize,
}

#[derive(Debug)]
enum Request {
    Initialize,
    Launch,
    Breakpoints,
    ConfigurationDone,
    StackTrace,
    Scopes,
    Variables,
    Evaluate(String),
    Control,
    Disconnect,
}

struct Session {
    id: u64,
    process: crate::rpc::Process,
    seq: i64,
    pending: HashMap<i64, Request>,
    launch: Launch,
    stopped: Option<(i64, String)>,
    frames: Vec<Frame>,
    locals: Vec<(String, String)>,
    console: String,
    exit: Option<i64>,
    panel: u64,
}

#[derive(Default)]
pub(crate) struct Debug {
    session: Option<Session>,
    next: u64,
    /// Breakpoint lines (zero-based) by file.
    pub breakpoints: BTreeMap<PathBuf, BTreeSet<usize>>,
}

impl App {
    pub fn debugging(&self) -> bool {
        self.debug.session.is_some()
    }

    fn debug_send(&mut self, command: &str, arguments: Value, request: Request) -> Result<()> {
        let Some(s) = self.debug.session.as_mut() else {
            bail!("No debug session");
        };
        s.seq += 1;
        let seq = s.seq;
        s.pending.insert(seq, request);
        s.process.send(&json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        }));
        Ok(())
    }

    /// Start debugging: a named launch configuration, a program path, or
    /// the only configuration there is.
    pub(crate) fn debug_start(&mut self, argument: &str) -> Result<()> {
        if self.debug.session.is_some() {
            return self.debug_control("continue");
        }
        let configured = launches(&self.root);
        let launch = match configured.iter().find(|l| l.name == argument) {
            Some(l) => l.clone(),
            None if !argument.is_empty() => Launch {
                name: argument.to_string(),
                program: argument.to_string(),
                ..Default::default()
            },
            None if configured.len() == 1 => configured[0].clone(),
            None if configured.is_empty() => {
                self.prompt = Some(crate::actions::prompt("debug-program", String::new()));
                return Ok(());
            }
            None => {
                let mut picker = crate::picker::Picker::new("launch", "Start debugging", true);
                picker.entries = configured
                    .into_iter()
                    .map(|l| crate::picker::Entry {
                        label: l.name.clone(),
                        detail: l.program.clone(),
                        kind: "launch".into(),
                        target: crate::picker::Target::Command(crate::Command::Action {
                            name: "debug-start".into(),
                            argument: l.name,
                        }),
                    })
                    .collect();
                picker.filter();
                self.picker = Some(picker);
                return Ok(());
            }
        };
        if !self.trusted {
            self.ask_trust(
                "Debugging runs programs from this folder",
                crate::Command::Action {
                    name: "debug-start".into(),
                    argument: launch.name.clone(),
                },
            );
            return Ok(());
        }
        let adapter = if launch.adapter.is_empty() {
            vec!["gdb".to_string(), "-i".into(), "dap".into()]
        } else {
            launch.adapter.clone()
        };
        if crate::fsio::which(&adapter[0]).is_none() && !Path::new(&adapter[0]).is_file() {
            bail!("{} is not installed", adapter[0]);
        }
        self.debug.next += 1;
        let id = self.debug.next;
        let output = self.services.reply_sender();
        let exit = self.services.reply_sender();
        let process = crate::rpc::Process::spawn(
            &adapter,
            &self.root,
            move |message| {
                output
                    .send(Reply::Ide(Event::Dap {
                        session: id,
                        message,
                    }))
                    .is_ok()
            },
            move |reason| {
                let _ = exit.send(Reply::Ide(Event::DapExited {
                    session: id,
                    reason,
                }));
            },
        )?;
        let panel = self.debug_panel(&launch.name);
        self.debug.session = Some(Session {
            id,
            process,
            seq: 0,
            pending: HashMap::new(),
            launch,
            stopped: None,
            frames: Vec::new(),
            locals: Vec::new(),
            console: String::new(),
            exit: None,
            panel,
        });
        self.debug_send(
            "initialize",
            json!({
                "clientID": "slate",
                "clientName": "Slate",
                "adapterID": "slate",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path",
                "supportsRunInTerminalRequest": false,
            }),
            Request::Initialize,
        )?;
        self.status = "Starting the debugger…".into();
        Ok(())
    }

    /// The Debug document, shown next to the terminals.
    fn debug_panel(&mut self, name: &str) -> u64 {
        let doc = self.id();
        self.documents.insert(
            doc,
            crate::document::Document::inspection(format!("Debug: {name}"), "Starting…\n".into()),
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
        let focus = self.focus;
        self.place_view(crate::layout::View::Editor(view), preferred);
        self.focus = focus;
        doc
    }

    fn debug_redraw(&mut self) {
        let Some(s) = self.debug.session.as_ref() else {
            return;
        };
        let mut text = String::new();
        match (&s.stopped, s.exit) {
            (_, Some(code)) => text.push_str(&format!("Exited with code {code}\n")),
            (Some((thread, reason)), _) => {
                let place = s
                    .frames
                    .first()
                    .map(|f| {
                        format!(
                            " in {} at {}:{}",
                            f.name,
                            f.path
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                            f.line
                        )
                    })
                    .unwrap_or_default();
                text.push_str(&format!("Paused ({reason}) · thread {thread}{place}\n"));
            }
            (None, None) => text.push_str("Running\n"),
        }
        if !s.frames.is_empty() {
            text.push_str("\nCall stack\n");
            for (i, f) in s.frames.iter().enumerate() {
                text.push_str(&format!(
                    "  #{i} {} {}:{}\n",
                    f.name,
                    f.path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "?".into()),
                    f.line
                ));
            }
        }
        if !s.locals.is_empty() {
            text.push_str("\nLocals\n");
            for (name, value) in &s.locals {
                text.push_str(&format!("  {name} = {value}\n"));
            }
        }
        if !s.console.is_empty() {
            text.push_str("\nOutput\n");
            text.push_str(&s.console);
            if !s.console.ends_with('\n') {
                text.push('\n');
            }
        }
        let panel = s.panel;
        let label = format!(
            "Debug: {}{}",
            s.launch.name,
            if s.exit.is_some() {
                " · exited"
            } else if s.stopped.is_some() {
                " · paused"
            } else {
                " · running"
            }
        );
        if let Some(d) = self.documents.get_mut(&panel) {
            let len = d.len();
            d.replace_inspection(0, len, &text);
            d.label = Some(label);
            d.generation += 1;
            self.clamp_views(panel);
        }
    }

    pub(crate) fn debug_event(&mut self, event: Event) {
        match event {
            Event::Dap { session, message } => {
                if self.debug.session.as_ref().is_some_and(|s| s.id == session) {
                    if let Err(e) = self.debug_message(message) {
                        self.status = format!("Debugger: {e:#}");
                    }
                }
            }
            Event::DapExited { session, reason } => {
                if self.debug.session.as_ref().is_some_and(|s| s.id == session) {
                    self.debug_end(&format!("Debugger stopped ({reason})"));
                }
            }
            _ => {}
        }
    }

    fn debug_end(&mut self, status: &str) {
        self.debug_redraw();
        if let Some(mut s) = self.debug.session.take() {
            s.process.kill();
        }
        self.status = status.to_string();
        self.revision += 1;
    }

    fn debug_message(&mut self, message: Value) -> Result<()> {
        match message["type"].as_str() {
            Some("response") => {
                let seq = message["request_seq"].as_i64().unwrap_or(0);
                let request = self
                    .debug
                    .session
                    .as_mut()
                    .and_then(|s| s.pending.remove(&seq));
                let Some(request) = request else {
                    return Ok(());
                };
                if message["success"] == Value::Bool(false) {
                    let text = message["message"]
                        .as_str()
                        .unwrap_or("request failed")
                        .to_string();
                    if let Request::Evaluate(expression) = request {
                        self.debug_console(&format!("> {expression}\n  {text}\n"));
                        return Ok(());
                    }
                    if matches!(request, Request::Launch) {
                        self.debug_end(&format!("Could not start: {text}"));
                        return Ok(());
                    }
                    bail!("{text}");
                }
                self.debug_response(request, &message["body"])
            }
            Some("event") => self.debug_notification(
                message["event"].as_str().unwrap_or_default(),
                &message["body"],
            ),
            // Reverse requests (runInTerminal…) are not supported.
            Some("request") => {
                if let Some(s) = self.debug.session.as_mut() {
                    s.seq += 1;
                    let seq = s.seq;
                    s.process.send(&json!({
                        "seq": seq,
                        "type": "response",
                        "request_seq": message["seq"],
                        "success": false,
                        "command": message["command"],
                        "message": "not supported",
                    }));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn debug_response(&mut self, request: Request, body: &Value) -> Result<()> {
        match request {
            Request::Initialize => {
                let launch = self.debug.session.as_ref().unwrap().launch.clone();
                let resolve = |p: &str| {
                    let path = Path::new(p);
                    if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        self.root.join(path)
                    }
                };
                let program = resolve(&launch.program);
                let cwd = launch
                    .cwd
                    .as_deref()
                    .map(resolve)
                    .unwrap_or_else(|| self.root.clone());
                self.debug_send(
                    "launch",
                    json!({
                        "program": program,
                        "args": launch.args,
                        "cwd": cwd,
                        "stopAtBeginningOfMainSubprogram": launch.stop_at_entry,
                        "stopOnEntry": launch.stop_at_entry,
                    }),
                    Request::Launch,
                )?;
            }
            Request::StackTrace => {
                let frames: Vec<Frame> = body["stackFrames"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| Frame {
                        id: f["id"].as_i64().unwrap_or(0),
                        name: f["name"].as_str().unwrap_or("?").to_string(),
                        path: f["source"]["path"].as_str().map(PathBuf::from),
                        line: f["line"].as_u64().unwrap_or(0) as usize,
                    })
                    .collect();
                let top = frames.first().cloned();
                if let Some(s) = self.debug.session.as_mut() {
                    s.frames = frames;
                    s.locals.clear();
                }
                if let Some(frame) = top {
                    self.debug_send("scopes", json!({"frameId": frame.id}), Request::Scopes)?;
                    // Show where the program stopped.
                    if let (Some(path), true) = (frame.path.clone(), frame.line > 0) {
                        if path.exists() {
                            self.goto(Location {
                                path,
                                line: frame.line - 1,
                                column: Column::Byte(0),
                            })?;
                        }
                    }
                }
                self.debug_redraw();
            }
            Request::Scopes => {
                let locals = body["scopes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|s| {
                        let name = s["name"].as_str().unwrap_or_default().to_lowercase();
                        name.contains("local") || s["presentationHint"] == "locals"
                    })
                    .or_else(|| body["scopes"].as_array().and_then(|a| a.first()))
                    .and_then(|s| s["variablesReference"].as_i64());
                if let Some(reference) = locals.filter(|r| *r > 0) {
                    self.debug_send(
                        "variables",
                        json!({"variablesReference": reference}),
                        Request::Variables,
                    )?;
                }
            }
            Request::Variables => {
                let locals = body["variables"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| {
                        (
                            v["name"].as_str().unwrap_or("?").to_string(),
                            v["value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect();
                if let Some(s) = self.debug.session.as_mut() {
                    s.locals = locals;
                }
                self.debug_redraw();
            }
            Request::Evaluate(expression) => {
                let result = body["result"].as_str().unwrap_or_default();
                self.status = format!("{expression} = {result}");
                self.debug_console(&format!("> {expression}\n  {result}\n"));
            }
            Request::Launch
            | Request::Breakpoints
            | Request::ConfigurationDone
            | Request::Control
            | Request::Disconnect => {}
        }
        Ok(())
    }

    fn debug_console(&mut self, text: &str) {
        if let Some(s) = self.debug.session.as_mut() {
            s.console.push_str(text);
            if s.console.len() > CONSOLE_LIMIT {
                let mut cut = s.console.len() - CONSOLE_LIMIT;
                while !s.console.is_char_boundary(cut) {
                    cut += 1;
                }
                s.console.drain(..cut);
            }
        }
        self.debug_redraw();
    }

    fn debug_notification(&mut self, event: &str, body: &Value) -> Result<()> {
        match event {
            "initialized" => {
                let files: Vec<PathBuf> = self.debug.breakpoints.keys().cloned().collect();
                for file in files {
                    self.send_breakpoints(&file)?;
                }
                self.debug_send("configurationDone", json!({}), Request::ConfigurationDone)?;
                self.status = "Debugging".into();
            }
            "stopped" => {
                let thread = body["threadId"].as_i64().unwrap_or(1);
                let reason = body["reason"].as_str().unwrap_or("paused").to_string();
                if let Some(s) = self.debug.session.as_mut() {
                    s.stopped = Some((thread, reason.clone()));
                }
                self.status = format!("Paused: {reason}");
                self.debug_send(
                    "stackTrace",
                    json!({"threadId": thread, "startFrame": 0, "levels": 32}),
                    Request::StackTrace,
                )?;
            }
            "continued" => {
                if let Some(s) = self.debug.session.as_mut() {
                    s.stopped = None;
                    s.frames.clear();
                    s.locals.clear();
                }
                self.debug_redraw();
            }
            "output" => {
                if body["category"] != "telemetry" {
                    let text = body["output"].as_str().unwrap_or_default().to_string();
                    self.debug_console(&text);
                }
            }
            "exited" => {
                let code = body["exitCode"].as_i64().unwrap_or(0);
                if let Some(s) = self.debug.session.as_mut() {
                    s.exit = Some(code);
                    s.stopped = None;
                    s.frames.clear();
                    s.locals.clear();
                }
                self.debug_redraw();
            }
            "terminated" => {
                let code = self.debug.session.as_ref().and_then(|s| s.exit);
                let _ = self.debug_send(
                    "disconnect",
                    json!({"terminateDebuggee": true}),
                    Request::Disconnect,
                );
                self.debug_end(&match code {
                    Some(code) => format!("Program exited with code {code}"),
                    None => "Debugging stopped".into(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn send_breakpoints(&mut self, file: &Path) -> Result<()> {
        let lines: Vec<Value> = self
            .debug
            .breakpoints
            .get(file)
            .into_iter()
            .flatten()
            .map(|line| json!({"line": line + 1}))
            .collect();
        self.debug_send(
            "setBreakpoints",
            json!({"source": {"path": file}, "breakpoints": lines}),
            Request::Breakpoints,
        )
    }

    /// Toggle a breakpoint on the caret's line.
    pub(crate) fn toggle_breakpoint(&mut self) -> Result<()> {
        let id = self
            .active_editor()
            .ok_or_else(|| anyhow::anyhow!("Focus an editor first"))?;
        let v = &self.views[&id];
        let d = &self.documents[&v.document];
        let Some(path) = d.path.clone() else {
            bail!("Save the document before setting breakpoints");
        };
        let line = d.line_of(v.cursor);
        let set = self.debug.breakpoints.entry(path.clone()).or_default();
        let added = set.insert(line);
        if !added {
            set.remove(&line);
        }
        if set.is_empty() {
            self.debug.breakpoints.remove(&path);
        }
        self.status = format!(
            "{} breakpoint at line {}",
            if added { "Set" } else { "Removed" },
            line + 1
        );
        if self.debug.session.is_some() {
            self.send_breakpoints(&path)?;
        }
        self.revision += 1;
        Ok(())
    }

    /// continue, next, stepIn, stepOut, pause or stop.
    pub(crate) fn debug_control(&mut self, command: &str) -> Result<()> {
        let Some(s) = self.debug.session.as_ref() else {
            bail!("Start debugging first (debug-start)");
        };
        if command == "stop" {
            let _ = self.debug_send(
                "disconnect",
                json!({"terminateDebuggee": true}),
                Request::Disconnect,
            );
            self.debug_end("Debugging stopped");
            return Ok(());
        }
        let thread = s.stopped.as_ref().map(|(t, _)| *t);
        if command == "pause" {
            if thread.is_some() {
                bail!("Already paused");
            }
            return self.debug_send("pause", json!({"threadId": 1}), Request::Control);
        }
        let Some(thread) = thread else {
            bail!("The program is running; pause it first");
        };
        if let Some(s) = self.debug.session.as_mut() {
            s.stopped = None;
        }
        self.debug_send(command, json!({"threadId": thread}), Request::Control)?;
        self.status = "Running…".into();
        Ok(())
    }

    pub(crate) fn debug_evaluate(&mut self, expression: &str) -> Result<()> {
        let frame = self
            .debug
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Start debugging first"))?
            .frames
            .first()
            .map(|f| f.id);
        self.debug_send(
            "evaluate",
            // "watch" evaluates an expression; "repl" would run debugger
            // commands (GDB treats `r` as `run`).
            json!({"expression": expression, "frameId": frame, "context": "watch"}),
            Request::Evaluate(expression.to_string()),
        )
    }

    /// Breakpoint lines of a document, and the line the debugger is paused
    /// at in it (zero-based).
    pub(crate) fn debug_marks(&self, doc: u64) -> (BTreeSet<usize>, Option<usize>) {
        let Some(path) = self.documents.get(&doc).and_then(|d| d.path.as_ref()) else {
            return Default::default();
        };
        let breakpoints = self
            .debug
            .breakpoints
            .get(path)
            .cloned()
            .unwrap_or_default();
        let current = self
            .debug
            .session
            .as_ref()
            .filter(|s| s.stopped.is_some())
            .and_then(|s| s.frames.first())
            .filter(|f| f.path.as_deref() == Some(path.as_path()) && f.line > 0)
            .map(|f| f.line - 1);
        (breakpoints, current)
    }
}
