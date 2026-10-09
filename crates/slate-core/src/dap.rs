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
//! marks breakpoints and the paused line. Breakpoints follow edits and are
//! remembered with the workspace.
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
    time::{Duration, Instant},
};

const CONSOLE_LIMIT: usize = 256 * 1024;
/// The panel is redrawn at most this often while a program prints.
const REDRAW_INTERVAL: Duration = Duration::from_millis(100);

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

/// Launch configurations of a workspace, or the reason they could not be
/// read.
pub fn launches(root: &Path) -> Result<Vec<Launch>, String> {
    Ok(
        crate::config::read::<File>(&root.join(".slate/launch.toml"))?
            .map(|f| f.launch)
            .unwrap_or_default(),
    )
}

#[derive(Clone, Debug)]
struct Frame {
    id: i64,
    name: String,
    path: Option<PathBuf>,
    line: usize,
}

/// Requests in flight. Those about a pause carry the pause they belong to,
/// so a late answer after the program moved on is dropped.
#[derive(Debug)]
enum Request {
    Initialize,
    Launch,
    Breakpoints,
    ConfigurationDone,
    StackTrace(u64),
    Scopes(u64),
    Variables(u64),
    Evaluate(String),
    Threads,
    /// A step or continue, with the pause it left (restored on failure).
    Control(Option<(i64, String)>),
    Disconnect,
}

struct Session {
    id: u64,
    process: crate::rpc::Process,
    seq: i64,
    pending: HashMap<i64, Request>,
    deadlines: HashMap<i64, Instant>,
    launch: Launch,
    stopped: Option<(i64, String)>,
    /// Counts pauses and resumes.
    pause: u64,
    /// The last thread seen, for pausing.
    thread: Option<i64>,
    frames: Vec<Frame>,
    /// The frame whose locals are shown and in which expressions evaluate.
    frame: usize,
    locals: Vec<(String, String)>,
    console: String,
    exit: Option<i64>,
    panel: u64,
    dirty: bool,
    drawn: Instant,
}

#[derive(Default)]
pub(crate) struct Debug {
    session: Option<Session>,
    next: u64,
    /// Breakpoint lines (zero-based) by file.
    pub breakpoints: BTreeMap<PathBuf, BTreeSet<usize>>,
    /// For open documents, the byte offsets of breakpoint lines, so they
    /// move with edits.
    anchors: BTreeMap<u64, Vec<usize>>,
    /// The launch configuration used last.
    last: Option<String>,
    /// The session that ended and its panel: output still in flight when it
    /// ended is added to the panel.
    ended: Option<(u64, u64)>,
}

impl App {
    pub fn debugging(&self) -> bool {
        self.debug.session.is_some()
    }

    pub fn debug_paused(&self) -> bool {
        self.debug
            .session
            .as_ref()
            .is_some_and(|s| s.stopped.is_some())
    }

    fn debug_send(&mut self, command: &str, arguments: Value, request: Request) -> Result<()> {
        if self
            .debug
            .session
            .as_ref()
            .is_some_and(|s| s.pending.len() >= 1024)
        {
            self.debug_end("Debug adapter overloaded");
            bail!("Debug adapter overloaded");
        }
        let Some(s) = self.debug.session.as_mut() else {
            bail!("No debug session");
        };
        s.seq += 1;
        let seq = s.seq;
        s.pending.insert(seq, request);
        s.deadlines.insert(seq, Instant::now());
        s.process.send(&json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        }));
        Ok(())
    }

    /// Start debugging: a named launch configuration, a program path, the
    /// configuration used last, or the only one there is. While debugging,
    /// continue.
    pub(crate) fn debug_start(&mut self, argument: &str) -> Result<()> {
        if self.debug.session.is_some() {
            return self.debug_control("continue");
        }
        let configured = launches(&self.root).map_err(anyhow::Error::msg)?;
        let remembered = self
            .debug
            .last
            .as_ref()
            .and_then(|name| configured.iter().find(|l| &l.name == name));
        let launch = match configured.iter().find(|l| l.name == argument) {
            Some(l) => l.clone(),
            None if !argument.is_empty() => Launch {
                name: argument.to_string(),
                program: argument.to_string(),
                ..Default::default()
            },
            None if remembered.is_some() => remembered.unwrap().clone(),
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
        let program = self.root.join(&launch.program);
        let cwd = launch
            .cwd
            .as_ref()
            .map(|cwd| self.root.join(cwd))
            .unwrap_or_else(|| self.root.clone());
        let program_folder = program.parent().unwrap_or(&self.root).to_path_buf();
        for folder in [program_folder, cwd] {
            if !self.trusted_path(&folder) {
                self.ask_trust_for(
                    &folder,
                    "Debugging runs programs from this folder",
                    crate::Command::Action {
                        name: "debug-start".into(),
                        argument: launch.name.clone(),
                    },
                );
                return Ok(());
            }
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
        self.debug.last = Some(launch.name.clone());
        let panel = self.debug_panel(&launch.name);
        self.debug.session = Some(Session {
            id,
            process,
            seq: 0,
            pending: HashMap::new(),
            deadlines: HashMap::new(),
            launch,
            stopped: None,
            pause: 0,
            thread: None,
            frames: Vec::new(),
            frame: 0,
            locals: Vec::new(),
            console: String::new(),
            exit: None,
            panel,
            dirty: false,
            drawn: Instant::now(),
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

    /// The Debug document (reused from an earlier session), shown next to
    /// the terminals.
    fn debug_panel(&mut self, name: &str) -> u64 {
        let label = format!("Debug: {name}");
        let existing = self
            .documents
            .iter()
            .find(|(_, d)| d.label.as_deref().is_some_and(|l| l.starts_with("Debug: ")))
            .map(|(id, _)| *id);
        let doc = match existing {
            Some(doc) => {
                let d = self.documents.get_mut(&doc).unwrap();
                let len = d.len();
                d.replace_inspection(0, len, "Starting…\n");
                d.label = Some(label);
                d.generation += 1;
                self.clamp_views(doc);
                doc
            }
            None => {
                let doc = self.id();
                self.documents.insert(
                    doc,
                    crate::document::Document::inspection(label, "Starting…\n".into()),
                );
                doc
            }
        };
        self.show_output(doc);
        doc
    }

    /// Redraw the panel when output arrived and it has not been drawn
    /// recently.
    pub(crate) fn debug_flush(&mut self) {
        let expired: Vec<_> = self
            .debug
            .session
            .as_ref()
            .map(|s| {
                s.deadlines
                    .iter()
                    .filter(|(_, at)| at.elapsed() >= std::time::Duration::from_secs(30))
                    .map(|(seq, _)| *seq)
                    .collect()
            })
            .unwrap_or_default();
        for seq in expired {
            if let Some(s) = self.debug.session.as_mut() {
                s.deadlines.remove(&seq);
                if let Some(request) = s.pending.remove(&seq) {
                    let handshake = matches!(
                        request,
                        Request::Initialize | Request::Launch | Request::ConfigurationDone
                    );
                    if handshake {
                        self.debug_end("Debug adapter request timed out");
                    } else if let Err(error) =
                        self.debug_failed(request, "Debug adapter request timed out")
                    {
                        self.status = format!("{error:#}");
                    }
                }
            }
        }

        if self
            .debug
            .session
            .as_ref()
            .is_some_and(|s| s.dirty && s.drawn.elapsed() >= REDRAW_INTERVAL)
        {
            self.debug_redraw();
        }
    }

    fn debug_redraw(&mut self) {
        let Some(s) = self.debug.session.as_mut() else {
            return;
        };
        s.dirty = false;
        s.drawn = Instant::now();
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
            text.push_str("\nCall stack (debug-call-stack chooses a frame)\n");
            for (i, f) in s.frames.iter().enumerate() {
                text.push_str(&format!(
                    "{} #{i} {} {}:{}\n",
                    if i == s.frame { "▶" } else { " " },
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
            text.push_str(&format!("\nLocals of #{}\n", s.frame));
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
                } else if let Some((_, panel)) = self.debug.ended.filter(|(id, _)| *id == session) {
                    if message["type"] == "event"
                        && message["event"] == "output"
                        && message["body"]["category"] != "telemetry"
                    {
                        let text = message["body"]["output"].as_str().unwrap_or_default();
                        if let Some(d) = self.documents.get_mut(&panel) {
                            d.append_inspection(text);
                        }
                    }
                }
            }
            Event::DapExited { session, reason }
                if self.debug.session.as_ref().is_some_and(|s| s.id == session) =>
            {
                self.debug_end(&format!("Debugger stopped ({reason})"));
            }
            _ => {}
        }
    }

    fn debug_end(&mut self, status: &str) {
        self.debug_redraw();
        if let Some(mut s) = self.debug.session.take() {
            self.debug.ended = Some((s.id, s.panel));
            s.process.stop(Duration::from_millis(200));
        }
        self.status = status.to_string();
        self.revision += 1;
    }

    fn debug_message(&mut self, message: Value) -> Result<()> {
        match message["type"].as_str() {
            Some("response") => {
                let seq = message["request_seq"].as_i64().unwrap_or(0);
                if let Some(s) = self.debug.session.as_mut() {
                    s.deadlines.remove(&seq);
                }
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
                    return self.debug_failed(request, &text);
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

    fn debug_failed(&mut self, request: Request, text: &str) -> Result<()> {
        match request {
            Request::Evaluate(expression) => {
                self.status = format!("{expression}: {text}");
                self.debug_console(&format!("> {expression}\n  {text}\n"));
                Ok(())
            }
            Request::Launch => {
                self.debug_end(&format!("Could not start: {text}"));
                Ok(())
            }
            // The program did not move: it is still paused where it was.
            Request::Control(previous) => {
                if let Some(s) = self.debug.session.as_mut() {
                    s.stopped = previous;
                }
                self.debug_redraw();
                bail!("{text}")
            }
            _ => bail!("{text}"),
        }
    }

    /// Whether an answer about pause `pause` is still current.
    fn current_pause(&self, pause: u64) -> bool {
        self.debug
            .session
            .as_ref()
            .is_some_and(|s| s.pause == pause && s.stopped.is_some())
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
            Request::StackTrace(pause) => {
                if !self.current_pause(pause) {
                    return Ok(());
                }
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
                if let Some(s) = self.debug.session.as_mut() {
                    s.frames = frames;
                    s.frame = 0;
                    s.locals.clear();
                }
                self.select_frame(0)?;
            }
            Request::Scopes(pause) => {
                if !self.current_pause(pause) {
                    return Ok(());
                }
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
                        Request::Variables(pause),
                    )?;
                }
            }
            Request::Variables(pause) => {
                if !self.current_pause(pause) {
                    return Ok(());
                }
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
            Request::Threads => {
                let thread = body["threads"]
                    .as_array()
                    .and_then(|t| t.first())
                    .and_then(|t| t["id"].as_i64());
                if let Some(thread) = thread {
                    if let Some(s) = self.debug.session.as_mut() {
                        s.thread = Some(thread);
                    }
                    self.debug_send("pause", json!({"threadId": thread}), Request::Control(None))?;
                }
            }
            Request::Launch
            | Request::Breakpoints
            | Request::ConfigurationDone
            | Request::Control(_)
            | Request::Disconnect => {}
        }
        Ok(())
    }

    /// Show frame `index`: move the editor there and list its locals.
    pub(crate) fn select_frame(&mut self, index: usize) -> Result<()> {
        let Some(s) = self.debug.session.as_mut() else {
            bail!("Start debugging first");
        };
        let Some(frame) = s.frames.get(index).cloned() else {
            bail!("No such frame");
        };
        s.frame = index;
        s.locals.clear();
        let pause = s.pause;
        self.debug_send(
            "scopes",
            json!({"frameId": frame.id}),
            Request::Scopes(pause),
        )?;
        if let (Some(path), true) = (frame.path.clone(), frame.line > 0) {
            if path.exists() {
                self.goto(Location {
                    path,
                    line: frame.line - 1,
                    column: Column::Byte(0),
                })?;
            }
        }
        self.debug_redraw();
        Ok(())
    }

    /// A picker of the paused call stack.
    pub(crate) fn frame_picker(&mut self) -> Result<()> {
        let frames = match self.debug.session.as_ref() {
            Some(s) if s.stopped.is_some() && !s.frames.is_empty() => s.frames.clone(),
            Some(_) => bail!("The program is not paused"),
            None => bail!("Start debugging first"),
        };
        let mut picker = crate::picker::Picker::new("frames", "Call stack", true);
        picker.entries = frames
            .iter()
            .enumerate()
            .map(|(i, f)| crate::picker::Entry {
                label: format!("#{i} {}", f.name),
                detail: format!(
                    "{}:{}",
                    f.path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "?".into()),
                    f.line
                ),
                kind: "frame".into(),
                target: crate::picker::Target::Command(crate::Command::Action {
                    name: "debug-frame".into(),
                    argument: i.to_string(),
                }),
            })
            .collect();
        picker.filter();
        self.picker = Some(picker);
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
            s.dirty = true;
        }
        self.debug_flush();
    }

    /// The program runs again: forget the pause it left.
    fn resumed(&mut self) {
        if let Some(s) = self.debug.session.as_mut() {
            s.stopped = None;
            s.pause += 1;
            s.frames.clear();
            s.frame = 0;
            s.locals.clear();
        }
        self.debug_redraw();
    }

    fn debug_notification(&mut self, event: &str, body: &Value) -> Result<()> {
        match event {
            "initialized" => {
                self.sync_breakpoint_lines();
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
                let pause = match self.debug.session.as_mut() {
                    Some(s) => {
                        s.pause += 1;
                        s.stopped = Some((thread, reason.clone()));
                        s.thread = Some(thread);
                        s.pause
                    }
                    None => return Ok(()),
                };
                self.status = format!("Paused: {reason}");
                self.debug_send(
                    "stackTrace",
                    json!({"threadId": thread, "startFrame": 0, "levels": 32}),
                    Request::StackTrace(pause),
                )?;
            }
            "continued" => self.resumed(),
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
                }
                self.resumed();
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

    /// Byte offsets of a document's breakpoint lines (made from the stored
    /// lines the first time they are needed).
    fn breakpoint_anchors(&mut self, doc: u64) -> Vec<usize> {
        if let Some(anchors) = self.debug.anchors.get(&doc) {
            return anchors.clone();
        }
        let Some(d) = self.documents.get(&doc) else {
            return Vec::new();
        };
        let anchors: Vec<usize> = d
            .path
            .as_ref()
            .and_then(|p| self.debug.breakpoints.get(p))
            .into_iter()
            .flatten()
            .filter(|line| **line < d.line_count())
            .map(|line| d.line_offset(*line))
            .collect();
        if !anchors.is_empty() {
            self.debug.anchors.insert(doc, anchors.clone());
        }
        anchors
    }

    /// Move a document's breakpoints with an edit (`position` maps old
    /// offsets to new ones).
    pub(crate) fn rebase_breakpoints(&mut self, doc: u64, position: impl Fn(usize) -> usize) {
        let Some(anchors) = self.debug.anchors.get_mut(&doc) else {
            return;
        };
        for a in anchors.iter_mut() {
            *a = position(*a);
        }
        self.store_breakpoint_lines(doc);
    }

    /// The stored lines of a document follow its anchors.
    fn store_breakpoint_lines(&mut self, doc: u64) {
        let (Some(d), Some(anchors)) = (self.documents.get(&doc), self.debug.anchors.get(&doc))
        else {
            return;
        };
        let Some(path) = d.path.clone() else {
            return;
        };
        let lines: BTreeSet<usize> = anchors
            .iter()
            .map(|a| d.line_of((*a).min(d.len())))
            .collect();
        let anchors: Vec<usize> = lines.iter().map(|l| d.line_offset(*l)).collect();
        self.debug.anchors.insert(doc, anchors);
        if lines.is_empty() {
            self.debug.breakpoints.remove(&path);
        } else {
            self.debug.breakpoints.insert(path, lines);
        }
    }

    /// Bring stored lines up to date for every open document and forget
    /// anchors of closed or reloaded ones.
    fn sync_breakpoint_lines(&mut self) {
        let docs: Vec<u64> = self.debug.anchors.keys().copied().collect();
        for doc in docs {
            if self.documents.contains_key(&doc) {
                self.store_breakpoint_lines(doc);
            } else {
                self.debug.anchors.remove(&doc);
            }
        }
    }

    /// A document's text was replaced (reload): its breakpoints are placed
    /// again from their lines.
    pub(crate) fn reset_breakpoints(&mut self, doc: u64) {
        self.debug.anchors.remove(&doc);
    }

    /// Toggle a breakpoint on the caret's line.
    pub(crate) fn toggle_breakpoint(&mut self) -> Result<()> {
        let id = self
            .active_editor()
            .ok_or_else(|| anyhow::anyhow!("Focus an editor first"))?;
        let doc = self.views[&id].document;
        let cursor = self.views[&id].cursor;
        let Some(path) = self.documents[&doc].path.clone() else {
            bail!("Save the document before setting breakpoints");
        };
        let line = self.documents[&doc].line_of(cursor);
        let mut anchors = self.breakpoint_anchors(doc);
        let start = self.documents[&doc].line_offset(line);
        let added = !anchors.contains(&start);
        if added {
            anchors.push(start);
        } else {
            anchors.retain(|a| *a != start);
        }
        self.debug.anchors.insert(doc, anchors);
        self.store_breakpoint_lines(doc);
        self.status = format!(
            "{} breakpoint at line {}",
            if added { "Set" } else { "Removed" },
            line + 1
        );
        if self.debug.session.is_some() {
            self.send_breakpoints(&path)?;
        }
        self.recovery.dirty = true;
        self.revision += 1;
        Ok(())
    }

    /// Remove every breakpoint.
    pub(crate) fn clear_breakpoints(&mut self) -> Result<()> {
        let files: Vec<PathBuf> = self.debug.breakpoints.keys().cloned().collect();
        self.debug.breakpoints.clear();
        self.debug.anchors.clear();
        if self.debug.session.is_some() {
            for file in files {
                self.send_breakpoints(&file)?;
            }
        }
        self.status = "Removed all breakpoints".into();
        self.recovery.dirty = true;
        self.revision += 1;
        Ok(())
    }

    /// A picker of every breakpoint.
    pub(crate) fn breakpoint_picker(&mut self) -> Result<()> {
        self.sync_breakpoint_lines();
        if self.debug.breakpoints.is_empty() {
            bail!("No breakpoints (toggle-breakpoint sets one)");
        }
        let root = self.root.clone();
        let mut picker = crate::picker::Picker::new("breakpoints", "Breakpoints", true);
        picker.entries = self
            .debug
            .breakpoints
            .iter()
            .flat_map(|(path, lines)| {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned();
                lines.iter().map(move |line| crate::picker::Entry {
                    label: format!("{relative}:{}", line + 1),
                    detail: String::new(),
                    kind: "breakpoint".into(),
                    target: crate::picker::Target::Location(Location {
                        path: path.clone(),
                        line: *line,
                        column: Column::Byte(0),
                    }),
                })
            })
            .collect();
        picker.filter();
        self.picker = Some(picker);
        Ok(())
    }

    /// Breakpoints for the workspace checkpoint, with open documents'
    /// breakpoints at their current lines.
    pub(crate) fn saved_breakpoints(&self) -> BTreeMap<PathBuf, Vec<usize>> {
        let mut all: BTreeMap<PathBuf, Vec<usize>> = self
            .debug
            .breakpoints
            .iter()
            .map(|(p, l)| (p.clone(), l.iter().copied().collect()))
            .collect();
        for (doc, anchors) in &self.debug.anchors {
            if let Some(d) = self.documents.get(doc) {
                if let Some(path) = &d.path {
                    let lines: BTreeSet<usize> = anchors
                        .iter()
                        .map(|a| d.line_of((*a).min(d.len())))
                        .collect();
                    all.insert(path.clone(), lines.into_iter().collect());
                }
            }
        }
        all.retain(|_, l| !l.is_empty());
        all
    }

    pub(crate) fn restore_breakpoints(&mut self, saved: BTreeMap<PathBuf, Vec<usize>>) {
        self.debug.anchors.clear();
        self.debug.breakpoints = saved
            .into_iter()
            .filter(|(_, lines)| !lines.is_empty())
            .take(1000)
            .map(|(p, l)| (p, l.into_iter().take(1000).collect()))
            .collect();
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
        let stopped = s.stopped.clone();
        if command == "pause" {
            if stopped.is_some() {
                bail!("Already paused");
            }
            // The thread last seen, or the first one the adapter lists.
            return match s.thread {
                Some(thread) => {
                    self.debug_send("pause", json!({"threadId": thread}), Request::Control(None))
                }
                None => self.debug_send("threads", json!({}), Request::Threads),
            };
        }
        let Some((thread, _)) = stopped.clone() else {
            bail!("The program is running; pause it first");
        };
        // Adapters need not report "continued" for requested steps.
        self.resumed();
        self.debug_send(
            command,
            json!({"threadId": thread}),
            Request::Control(stopped),
        )?;
        self.status = "Running…".into();
        Ok(())
    }

    pub(crate) fn debug_evaluate(&mut self, expression: &str) -> Result<()> {
        let session = self
            .debug
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Start debugging first"))?;
        let frame = session.frames.get(session.frame).map(|f| f.id);
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
        let Some(d) = self.documents.get(&doc) else {
            return Default::default();
        };
        let Some(path) = d.path.as_ref() else {
            return Default::default();
        };
        let breakpoints = match self.debug.anchors.get(&doc) {
            Some(anchors) => anchors
                .iter()
                .map(|a| d.line_of((*a).min(d.len())))
                .collect(),
            None => self
                .debug
                .breakpoints
                .get(path)
                .cloned()
                .unwrap_or_default(),
        };
        let current = self
            .debug
            .session
            .as_ref()
            .filter(|s| s.stopped.is_some())
            .and_then(|s| s.frames.get(s.frame))
            .filter(|f| f.path.as_deref() == Some(path.as_path()) && f.line > 0)
            .map(|f| f.line - 1);
        (breakpoints, current)
    }
}

#[cfg(test)]
mod timeout_tests {
    #[test]
    fn an_unresponsive_adapter_cannot_leave_initialization_pending_forever() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        std::fs::create_dir_all(dir.path().join(".slate")).unwrap();
        std::fs::write(dir.path().join(".slate/launch.toml"), "[[launch]]\nname = 'stuck'\nprogram = 'fixture'\nadapter = ['python3', '-c', 'import time; time.sleep(60)']\n").unwrap();
        let mut app = crate::App::new(dir.path()).unwrap();
        app.set_session_trust(true);
        app.debug_start("stuck").unwrap();
        let session = app.debug.session.as_mut().unwrap();
        for started in session.deadlines.values_mut() {
            *started = std::time::Instant::now() - std::time::Duration::from_secs(31);
        }
        app.debug_flush();
        assert!(!app.debugging());
        assert!(app.status.contains("timed out"));
    }
}
