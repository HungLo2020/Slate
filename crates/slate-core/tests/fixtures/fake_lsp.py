#!/usr/bin/env python3
"""A small language server for Slate's tests.

It keeps its own copy of every open document, applying incremental changes
with UTF-16 positions, and writes that copy to $FAKE_LSP_DUMP/<name> after
each change so tests can compare it with the editor's text.

Language: lines `def NAME` define NAME. Lines containing ERROR or FIXME get
diagnostics. Formatting strips trailing spaces.
"""
import json
import os
import re
import sys

DUMP = os.environ.get("FAKE_LSP_DUMP")
docs = {}
next_id = 1000


def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode().strip()
        if not line:
            break
        name, value = line.split(":", 1)
        headers[name.lower()] = value.strip()
    body = sys.stdin.buffer.read(int(headers["content-length"]))
    return json.loads(body)


def send(message):
    message["jsonrpc"] = "2.0"
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


def offset(text, position):
    """Byte-free string index of an LSP (line, UTF-16 character) position."""
    lines = text.split("\n")
    line = min(position["line"], len(lines) - 1)
    index = sum(len(l) + 1 for l in lines[:line])
    units = 0
    for ch in lines[line]:
        if units >= position["character"]:
            break
        units += 2 if ord(ch) > 0xFFFF else 1
        index += 1
    return index


def position(text, index):
    before = text[:index]
    line = before.count("\n")
    start = before.rfind("\n") + 1
    character = sum(2 if ord(c) > 0xFFFF else 1 for c in before[start:])
    return {"line": line, "character": character}


def span(text, start, end):
    return {"start": position(text, start), "end": position(text, end)}


def dump(uri):
    if DUMP:
        name = uri.rsplit("/", 1)[-1]
        with open(os.path.join(DUMP, name), "w") as f:
            f.write(docs[uri])


def publish(uri):
    text = docs[uri]
    diagnostics = []
    for m in re.finditer(r"ERROR|FIXME", text):
        diagnostics.append({
            "range": span(text, m.start(), m.end()),
            "severity": 1 if m.group() == "ERROR" else 2,
            "message": "an error here" if m.group() == "ERROR" else "fix me",
            "source": "fake",
        })
    send({"method": "textDocument/publishDiagnostics",
          "params": {"uri": uri, "diagnostics": diagnostics}})


def word_at(text, index):
    start = index
    while start > 0 and (text[start - 1].isalnum() or text[start - 1] == "_"):
        start -= 1
    end = index
    while end < len(text) and (text[end].isalnum() or text[end] == "_"):
        end += 1
    return text[start:end], start, end


def definitions(name):
    for uri, text in docs.items():
        for m in re.finditer(r"^def (\w+)", text, re.M):
            if m.group(1) == name:
                yield {"uri": uri, "range": span(text, m.start(1), m.end(1))}


def occurrences(name):
    for uri, text in docs.items():
        for m in re.finditer(r"\b%s\b" % re.escape(name), text):
            yield uri, m.start(), m.end()


def handle(message):
    global next_id
    method = message.get("method")
    params = message.get("params", {})
    mid = message.get("id")
    if method is None and mid == 1001 and DUMP:
        with open(os.path.join(DUMP, "configuration.json"), "w") as f:
            json.dump(message.get("result"), f)
    elif method == "initialize":
        if DUMP:
            with open(os.path.join(DUMP, "initialize.json"), "w") as f:
                json.dump(params, f)
        # Exercise server-to-client requests during start-up.
        send({"id": next_id, "method": "window/workDoneProgress/create", "params": {"token": "load"}})
        next_id += 1
        send({"id": next_id, "method": "workspace/configuration", "params": {"items": [{"section": "fake"}]}})
        next_id += 1
        send({"id": mid, "result": {
            "serverInfo": {"name": "fake-lsp"},
            "capabilities": {
                "textDocumentSync": {"openClose": True, "change": 2, "save": True},
                "hoverProvider": True,
                "signatureHelpProvider": {"triggerCharacters": ["("]},
                "definitionProvider": True,
                "referencesProvider": True,
                "renameProvider": True,
                "documentFormattingProvider": True,
                "documentSymbolProvider": True,
                "workspaceSymbolProvider": True,
                "codeActionProvider": True,
                "completionProvider": {"triggerCharacters": ["."]},
            }}})
    elif method == "initialized":
        send({"method": "$/progress", "params": {"token": "load", "value": {"kind": "begin", "title": "Indexing"}}})
        send({"method": "$/progress", "params": {"token": "load", "value": {"kind": "end"}}})
    elif method == "workspace/didChangeConfiguration":
        if DUMP:
            with open(os.path.join(DUMP, "settings.json"), "w") as f:
                json.dump(params["settings"], f)
    elif method == "textDocument/signatureHelp":
        send({"id": mid, "result": {"signatures": [{"label": "call(arg: int)", "documentation": "Argument documentation"}], "activeSignature": 0, "activeParameter": 0}})
    elif method == "textDocument/didOpen":
        doc = params["textDocument"]
        docs[doc["uri"]] = doc["text"]
        dump(doc["uri"])
        publish(doc["uri"])
    elif method == "textDocument/didChange":
        uri = params["textDocument"]["uri"]
        for change in params["contentChanges"]:
            text = docs[uri]
            if "range" in change:
                start = offset(text, change["range"]["start"])
                end = offset(text, change["range"]["end"])
                docs[uri] = text[:start] + change["text"] + text[end:]
            else:
                docs[uri] = change["text"]
        dump(uri)
        publish(uri)
    elif method == "textDocument/didSave":
        if DUMP:
            open(os.path.join(DUMP, "saved"), "a").write(params["textDocument"]["uri"] + "\n")
    elif method == "textDocument/didClose":
        docs.pop(params["textDocument"]["uri"], None)
    elif method == "textDocument/hover":
        uri = params["textDocument"]["uri"]
        text = docs[uri]
        word, _, _ = word_at(text, offset(text, params["position"]))
        send({"id": mid, "result": {"contents": {"kind": "markdown", "value": "```\n%s\n```\nhover for %s" % (word, word)}} if word else None})
    elif method == "textDocument/definition":
        uri = params["textDocument"]["uri"]
        text = docs[uri]
        word, _, _ = word_at(text, offset(text, params["position"]))
        send({"id": mid, "result": list(definitions(word))})
    elif method == "textDocument/references":
        uri = params["textDocument"]["uri"]
        text = docs[uri]
        word, _, _ = word_at(text, offset(text, params["position"]))
        send({"id": mid, "result": [{"uri": u, "range": span(docs[u], s, e)} for u, s, e in occurrences(word)]})
    elif method == "textDocument/rename":
        uri = params["textDocument"]["uri"]
        text = docs[uri]
        word, _, _ = word_at(text, offset(text, params["position"]))
        changes = {}
        for u, s, e in occurrences(word):
            changes.setdefault(u, []).append({"range": span(docs[u], s, e), "newText": params["newName"]})
        # A file the server knows about but the editor has not opened.
        extra = os.environ.get("FAKE_LSP_EXTRA")
        if extra:
            content = open(extra).read()
            for m in re.finditer(r"\b%s\b" % re.escape(word), content):
                changes.setdefault("file://" + extra, []).append(
                    {"range": span(content, m.start(), m.end()), "newText": params["newName"]})
        send({"id": mid, "result": {"changes": changes}})
    elif method == "textDocument/formatting":
        uri = params["textDocument"]["uri"]
        text = docs[uri]
        edits = []
        index = 0
        for line in text.split("\n"):
            stripped = line.rstrip(" ")
            if stripped != line:
                edits.append({"range": span(text, index + len(stripped), index + len(line)), "newText": ""})
            index += len(line) + 1
        send({"id": mid, "result": edits})
    elif method == "textDocument/documentSymbol":
        uri = params["textDocument"]["uri"]
        text = docs[uri]
        symbols = []
        for m in re.finditer(r"^def (\w+)", text, re.M):
            r = span(text, m.start(), m.end())
            symbols.append({"name": m.group(1), "kind": 12, "range": r,
                            "selectionRange": span(text, m.start(1), m.end(1)), "children": []})
        send({"id": mid, "result": symbols})
    elif method == "workspace/symbol":
        query = params.get("query", "")
        result = []
        for uri, text in docs.items():
            for m in re.finditer(r"^def (\w+)", text, re.M):
                if query.lower() in m.group(1).lower():
                    result.append({"name": m.group(1), "kind": 12,
                                   "location": {"uri": uri, "range": span(text, m.start(1), m.end(1))}})
        send({"id": mid, "result": result})
    elif method == "textDocument/codeAction":
        uri = params["textDocument"]["uri"]
        text = docs[uri]
        actions = []
        for d in params["context"]["diagnostics"]:
            if d["message"] == "an error here":
                actions.append({"title": "Replace ERROR with OK", "kind": "quickfix",
                                "edit": {"changes": {uri: [{"range": d["range"], "newText": "OK"}]}}})
        actions.append({"title": "Add a header comment", "kind": "refactor",
                        "command": {"title": "header", "command": "fake.header", "arguments": [uri]}})
        send({"id": mid, "result": actions})
    elif method == "workspace/executeCommand":
        uri = params["arguments"][0]
        # Ask the editor to apply an edit, as servers do for commands.
        send({"id": next_id, "method": "workspace/applyEdit", "params": {"edit": {"changes": {
            uri: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                   "newText": "# header\n"}]}}}})
        next_id += 1
        send({"id": mid, "result": None})
    elif method == "textDocument/completion":
        uri = params["textDocument"]["uri"]
        text = docs[uri]
        index = offset(text, params["position"])
        words = sorted(set(re.findall(r"[A-Za-z_]\w{2,}", text)))
        items = [{"label": w, "kind": 6} for w in words]
        items.append({"label": "print_line", "kind": 3, "detail": "fn(text)",
                      "insertText": "print_line(${1:text})$0", "insertTextFormat": 2})
        send({"id": mid, "result": {"isIncomplete": False, "items": items}})
    elif method == "shutdown":
        send({"id": mid, "result": None})
    elif method == "exit":
        sys.exit(0)
    elif mid is not None and method is not None:
        send({"id": mid, "error": {"code": -32601, "message": "unknown method " + method}})


while True:
    message = read()
    if message is None:
        break
    handle(message)
