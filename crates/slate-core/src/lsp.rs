//! Language Server Protocol client.
//!
//! Servers start on demand for documents whose language has one installed
//! (see `languages.rs`), only in trusted workspaces. Documents sync
//! incrementally from the edit feed the document records; requests are
//! asynchronous and their answers arrive as `ide::Event`s on the main loop.
use crate::{
    ide::Event,
    languages::Language,
    navigation::{Column, Location},
    picker::{Entry, Picker, Target},
    services::Reply,
    App, Command,
};
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

/// Completion items kept per request.
const COMPLETION_LIMIT: usize = 2_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// Zero-based (line, UTF-16 column) start and end.
    pub start: (usize, usize),
    pub end: (usize, usize),
    /// 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u8,
    pub message: String,
    pub source: String,
    #[serde(skip)]
    pub raw: Value,
}

struct Server {
    language: String,
    root: PathBuf,
    process: crate::rpc::Process,
    capabilities: Value,
    ready: bool,
    /// Messages held until the server has initialized.
    queue: Vec<Value>,
}

struct OpenDoc {
    server: u64,
    uri: String,
    path: PathBuf,
    version: i64,
    content: u64,
}

#[derive(Debug)]
enum Pending {
    Initialize,
    Hover {
        doc: u64,
        offset: usize,
    },
    Definition,
    References,
    Completion {
        view: u64,
        doc: u64,
        offset: usize,
    },
    Rename,
    Formatting {
        doc: u64,
        content: u64,
        then_save: bool,
    },
    DocumentSymbols {
        doc: u64,
    },
    WorkspaceSymbols {
        generation: u64,
    },
    CodeActions,
    Ignore,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CompletionView {
    pub pane: u64,
    /// Screen cell of the completed word's start, within the pane's text.
    pub row: usize,
    pub col: usize,
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct HoverView {
    pub pane: u64,
    pub row: usize,
    pub col: usize,
    pub text: String,
}

struct Candidate {
    item: CompletionItem,
    filter: String,
    insert: String,
    snippet: bool,
    /// A replacement range from the server, as byte offsets.
    range: Option<(usize, usize)>,
    additional: Vec<(usize, usize, String)>,
}

pub(crate) struct Completion {
    view: u64,
    doc: u64,
    /// Where the completed word starts.
    start: usize,
    candidates: Vec<Candidate>,
    shown: Vec<usize>,
    selected: usize,
    incomplete: bool,
}

struct Hover {
    doc: u64,
    offset: usize,
    text: String,
}

#[derive(Default)]
pub(crate) struct Lsp {
    servers: BTreeMap<u64, Server>,
    keys: HashMap<(String, PathBuf), u64>,
    failed: HashSet<(String, PathBuf)>,
    docs: BTreeMap<u64, OpenDoc>,
    languages: Option<Vec<Language>>,
    pending: HashMap<(u64, i64), Pending>,
    next_id: i64,
    next_server: u64,
    /// Diagnostics by file, from language servers and task output.
    pub diagnostics: BTreeMap<PathBuf, Vec<Diagnostic>>,
    pub revision: u64,
    hover: Option<Hover>,
    completion: Option<Completion>,
    progress: BTreeMap<String, String>,
    /// Messages a server showed the user.
    pub messages: Vec<String>,
}

// ----- URIs and positions -----

pub fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

fn position(value: &Value) -> (usize, usize) {
    (
        value["line"].as_u64().unwrap_or(0) as usize,
        value["character"].as_u64().unwrap_or(0) as usize,
    )
}

fn json_position((line, character): (usize, usize)) -> Value {
    json!({"line": line, "character": character})
}

fn severity_name(severity: u8) -> &'static str {
    match severity {
        1 => "error",
        2 => "warning",
        3 => "info",
        _ => "hint",
    }
}

fn completion_kind(kind: u64) -> &'static str {
    match kind {
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "module",
        10 => "property",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        21 => "constant",
        22 => "struct",
        _ => "text",
    }
}

fn symbol_kind(kind: u64) -> &'static str {
    match kind {
        2 => "module",
        3 => "namespace",
        5 => "class",
        6 => "method",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        22 => "enum member",
        23 => "struct",
        _ => "symbol",
    }
}

/// Hover contents as plain text.
fn hover_text(contents: &Value) -> String {
    match contents {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(hover_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(o) => o
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
    .lines()
    .filter(|l| !l.trim_start().starts_with("```"))
    .collect::<Vec<_>>()
    .join("\n")
    .trim()
    .to_string()
}

/// Move diagnostic positions across one edit (LSP line/UTF-16 positions).
fn shift_diagnostics(list: &mut [Diagnostic], change: &crate::document::TextChange) {
    let start = change.start_position;
    let old_end = change.old_end_position;
    let inserted_lines = change.text.matches('\n').count();
    let last_line_units: usize = change
        .text
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .encode_utf16()
        .count();
    let new_end = if inserted_lines == 0 {
        (start.0, start.1 + last_line_units)
    } else {
        (start.0 + inserted_lines, last_line_units)
    };
    let shift = |p: (usize, usize)| -> (usize, usize) {
        if p <= start {
            p
        } else if p < old_end {
            new_end
        } else if p.0 == old_end.0 {
            (new_end.0, new_end.1 + (p.1 - old_end.1))
        } else {
            // A later line: `p.0 > old_end.0`, so this cannot underflow.
            (p.0 + new_end.0 - old_end.0, p.1)
        }
    };
    for d in list.iter_mut() {
        d.start = shift(d.start);
        d.end = shift(d.end);
    }
}

impl App {
    fn languages(&mut self) -> &[Language] {
        if self.lsp.languages.is_none() {
            self.lsp.languages = Some(crate::languages::all());
        }
        self.lsp.languages.as_deref().unwrap()
    }

    /// The language configuration for a document.
    pub(crate) fn language_for(&mut self, doc: u64) -> Option<Language> {
        let d = self.documents.get(&doc)?;
        if d.label.is_some() {
            return None;
        }
        let path = d.path.clone()?;
        let syntax = crate::highlight::syntax_for(Some(&path), &d.line_text(0))
            .name
            .clone();
        let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
        let languages = self.languages();
        languages
            .iter()
            .find(|l| extension.as_ref().is_some_and(|e| l.extensions.contains(e)))
            .or_else(|| languages.iter().find(|l| l.name == syntax))
            .cloned()
    }

    fn server_for(&mut self, doc: u64) -> Option<u64> {
        self.lsp.docs.get(&doc).map(|d| d.server)
    }

    fn capability(&self, server: u64, name: &str) -> bool {
        self.lsp.servers.get(&server).is_some_and(|s| {
            let value = &s.capabilities[name];
            !value.is_null() && value != &Value::Bool(false)
        })
    }

    fn start_server(&mut self, language: &Language, root: PathBuf) -> Option<u64> {
        let key = (language.name.clone(), root.clone());
        if let Some(id) = self.lsp.keys.get(&key) {
            return Some(*id);
        }
        if self.lsp.failed.contains(&key) {
            return None;
        }
        let Some(command) = language.server_command() else {
            self.lsp.failed.insert(key);
            return None;
        };
        self.lsp.next_server += 1;
        let id = self.lsp.next_server;
        let output = self.services.reply_sender();
        let exit = self.services.reply_sender();
        let process = match crate::rpc::Process::spawn(
            &command,
            &root,
            move |message| {
                output
                    .send(Reply::Ide(Event::Lsp {
                        server: id,
                        message,
                    }))
                    .is_ok()
            },
            move |reason| {
                let _ = exit.send(Reply::Ide(Event::LspExited { server: id, reason }));
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                self.status = format!("{}: {e}", command[0]);
                self.lsp.failed.insert(key);
                return None;
            }
        };
        let root_uri = uri(&root);
        let request_id = self.next_request();
        process.send(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "clientInfo": {"name": "Slate", "version": env!("CARGO_PKG_VERSION")},
                "rootUri": root_uri,
                "rootPath": root.to_string_lossy(),
                "workspaceFolders": [{"uri": root_uri, "name": root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()}],
                "capabilities": {
                    "general": {"positionEncodings": ["utf-16"]},
                    "window": {"workDoneProgress": true, "showMessage": {}},
                    "workspace": {
                        "applyEdit": true,
                        "workspaceEdit": {"documentChanges": true},
                        "configuration": true,
                        "workspaceFolders": true,
                        "symbol": {}
                    },
                    "textDocument": {
                        "synchronization": {"didSave": true, "dynamicRegistration": false},
                        "hover": {"contentFormat": ["plaintext", "markdown"]},
                        "completion": {
                            "completionItem": {"snippetSupport": true, "documentationFormat": ["plaintext"]},
                            "contextSupport": true
                        },
                        "definition": {"linkSupport": true},
                        "references": {},
                        "rename": {"prepareSupport": false},
                        "formatting": {},
                        "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                        "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"]}}},
                        "publishDiagnostics": {"relatedInformation": false}
                    }
                }
            }
        }));
        self.lsp
            .pending
            .insert((id, request_id), Pending::Initialize);
        self.lsp.servers.insert(
            id,
            Server {
                language: language.name.clone(),
                root,
                process,
                capabilities: Value::Null,
                ready: false,
                queue: Vec::new(),
            },
        );
        self.lsp.keys.insert(key, id);
        self.status = format!("Starting {}…", command[0]);
        Some(id)
    }

    fn next_request(&mut self) -> i64 {
        self.lsp.next_id += 1;
        self.lsp.next_id
    }

    fn send_to(&mut self, server: u64, message: Value) {
        if let Some(s) = self.lsp.servers.get_mut(&server) {
            if s.ready {
                s.process.send(&message);
            } else {
                s.queue.push(message);
            }
        }
    }

    fn notify(&mut self, server: u64, method: &str, params: Value) {
        self.send_to(
            server,
            json!({"jsonrpc": "2.0", "method": method, "params": params}),
        );
    }

    fn request(&mut self, server: u64, method: &str, params: Value, pending: Pending) {
        let id = self.next_request();
        self.lsp.pending.insert((server, id), pending);
        self.send_to(
            server,
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        );
    }

    /// Keep servers and their documents in step with the editor. Runs on
    /// every pass of the event loop; it does nothing when nothing changed.
    pub(crate) fn lsp_sync(&mut self) {
        if !self.trusted {
            if !self.lsp.servers.is_empty() {
                self.lsp_stop_all();
            }
            return;
        }
        // Documents that closed or changed path leave their server.
        let gone: Vec<u64> = self
            .lsp
            .docs
            .iter()
            .filter(|(doc, open)| {
                self.documents
                    .get(doc)
                    .is_none_or(|d| d.path.as_ref() != Some(&open.path))
            })
            .map(|(doc, _)| *doc)
            .collect();
        for doc in gone {
            let open = self.lsp.docs.remove(&doc).unwrap();
            self.notify(
                open.server,
                "textDocument/didClose",
                json!({"textDocument": {"uri": open.uri}}),
            );
        }
        let ids: Vec<u64> = self.documents.keys().copied().collect();
        for doc in ids {
            if self.lsp.docs.contains_key(&doc) {
                self.lsp_sync_changes(doc);
                continue;
            }
            let Some(language) = self.language_for(doc) else {
                continue;
            };
            let path = self.documents[&doc].path.clone().unwrap();
            let root = language.root_for(&path, &self.root);
            let Some(server) = self.start_server(&language, root) else {
                continue;
            };
            let d = self.documents.get_mut(&doc).unwrap();
            // Edits made before opening are part of the full text sent now.
            let _ = d.take_changes();
            let text = d.text();
            let content = d.content_version();
            let uri = uri(&path);
            self.notify(
                server,
                "textDocument/didOpen",
                json!({"textDocument": {"uri": uri, "languageId": language.id(), "version": 1, "text": text}}),
            );
            self.lsp.docs.insert(
                doc,
                OpenDoc {
                    server,
                    uri,
                    path,
                    version: 1,
                    content,
                },
            );
        }
    }

    fn lsp_sync_changes(&mut self, doc: u64) {
        let d = self.documents.get_mut(&doc).unwrap();
        let open = self.lsp.docs.get_mut(&doc).unwrap();
        let changes = d.take_changes();
        let content = d.content_version();
        let full = match &changes {
            Some(list) if !list.is_empty() => false,
            Some(_) if content == open.content => return,
            _ => true,
        };
        open.version += 1;
        open.content = content;
        let (server, uri, version) = (open.server, open.uri.clone(), open.version);
        let incremental = self.lsp.servers.get(&server).is_some_and(|s| {
            let sync = &s.capabilities["textDocumentSync"];
            sync.as_u64().or_else(|| sync["change"].as_u64()) == Some(2)
        });
        // Diagnostics move with the text until the server reports again.
        if let (Some(list), false) = (&changes, full) {
            let path = self.documents[&doc].path.clone();
            if let Some(diagnostics) = path.and_then(|p| self.lsp.diagnostics.get_mut(&p)) {
                for change in list {
                    shift_diagnostics(diagnostics, change);
                }
                self.lsp.revision += 1;
            }
        }
        let content_changes: Vec<Value> = if full || !incremental {
            vec![json!({"text": self.documents[&doc].text()})]
        } else {
            changes
                .unwrap_or_default()
                .into_iter()
                .map(|c| {
                    json!({
                        "range": {"start": json_position(c.start_position), "end": json_position(c.old_end_position)},
                        "text": c.text
                    })
                })
                .collect()
        };
        self.notify(
            server,
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": content_changes}),
        );
        // Edits invalidate a shown hover.
        if self.lsp.hover.as_ref().is_some_and(|h| h.doc == doc) {
            self.lsp.hover = None;
        }
    }

    /// Tell the server a document was saved.
    pub(crate) fn lsp_saved(&mut self, doc: u64) {
        self.lsp_sync();
        if let Some(open) = self.lsp.docs.get(&doc) {
            let (server, uri) = (open.server, open.uri.clone());
            self.notify(
                server,
                "textDocument/didSave",
                json!({"textDocument": {"uri": uri}}),
            );
        }
    }

    /// Stop every server and forget failures; documents reopen on the next
    /// sync.
    pub(crate) fn lsp_restart(&mut self) {
        self.lsp_stop_all();
        self.lsp.failed.clear();
        self.lsp.languages = None;
    }

    pub(crate) fn lsp_stop_all(&mut self) {
        let ids: Vec<u64> = self.lsp.servers.keys().copied().collect();
        for id in ids {
            self.lsp_stop(id);
        }
    }

    fn lsp_stop(&mut self, server: u64) {
        if let Some(mut s) = self.lsp.servers.remove(&server) {
            // Ask politely, then make sure.
            s.process
                .send(&json!({"jsonrpc": "2.0", "id": 0, "method": "shutdown"}));
            s.process.send(&json!({"jsonrpc": "2.0", "method": "exit"}));
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                s.process.kill();
            });
        }
        self.lsp.keys.retain(|_, id| *id != server);
        self.lsp.docs.retain(|_, d| d.server != server);
        self.lsp.pending.retain(|(s, _), _| *s != server);
    }

    pub(crate) fn lsp_event(&mut self, event: Event) {
        match event {
            Event::Lsp { server, message } => self.lsp_message(server, message),
            Event::LspExited { server, reason } => {
                if let Some(s) = self.lsp.servers.remove(&server) {
                    self.status = format!("{} language server stopped ({reason})", s.language);
                    // Do not restart a crashing server in a loop.
                    self.lsp.failed.insert((s.language, s.root));
                }
                self.lsp.keys.retain(|_, id| *id != server);
                self.lsp.docs.retain(|_, d| d.server != server);
                self.lsp.pending.retain(|(s, _), _| *s != server);
            }
            _ => {}
        }
    }

    fn lsp_message(&mut self, server: u64, message: Value) {
        let id = message.get("id").cloned();
        let method = message["method"].as_str().map(str::to_owned);
        match (id, method) {
            // A request from the server.
            (Some(id), Some(method)) => {
                let result = self.server_request(&method, &message["params"]);
                if let Some(s) = self.lsp.servers.get(&server) {
                    s.process
                        .send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
                }
            }
            (None, Some(method)) => self.server_notification(&method, &message["params"]),
            (Some(id), None) => {
                let Some(id) = id.as_i64() else { return };
                let Some(pending) = self.lsp.pending.remove(&(server, id)) else {
                    return;
                };
                if let Some(error) = message.get("error") {
                    if let Pending::Formatting { doc, then_save, .. } = pending {
                        self.formatting.remove(&doc);
                        if then_save {
                            let _ = self.finish_format_on_save(doc);
                        }
                    }
                    if !matches!(pending, Pending::Ignore | Pending::Completion { .. }) {
                        self.status = format!(
                            "Language server: {}",
                            error["message"].as_str().unwrap_or("request failed")
                        );
                    }
                    if let Some(p) = self.picker.as_mut() {
                        p.busy = false;
                    }
                    return;
                }
                if let Err(e) = self.lsp_response(server, pending, &message["result"]) {
                    self.status = format!("Error: {e:#}");
                }
            }
            _ => {}
        }
    }

    fn server_request(&mut self, method: &str, params: &Value) -> Value {
        match method {
            "workspace/configuration" => Value::Array(
                params["items"]
                    .as_array()
                    .map(|items| items.iter().map(|_| Value::Null).collect())
                    .unwrap_or_default(),
            ),
            "workspace/applyEdit" => {
                let applied = self.apply_workspace_edit(&params["edit"]);
                if let Err(e) = &applied {
                    self.status = format!("Error: {e:#}");
                }
                json!({"applied": applied.is_ok()})
            }
            "workspace/workspaceFolders" => json!([{"uri": uri(&self.root), "name": ""}]),
            _ => Value::Null,
        }
    }

    fn server_notification(&mut self, method: &str, params: &Value) {
        match method {
            "textDocument/publishDiagnostics" => {
                let Some(path) = params["uri"].as_str().and_then(path_of) else {
                    return;
                };
                let list: Vec<Diagnostic> = params["diagnostics"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .map(|d| Diagnostic {
                                start: position(&d["range"]["start"]),
                                end: position(&d["range"]["end"]),
                                severity: d["severity"].as_u64().unwrap_or(1) as u8,
                                message: d["message"].as_str().unwrap_or_default().to_string(),
                                source: d["source"].as_str().unwrap_or_default().to_string(),
                                raw: d.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let key = self.diagnostic_key(&path);
                if list.is_empty() {
                    self.lsp.diagnostics.remove(&key);
                } else {
                    self.lsp.diagnostics.insert(key, list);
                }
                self.lsp.revision += 1;
                self.revision += 1;
            }
            "window/showMessage" => {
                let text = params["message"].as_str().unwrap_or_default().to_string();
                if params["type"].as_u64().unwrap_or(4) <= 2 {
                    self.status = text.clone();
                }
                self.lsp.messages.push(text);
                if self.lsp.messages.len() > 100 {
                    self.lsp.messages.remove(0);
                }
            }
            "$/progress" => {
                let token = params["token"].to_string();
                let value = &params["value"];
                match value["kind"].as_str() {
                    Some("end") => {
                        if let Some(title) = self.lsp.progress.remove(&token) {
                            if self.status.starts_with(&title) {
                                self.status = format!("{title} done");
                            }
                        }
                    }
                    _ => {
                        let title =
                            value["title"]
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| {
                                    self.lsp.progress.get(&token).cloned().unwrap_or_default()
                                });
                        let detail = value["message"].as_str().unwrap_or_default();
                        let percent = value["percentage"]
                            .as_u64()
                            .map(|p| format!(" {p}%"))
                            .unwrap_or_default();
                        self.lsp.progress.insert(token, title.clone());
                        self.status = format!("{title}{percent} {detail}").trim().to_string();
                    }
                }
            }
            _ => {}
        }
    }

    /// Diagnostics are keyed by the path documents use for the file.
    fn diagnostic_key(&self, path: &Path) -> PathBuf {
        self.document_for(path)
            .and_then(|doc| self.documents[&doc].path.clone())
            .unwrap_or_else(|| path.to_path_buf())
    }

    fn lsp_response(&mut self, server: u64, pending: Pending, result: &Value) -> Result<()> {
        match pending {
            Pending::Initialize => {
                if let Some(s) = self.lsp.servers.get_mut(&server) {
                    s.capabilities = result["capabilities"].clone();
                    s.ready = true;
                    s.process
                        .send(&json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
                    for message in std::mem::take(&mut s.queue) {
                        s.process.send(&message);
                    }
                    let name = result["serverInfo"]["name"]
                        .as_str()
                        .unwrap_or(&s.language)
                        .to_string();
                    self.status = format!("{name} ready");
                }
            }
            Pending::Hover { doc, offset } => {
                let text = hover_text(&result["contents"]);
                if text.is_empty() {
                    self.status = "No information here".into();
                } else {
                    self.lsp.hover = Some(Hover { doc, offset, text });
                }
            }
            Pending::Definition => {
                let locations = self.locations(result);
                match locations.len() {
                    0 => self.status = "No definition found".into(),
                    1 => self.goto(locations.into_iter().next().unwrap())?,
                    _ => self.location_picker("Definitions", locations),
                }
            }
            Pending::References => {
                let locations = self.locations(result);
                if locations.is_empty() {
                    self.status = "No references found".into();
                } else {
                    self.location_picker("References", locations);
                }
            }
            Pending::Completion { view, doc, offset } => {
                self.completion_results(view, doc, offset, result)
            }
            Pending::Rename => {
                let files = self.apply_workspace_edit(result)?;
                self.status = format!("Renamed in {files} files");
            }
            Pending::Formatting {
                doc,
                content,
                then_save,
            } => {
                if self.documents.get(&doc).map(|d| d.content_version()) != Some(content) {
                    self.formatting.remove(&doc);
                    if then_save {
                        self.finish_format_on_save(doc)?;
                    }
                    bail!("The document changed while formatting; format again");
                }
                let edits = self.text_edits(doc, result);
                let count = edits.len();
                if count > 0 {
                    let view = self.reveal_document(doc);
                    let before = self.views[&view].cursor;
                    self.documents
                        .get_mut(&doc)
                        .unwrap()
                        .replace_many(&edits, before)?;
                    self.clamp_views(doc);
                }
                self.status = if count == 0 {
                    "Already formatted".into()
                } else {
                    "Formatted".into()
                };
                if then_save {
                    self.finish_format_on_save(doc)?;
                } else {
                    self.formatting.remove(&doc);
                }
            }
            Pending::DocumentSymbols { doc } => {
                let mut symbols = Vec::new();
                fn walk(
                    items: &Value,
                    depth: usize,
                    out: &mut Vec<(String, String, Value, usize)>,
                ) {
                    for s in items.as_array().into_iter().flatten() {
                        let kind = symbol_kind(s["kind"].as_u64().unwrap_or(0)).to_string();
                        let name = s["name"].as_str().unwrap_or_default().to_string();
                        // DocumentSymbol has selectionRange; SymbolInformation a location.
                        let range = if s["selectionRange"].is_object() {
                            s["selectionRange"].clone()
                        } else {
                            s["location"]["range"].clone()
                        };
                        out.push((name, kind, range, depth));
                        walk(&s["children"], depth + 1, out);
                    }
                }
                walk(result, 0, &mut symbols);
                let path = self.documents.get(&doc).and_then(|d| d.path.clone());
                if let (Some(p), Some(path)) =
                    (self.picker.as_mut().filter(|p| p.kind == "symbols"), path)
                {
                    p.busy = false;
                    p.entries = symbols
                        .into_iter()
                        .map(|(name, kind, range, depth)| {
                            let (line, character) = position(&range["start"]);
                            Entry {
                                label: format!("{}{name}", "  ".repeat(depth)),
                                detail: format!("{kind} · line {}", line + 1),
                                kind,
                                target: Target::Location(Location {
                                    path: path.clone(),
                                    line,
                                    column: Column::Utf16(character),
                                }),
                            }
                        })
                        .collect();
                    if p.entries.is_empty() {
                        p.message = "No symbols found".into();
                    }
                    p.filter();
                }
            }
            Pending::WorkspaceSymbols { generation } => {
                let root = self.root.clone();
                if let Some(p) = self
                    .picker
                    .as_mut()
                    .filter(|p| p.kind == "workspace-symbols" && p.generation == generation)
                {
                    p.busy = false;
                    p.entries = result
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| {
                            let path = path_of(s["location"]["uri"].as_str()?)?;
                            let (line, character) = position(&s["location"]["range"]["start"]);
                            let relative = path
                                .strip_prefix(&root)
                                .unwrap_or(&path)
                                .to_string_lossy()
                                .into_owned();
                            Some(Entry {
                                label: s["name"].as_str()?.to_string(),
                                detail: format!(
                                    "{} · {relative}:{}",
                                    symbol_kind(s["kind"].as_u64().unwrap_or(0)),
                                    line + 1
                                ),
                                kind: symbol_kind(s["kind"].as_u64().unwrap_or(0)).into(),
                                target: Target::Location(Location {
                                    path,
                                    line,
                                    column: Column::Utf16(character),
                                }),
                            })
                        })
                        .collect();
                    p.message = if p.entries.is_empty() {
                        "No symbols found".into()
                    } else {
                        String::new()
                    };
                    p.filter();
                }
            }
            Pending::CodeActions => {
                let actions: Vec<Value> = result.as_array().cloned().unwrap_or_default();
                if actions.is_empty() {
                    self.status = "No code actions here".into();
                    return Ok(());
                }
                let mut picker = Picker::new("code-actions", "Code actions", true);
                picker.entries = actions
                    .into_iter()
                    .map(|action| Entry {
                        label: action["title"].as_str().unwrap_or("Action").to_string(),
                        detail: action["kind"].as_str().unwrap_or_default().to_string(),
                        kind: "action".into(),
                        target: Target::Command(Command::Action {
                            name: "apply-code-action".into(),
                            argument: format!("{server}\u{1f}{action}"),
                        }),
                    })
                    .collect();
                picker.filter();
                self.picker = Some(picker);
            }
            Pending::Ignore => {}
        }
        Ok(())
    }

    /// Locations from a definition/references result.
    fn locations(&self, result: &Value) -> Vec<Location> {
        let items: Vec<&Value> = match result {
            Value::Array(items) => items.iter().collect(),
            Value::Object(_) => vec![result],
            _ => vec![],
        };
        items
            .into_iter()
            .filter_map(|l| {
                let uri = l["uri"].as_str().or_else(|| l["targetUri"].as_str())?;
                let range = if l["targetSelectionRange"].is_object() {
                    &l["targetSelectionRange"]
                } else {
                    &l["range"]
                };
                let (line, character) = position(&range["start"]);
                Some(Location {
                    path: path_of(uri)?,
                    line,
                    column: Column::Utf16(character),
                })
            })
            .collect()
    }

    fn location_picker(&mut self, title: &str, locations: Vec<Location>) {
        let mut picker = Picker::new("locations", title, true);
        picker.entries = locations
            .into_iter()
            .map(|l| {
                let relative = l
                    .path
                    .strip_prefix(&self.root)
                    .unwrap_or(&l.path)
                    .to_string_lossy()
                    .into_owned();
                let preview = self
                    .document_for(&l.path)
                    .map(|doc| self.documents[&doc].line_text(l.line).trim().to_string())
                    .or_else(|| {
                        std::fs::read_to_string(&l.path)
                            .ok()
                            .and_then(|t| t.lines().nth(l.line).map(|s| s.trim().to_string()))
                    })
                    .unwrap_or_default();
                Entry {
                    label: if preview.is_empty() {
                        relative.clone()
                    } else {
                        preview
                    },
                    detail: format!("{relative}:{}", l.line + 1),
                    kind: "location".into(),
                    target: Target::Location(l),
                }
            })
            .collect();
        picker.filter();
        self.picker = Some(picker);
    }

    /// TextEdits as byte-range replacements in a document.
    fn text_edits(&self, doc: u64, edits: &Value) -> Vec<(usize, usize, String)> {
        let d = &self.documents[&doc];
        let mut out: Vec<(usize, usize, String)> = edits
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                let (sl, sc) = position(&e["range"]["start"]);
                let (el, ec) = position(&e["range"]["end"]);
                let start = d.offset_of(sl, sc);
                let end = d.offset_of(el, ec).max(start);
                (
                    start,
                    end,
                    e["newText"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        // Edits at the same position apply in the order given.
        out.sort_by_key(|(start, end, _)| (*start, *end));
        out
    }

    /// Apply a WorkspaceEdit: open documents change in the editor (undoable,
    /// unsaved); other files are rewritten on disk. Returns the files changed.
    pub(crate) fn apply_workspace_edit(&mut self, edit: &Value) -> Result<usize> {
        let mut per_file: Vec<(PathBuf, Value)> = Vec::new();
        if let Some(changes) = edit["changes"].as_object() {
            for (uri, edits) in changes {
                if let Some(path) = path_of(uri) {
                    per_file.push((path, edits.clone()));
                }
            }
        }
        for change in edit["documentChanges"].as_array().into_iter().flatten() {
            if change["kind"].is_string() {
                bail!("File operations in edits are not supported yet");
            }
            if let Some(path) = change["textDocument"]["uri"].as_str().and_then(path_of) {
                per_file.push((path, change["edits"].clone()));
            }
        }
        let mut files = 0;
        for (path, edits) in per_file {
            match self.document_for(&path) {
                Some(doc) => {
                    let replacements = self.text_edits(doc, &edits);
                    if replacements.is_empty() {
                        continue;
                    }
                    let cursor = self
                        .views
                        .values()
                        .find(|v| v.document == doc)
                        .map_or(0, |v| v.cursor);
                    self.documents
                        .get_mut(&doc)
                        .unwrap()
                        .replace_many(&replacements, cursor)?;
                    self.clamp_views(doc);
                }
                None => {
                    let mut d = crate::document::Document::open(&path)?;
                    let replacements: Vec<(usize, usize, String)> = {
                        let mut list: Vec<(usize, usize, String)> = edits
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|e| {
                                let (sl, sc) = position(&e["range"]["start"]);
                                let (el, ec) = position(&e["range"]["end"]);
                                let start = d.offset_of(sl, sc);
                                (
                                    start,
                                    d.offset_of(el, ec).max(start),
                                    e["newText"].as_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect();
                        list.sort_by_key(|(s, e, _)| (*s, *e));
                        list
                    };
                    d.replace_many(&replacements, 0)?;
                    d.save(None)?;
                }
            }
            files += 1;
        }
        self.revision += 1;
        Ok(files)
    }

    // ----- Requests the user makes -----

    fn editor_position(&mut self) -> Result<(u64, u64, u64, usize, Value)> {
        let id = self
            .active_editor()
            .ok_or_else(|| anyhow::anyhow!("Focus an editor first"))?;
        let v = &self.views[&id];
        let doc = v.document;
        let offset = v.cursor;
        if !self.trusted {
            bail!("Language servers run code from this folder. Trust the workspace first (trust-workspace)");
        }
        self.lsp_sync();
        let server = self
            .server_for(doc)
            .ok_or_else(|| anyhow::anyhow!("No language server for this document"))?;
        let open = &self.lsp.docs[&doc];
        let params = json!({
            "textDocument": {"uri": open.uri},
            "position": json_position(self.documents[&doc].position(offset)),
        });
        Ok((id, doc, server, offset, params))
    }

    pub(crate) fn lsp_hover(&mut self) -> Result<()> {
        let (_, doc, server, offset, params) = self.editor_position()?;
        self.request(
            server,
            "textDocument/hover",
            params,
            Pending::Hover { doc, offset },
        );
        Ok(())
    }

    pub(crate) fn lsp_definition(&mut self) -> Result<()> {
        let (_, _, server, _, params) = self.editor_position()?;
        self.request(
            server,
            "textDocument/definition",
            params,
            Pending::Definition,
        );
        Ok(())
    }

    pub(crate) fn lsp_references(&mut self) -> Result<()> {
        let (_, _, server, _, mut params) = self.editor_position()?;
        params["context"] = json!({"includeDeclaration": true});
        self.request(
            server,
            "textDocument/references",
            params,
            Pending::References,
        );
        Ok(())
    }

    pub(crate) fn lsp_rename(&mut self, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("Enter the new name");
        }
        let (_, _, server, _, mut params) = self.editor_position()?;
        if !self.capability(server, "renameProvider") {
            bail!("This language server cannot rename");
        }
        params["newName"] = json!(name);
        self.request(server, "textDocument/rename", params, Pending::Rename);
        Ok(())
    }

    pub(crate) fn lsp_code_actions(&mut self) -> Result<()> {
        let (id, doc, server, _, _) = self.editor_position()?;
        let v = &self.views[&id];
        let d = &self.documents[&doc];
        let (a, b) = {
            let anchor = v.anchor.unwrap_or(v.cursor);
            (v.cursor.min(anchor), v.cursor.max(anchor))
        };
        let (start, end) = (d.position(a), d.position(b));
        let path = d.path.clone().unwrap_or_default();
        let diagnostics: Vec<Value> = self
            .lsp
            .diagnostics
            .get(&path)
            .into_iter()
            .flatten()
            .filter(|x| x.start.0 <= end.0 && x.end.0 >= start.0)
            .map(|x| x.raw.clone())
            .filter(|raw| !raw.is_null())
            .collect();
        let uri = self.lsp.docs[&doc].uri.clone();
        self.request(
            server,
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": uri},
                "range": {"start": json_position(start), "end": json_position(end)},
                "context": {"diagnostics": diagnostics}
            }),
            Pending::CodeActions,
        );
        Ok(())
    }

    /// Run a chosen code action: apply its edit, then its command.
    pub(crate) fn apply_code_action(&mut self, argument: &str) -> Result<()> {
        let (server, action) = argument
            .split_once('\u{1f}')
            .ok_or_else(|| anyhow::anyhow!("Invalid code action"))?;
        let server: u64 = server.parse()?;
        let action: Value = serde_json::from_str(action)?;
        if action["edit"].is_object() {
            self.apply_workspace_edit(&action["edit"])?;
        }
        // A bare Command, or a CodeAction carrying one.
        let command = if action["command"].is_string() {
            action.clone()
        } else {
            action["command"].clone()
        };
        if command["command"].is_string() {
            self.request(
                server,
                "workspace/executeCommand",
                json!({"command": command["command"], "arguments": command["arguments"]}),
                Pending::Ignore,
            );
        }
        self.status = format!("Applied {}", action["title"].as_str().unwrap_or("action"));
        Ok(())
    }

    /// Format with the language server. Returns false when it cannot.
    pub(crate) fn lsp_format(&mut self, doc: u64, then_save: bool) -> bool {
        if !self.trusted {
            return false;
        }
        self.lsp_sync();
        let Some(server) = self.server_for(doc) else {
            return false;
        };
        if !self.capability(server, "documentFormattingProvider") {
            return false;
        }
        let uri = self.lsp.docs[&doc].uri.clone();
        let content = self.documents[&doc].content_version();
        let options = json!({
            "tabSize": self.preferences.indent_width,
            "insertSpaces": self.preferences.insert_spaces,
            "trimTrailingWhitespace": true,
        });
        self.request(
            server,
            "textDocument/formatting",
            json!({"textDocument": {"uri": uri}, "options": options}),
            Pending::Formatting {
                doc,
                content,
                then_save,
            },
        );
        true
    }

    pub(crate) fn lsp_document_symbols(&mut self, doc: u64) -> bool {
        if !self.trusted {
            return false;
        }
        self.lsp_sync();
        let Some(server) = self.server_for(doc) else {
            return false;
        };
        if !self.capability(server, "documentSymbolProvider") {
            return false;
        }
        let uri = self.lsp.docs[&doc].uri.clone();
        self.request(
            server,
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}}),
            Pending::DocumentSymbols { doc },
        );
        true
    }

    pub(crate) fn request_workspace_symbols(&mut self) -> Result<()> {
        let query = self
            .picker
            .as_ref()
            .map(|p| p.query.clone())
            .unwrap_or_default();
        self.lsp_sync();
        let server = self
            .lsp
            .servers
            .iter()
            .find(|(_, s)| {
                let v = &s.capabilities["workspaceSymbolProvider"];
                !v.is_null() && v != &Value::Bool(false)
            })
            .map(|(id, _)| *id);
        let Some(server) = server else {
            if let Some(p) = self.picker.as_mut() {
                p.message = "No language server offers workspace symbols".into();
            }
            return Ok(());
        };
        let generation = self.lsp.next_id as u64 + 1;
        if let Some(p) = self.picker.as_mut() {
            p.generation = generation;
            p.busy = true;
        }
        self.request(
            server,
            "workspace/symbol",
            json!({"query": query}),
            Pending::WorkspaceSymbols { generation },
        );
        Ok(())
    }

    // ----- Completion -----

    /// Ask for completions at the caret. `manual` comes from Ctrl+Space.
    pub(crate) fn lsp_complete(&mut self, manual: bool) -> Result<()> {
        let (view, doc, server, offset, params) = match self.editor_position() {
            Ok(p) => p,
            Err(e) if manual => return Err(e),
            Err(_) => return Ok(()),
        };
        if !self.capability(server, "completionProvider") {
            if manual {
                bail!("This language server does not complete");
            }
            return Ok(());
        }
        self.request(
            server,
            "textDocument/completion",
            params,
            Pending::Completion { view, doc, offset },
        );
        Ok(())
    }

    /// The identifier being typed before `offset`: its start.
    fn word_start(&self, doc: u64, offset: usize) -> usize {
        let d = &self.documents[&doc];
        let line = d.line_of(offset);
        let (start, _) = d.line_range(line);
        let before = d.slice(start, offset);
        let word = before
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
            .last()
            .map_or(before.len(), |(i, _)| i);
        start + word
    }

    fn completion_results(&mut self, view: u64, doc: u64, offset: usize, result: &Value) {
        if !self.views.contains_key(&view) || self.views[&view].cursor < offset {
            return;
        }
        let (items, incomplete) = match result {
            Value::Array(items) => (items.clone(), false),
            Value::Object(o) => (
                o.get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                o.get("isIncomplete")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
            _ => (vec![], false),
        };
        let start = self.word_start(doc, offset);
        let d = &self.documents[&doc];
        let mut candidates: Vec<Candidate> = items
            .into_iter()
            .take(COMPLETION_LIMIT)
            .map(|item| {
                let label = item["label"].as_str().unwrap_or_default().to_string();
                let edit = &item["textEdit"];
                let range_value = if edit["range"].is_object() {
                    &edit["range"]
                } else {
                    &edit["replace"]
                };
                let range = range_value.is_object().then(|| {
                    let (sl, sc) = position(&range_value["start"]);
                    let (el, ec) = position(&range_value["end"]);
                    (d.offset_of(sl, sc), d.offset_of(el, ec))
                });
                let insert = edit["newText"]
                    .as_str()
                    .or_else(|| item["insertText"].as_str())
                    .unwrap_or(&label)
                    .to_string();
                let additional = item["additionalTextEdits"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|e| {
                        let (sl, sc) = position(&e["range"]["start"]);
                        let (el, ec) = position(&e["range"]["end"]);
                        (
                            d.offset_of(sl, sc),
                            d.offset_of(el, ec),
                            e["newText"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect();
                Candidate {
                    item: CompletionItem {
                        detail: item["detail"].as_str().unwrap_or_default().to_string(),
                        kind: completion_kind(item["kind"].as_u64().unwrap_or(1)).into(),
                        label: label.clone(),
                    },
                    filter: item["filterText"].as_str().unwrap_or(&label).to_string(),
                    insert,
                    snippet: item["insertTextFormat"].as_u64() == Some(2),
                    range,
                    additional,
                }
            })
            .collect();
        // Snippets for the language join the list.
        let language = crate::highlight::syntax_for(d.path.as_deref(), &d.line_text(0))
            .name
            .clone();
        for s in crate::snippets::available(&language) {
            candidates.push(Candidate {
                item: CompletionItem {
                    label: s.prefix.clone(),
                    detail: if s.description.is_empty() {
                        "snippet".into()
                    } else {
                        s.description.clone()
                    },
                    kind: "snippet".into(),
                },
                filter: s.prefix.clone(),
                insert: s.body.clone(),
                snippet: true,
                range: None,
                additional: vec![],
            });
        }
        if candidates.is_empty() {
            self.lsp.completion = None;
            return;
        }
        self.lsp.completion = Some(Completion {
            view,
            doc,
            start,
            candidates,
            shown: Vec::new(),
            selected: 0,
            incomplete,
        });
        self.filter_completion();
    }

    /// Narrow completions to the text typed since they were requested.
    fn filter_completion(&mut self) {
        let Some(c) = self.lsp.completion.as_ref() else {
            return;
        };
        let Some(v) = self.views.get(&c.view) else {
            self.lsp.completion = None;
            return;
        };
        if v.cursor < c.start || v.document != c.doc {
            self.lsp.completion = None;
            return;
        }
        let typed = self.documents[&c.doc].slice(c.start, v.cursor).into_owned();
        use nucleo_matcher::{
            pattern::{CaseMatching, Normalization, Pattern},
            Config, Matcher, Utf32Str,
        };
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(&typed, CaseMatching::Smart, Normalization::Smart);
        let mut buffer = Vec::new();
        let mut scored: Vec<(u32, usize)> = c
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(i, candidate)| {
                if typed.is_empty() {
                    return Some((0, i));
                }
                let haystack = Utf32Str::new(&candidate.filter, &mut buffer);
                pattern.score(haystack, &mut matcher).map(|s| (s, i))
            })
            .collect();
        // Snippets only show once their prefix is typed, and a suggestion
        // that would insert exactly what is already there is noise.
        scored.retain(|(_, i)| {
            let candidate = &c.candidates[*i];
            (candidate.item.kind != "snippet" || !typed.is_empty())
                && !(candidate.insert == typed && candidate.range.is_none())
        });
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let c = self.lsp.completion.as_mut().unwrap();
        c.shown = scored.into_iter().map(|(_, i)| i).collect();
        c.selected = 0;
        if c.shown.is_empty() {
            self.lsp.completion = None;
        }
    }

    fn accept_completion(&mut self) -> Result<()> {
        let Some(c) = self.lsp.completion.take() else {
            return Ok(());
        };
        let Some(index) = c.shown.get(c.selected) else {
            return Ok(());
        };
        let candidate = &c.candidates[*index];
        let cursor = self.views[&c.view].cursor;
        let (start, end) = match candidate.range {
            // The server's range was computed before more was typed.
            Some((a, b)) => (a.min(c.start), b.max(cursor)),
            None => (c.start, cursor),
        };
        if candidate.snippet {
            let line = self.documents[&c.doc].line_of(start);
            let indent: String = self.documents[&c.doc]
                .line_text(line)
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .collect();
            let expansion =
                crate::snippets::expand(&candidate.insert, &indent, &self.indent_unit());
            self.insert_expansion(c.view, start, end, expansion)?;
        } else {
            let mut edits = candidate.additional.clone();
            edits.push((start, end, candidate.insert.clone()));
            let placed = self
                .documents
                .get_mut(&c.doc)
                .unwrap()
                .replace_many(&edits, cursor)?;
            let (_, after) = *placed.last().unwrap();
            for (s, e, text) in edits.iter().rev() {
                self.rebase_views(c.doc, *s, *e, text.len());
            }
            let v = self.views.get_mut(&c.view).unwrap();
            v.cursor = after;
            v.anchor = None;
        }
        Ok(())
    }

    /// Accept the completion at `index` in the list sent to frontends.
    pub(crate) fn completion_select(&mut self, index: usize) -> Result<()> {
        if let Some(c) = self.lsp.completion.as_mut() {
            let first = c.selected.saturating_sub(50);
            c.selected = (first + index).min(c.shown.len().saturating_sub(1));
        }
        self.accept_completion()
    }

    /// Keys while completions are shown. Returns true when used.
    pub(crate) fn completion_key(&mut self, k: &crate::Key) -> Result<bool> {
        if self.lsp.completion.is_none() {
            return Ok(false);
        }
        if k.ctrl || k.alt {
            if k.key == " " || k.key.eq_ignore_ascii_case("space") {
                return Ok(true);
            }
            self.lsp.completion = None;
            return Ok(false);
        }
        let c = self.lsp.completion.as_mut().unwrap();
        match k.key.as_str() {
            "Up" => c.selected = c.selected.saturating_sub(1),
            "Down" => c.selected = (c.selected + 1).min(c.shown.len().saturating_sub(1)),
            "PageUp" => c.selected = c.selected.saturating_sub(8),
            "PageDown" => c.selected = (c.selected + 8).min(c.shown.len().saturating_sub(1)),
            "Enter" | "Tab" => self.accept_completion()?,
            "Escape" => self.lsp.completion = None,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// After typing: narrow open completions, or ask for new ones after an
    /// identifier character or a trigger character.
    pub(crate) fn after_typing(&mut self, typed: &str) {
        if !self.trusted {
            return;
        }
        let word = typed.chars().all(|c| c.is_alphanumeric() || c == '_');
        if self.lsp.completion.is_some() {
            if word {
                let incomplete = self.lsp.completion.as_ref().is_some_and(|c| c.incomplete);
                self.filter_completion();
                if incomplete {
                    let _ = self.lsp_complete(false);
                }
                return;
            }
            self.lsp.completion = None;
        }
        let Some(id) = self.active_editor() else {
            return;
        };
        let doc = self.views[&id].document;
        let Some(server) = self.lsp.docs.get(&doc).map(|d| d.server) else {
            return;
        };
        let triggers: Vec<String> = self
            .lsp
            .servers
            .get(&server)
            .and_then(|s| {
                s.capabilities["completionProvider"]["triggerCharacters"]
                    .as_array()
                    .cloned()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect();
        if word || triggers.iter().any(|t| t == typed) {
            let _ = self.lsp_complete(false);
        }
    }

    /// The screen cell of an offset in a view, if visible.
    fn screen_cell(&self, view: u64, offset: usize) -> Option<(usize, usize)> {
        let v = &self.views[&view];
        let (line, row, col) = self.visual_position(view, offset);
        let shown = self.visible_rows(view, v.rows as usize);
        let y = shown.iter().position(|(l, r, _)| *l == line && *r == row)?;
        let gutter = self.gutter_width(v.document, v.cols.max(1));
        let left = if self.preferences.soft_wrap {
            0
        } else {
            v.left
        };
        Some((y, gutter + col.checked_sub(left)?))
    }

    fn pane_of(&self, view: u64) -> Option<u64> {
        self.layout
            .panes()
            .into_iter()
            .find(|p| self.layout.view(*p) == Some(&crate::layout::View::Editor(view)))
    }

    pub fn completion_view(&self) -> Option<CompletionView> {
        let c = self.lsp.completion.as_ref()?;
        let pane = self.pane_of(c.view)?;
        let (row, col) = self.screen_cell(c.view, c.start)?;
        let first = c.selected.saturating_sub(50);
        Some(CompletionView {
            pane,
            row,
            col,
            items: c
                .shown
                .iter()
                .skip(first)
                .take(100)
                .map(|i| c.candidates[*i].item.clone())
                .collect(),
            selected: c.selected - first,
        })
    }

    pub fn hover_view(&self) -> Option<HoverView> {
        let h = self.lsp.hover.as_ref()?;
        let view = self.active_editor()?;
        if self.views[&view].document != h.doc {
            return None;
        }
        let pane = self.pane_of(view)?;
        let (row, col) = self.screen_cell(view, h.offset)?;
        Some(HoverView {
            pane,
            row,
            col,
            text: h.text.clone(),
        })
    }

    pub(crate) fn dismiss_hover(&mut self) -> bool {
        self.lsp.hover.take().is_some()
    }

    // ----- Diagnostics -----

    /// Diagnostics of a document as byte ranges: (start, end, severity).
    pub(crate) fn diagnostic_spans(&self, doc: u64) -> Vec<(usize, usize, u8)> {
        let d = &self.documents[&doc];
        let Some(list) = d.path.as_ref().and_then(|p| self.lsp.diagnostics.get(p)) else {
            return Vec::new();
        };
        let mut spans: Vec<(usize, usize, u8)> = list
            .iter()
            .map(|x| {
                let start = d.offset_of(x.start.0, x.start.1);
                let mut end = d.offset_of(x.end.0, x.end.1);
                if end <= start {
                    // Empty ranges mark the next character (or the line end).
                    end = d.next_boundary(start).min(d.line_range(d.line_of(start)).1);
                }
                (start, end.max(start), x.severity)
            })
            .collect();
        spans.sort_unstable();
        spans
    }

    /// The most severe diagnostic per line, for gutter marks.
    pub(crate) fn diagnostic_lines(&self, doc: u64) -> BTreeMap<usize, u8> {
        let d = &self.documents[&doc];
        let mut lines: BTreeMap<usize, u8> = BTreeMap::new();
        for x in d
            .path
            .as_ref()
            .and_then(|p| self.lsp.diagnostics.get(p))
            .into_iter()
            .flatten()
        {
            let entry = lines.entry(x.start.0).or_insert(x.severity);
            *entry = (*entry).min(x.severity);
        }
        lines
    }

    pub(crate) fn diagnostic_entries(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        for (path, list) in &self.lsp.diagnostics {
            let relative = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            for x in list {
                entries.push((
                    x.severity,
                    Entry {
                        label: x.message.lines().next().unwrap_or_default().to_string(),
                        detail: format!(
                            "{} · {relative}:{}:{}{}",
                            severity_name(x.severity),
                            x.start.0 + 1,
                            x.start.1 + 1,
                            if x.source.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", x.source)
                            }
                        ),
                        kind: severity_name(x.severity).into(),
                        target: Target::Location(Location {
                            path: path.clone(),
                            line: x.start.0,
                            column: Column::Utf16(x.start.1),
                        }),
                    },
                ));
            }
        }
        entries.sort_by_key(|(severity, _)| *severity);
        entries.into_iter().map(|(_, e)| e).collect()
    }

    /// (errors, warnings) across the workspace.
    pub fn problem_counts(&self) -> (usize, usize) {
        let all = self.lsp.diagnostics.values().flatten();
        let (mut errors, mut warnings) = (0, 0);
        for x in all {
            match x.severity {
                1 => errors += 1,
                2 => warnings += 1,
                _ => {}
            }
        }
        (errors, warnings)
    }

    /// The diagnostic under the caret, for the status bar.
    pub fn problem_here(&self) -> Option<String> {
        let id = self.active_editor()?;
        let v = &self.views[&id];
        let d = &self.documents[&v.document];
        let list = self.lsp.diagnostics.get(d.path.as_ref()?)?;
        let (line, col) = d.position(v.cursor);
        list.iter()
            .filter(|x| x.start.0 <= line && line <= x.end.0)
            .min_by_key(|x| {
                (
                    x.severity,
                    !((x.start.0, x.start.1) <= (line, col) && (line, col) <= (x.end.0, x.end.1)),
                )
            })
            .map(|x| {
                format!(
                    "{}: {}",
                    severity_name(x.severity),
                    x.message.lines().next().unwrap_or_default()
                )
            })
    }

    /// Move to the next (or previous) diagnostic in the document.
    pub(crate) fn next_problem(&mut self, backwards: bool) -> Result<()> {
        let id = self
            .active_editor()
            .ok_or_else(|| anyhow::anyhow!("Focus an editor first"))?;
        let v = self.views[&id].clone();
        let mut spans = self.diagnostic_spans(v.document);
        spans.dedup_by_key(|s| s.0);
        let target = if backwards {
            spans.iter().rev().find(|s| s.0 < v.cursor).or(spans.last())
        } else {
            spans.iter().find(|s| s.0 > v.cursor).or(spans.first())
        };
        let Some((start, _, _)) = target.copied() else {
            bail!("No problems in this document");
        };
        let view = self.views.get_mut(&id).unwrap();
        view.cursor = start;
        view.anchor = None;
        view.manual_scroll = false;
        if let Some(text) = self.problem_here() {
            self.status = text;
        }
        Ok(())
    }

    /// Diagnostics for a file from a task's problem matcher.
    pub(crate) fn set_task_diagnostics(&mut self, results: BTreeMap<PathBuf, Vec<Diagnostic>>) {
        self.lsp.diagnostics.retain(|_, list| {
            list.retain(|d| d.source != "task");
            !list.is_empty()
        });
        for (path, list) in results {
            let key = self.diagnostic_key(&path);
            self.lsp.diagnostics.entry(key).or_default().extend(list);
        }
        self.lsp.revision += 1;
        self.revision += 1;
    }

    pub(crate) fn lsp_revision(&self) -> u64 {
        self.lsp.revision
    }

    /// Language servers running, for the status bar.
    pub fn language_servers(&self) -> Vec<String> {
        self.lsp
            .servers
            .values()
            .map(|s| format!("{}{}", s.language, if s.ready { "" } else { " (starting)" }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_with_spaces_and_unicode() {
        let path = Path::new("/tmp/a dir/ünï#1.rs");
        let u = uri(path);
        assert_eq!(u, "file:///tmp/a%20dir/%C3%BCn%C3%AF%231.rs");
        assert_eq!(path_of(&u).unwrap(), path);
        assert_eq!(path_of("file://localhost/x/y").unwrap(), Path::new("/x/y"));
        assert!(path_of("https://example.com").is_none());
    }

    #[test]
    fn diagnostics_follow_edits_before_the_server_reports() {
        let diagnostic = |start, end| Diagnostic {
            start,
            end,
            severity: 1,
            message: String::new(),
            source: String::new(),
            raw: Value::Null,
        };
        let change = |start, old_end, text: &str| crate::document::TextChange {
            start: 0,
            old_end: 0,
            start_position: start,
            old_end_position: old_end,
            text: text.into(),
        };
        let mut list = vec![
            diagnostic((1, 6), (1, 11)),
            diagnostic((0, 0), (0, 3)),
            diagnostic((3, 2), (3, 4)),
        ];
        // Typing at the start of line 1 moves its diagnostic right.
        shift_diagnostics(&mut list, &change((1, 0), (1, 0), "X"));
        assert_eq!((list[0].start, list[0].end), ((1, 7), (1, 12)));
        assert_eq!(list[1].start, (0, 0), "earlier positions stay");
        // A new line above moves later lines down.
        shift_diagnostics(&mut list, &change((0, 3), (0, 3), "\nnew"));
        assert_eq!(list[0].start, (2, 7));
        assert_eq!(list[2].start, (4, 2));
        // Deleting a line moves them back up.
        shift_diagnostics(&mut list, &change((0, 3), (1, 3), ""));
        assert_eq!(list[0].start, (1, 7));
        assert_eq!(list[2].start, (3, 2));
    }

    #[test]
    fn hover_markdown_becomes_plain_text() {
        let value = json!({"kind": "markdown", "value": "```rust\nfn f()\n```\nDocs"});
        assert_eq!(hover_text(&value), "fn f()\nDocs");
        assert_eq!(
            hover_text(&json!(["a", {"language": "x", "value": "b"}])),
            "a\n\nb"
        );
    }
}
