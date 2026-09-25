//! The pyprojx language server, started with `pyprojx server`.
//!
//! It checks open `pyproject.toml` documents as they change, publishing
//! pyprojx's diagnostics, and offers their fixes as quick fixes and safe fixes
//! as a fix-all action. See `docs/decisions/0002-language-server.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    ClientCapabilities, CodeAction, CodeActionKind, CodeActionOptions, CodeActionParams,
    CodeActionProvider, CodeActionResponse, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, InitializeParams, InitializeResult,
    PositionEncodingKind, PublishDiagnosticsParams, ServerCapabilities, ServerInfo,
    TextDocumentContentChangeEvent, TextDocumentSync, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextEdit, Uri, WorkspaceEdit,
};
use pyprojx_core::lock::{LockKind, find_with};
use pyprojx_core::{Applicability, Checked, Lock};

mod convert;
mod position;

use position::{Encoding, LineIndex};

/// The kind of the action that applies every safe fix.
const FIX_ALL: &str = "source.fixAll.pyprojx";

/// Runs the server over standard input and output until the client exits.
pub fn run() -> Result<(), String> {
    let (connection, threads) = Connection::stdio();
    serve(&connection)?;
    drop(connection);
    threads.join().map_err(|error| error.to_string())
}

/// Runs the server over `connection` until the client asks it to shut down.
pub fn serve(connection: &Connection) -> Result<(), String> {
    let (id, params) = connection
        .initialize_start()
        .map_err(|error| error.to_string())?;
    let params: InitializeParams =
        serde_json::from_value(params).map_err(|error| error.to_string())?;
    let encoding = negotiate_encoding(&params.capabilities);
    let result = InitializeResult {
        capabilities: capabilities(encoding),
        server_info: Some(ServerInfo {
            name: "pyprojx".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    };
    let result = serde_json::to_value(result).map_err(|error| error.to_string())?;
    connection
        .initialize_finish(id, result)
        .map_err(|error| error.to_string())?;

    let mut server = Server {
        encoding,
        documents: HashMap::new(),
        locks: HashMap::new(),
    };
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection
                    .handle_shutdown(&request)
                    .map_err(|error| error.to_string())?
                {
                    return Ok(());
                }
                let response = server.request(request);
                send(connection, Message::Response(response))?;
            }
            Message::Notification(notification) => {
                for message in server.notification(notification) {
                    send(connection, message)?;
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn send(connection: &Connection, message: Message) -> Result<(), String> {
    connection
        .sender
        .send(message)
        .map_err(|error| error.to_string())
}

/// UTF-8 if the client can count characters that way, which is cheaper, and
/// otherwise UTF-16, which every client supports.
fn negotiate_encoding(capabilities: &ClientCapabilities) -> Encoding {
    let offered = capabilities
        .general
        .as_ref()
        .and_then(|general| general.position_encodings.as_ref());
    if offered.is_some_and(|encodings| encodings.contains(&PositionEncodingKind::UTF8)) {
        Encoding::Utf8
    } else {
        Encoding::Utf16
    }
}

fn capabilities(encoding: Encoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(match encoding {
            Encoding::Utf8 => PositionEncodingKind::UTF8,
            Encoding::Utf16 => PositionEncodingKind::UTF16,
        }),
        text_document_sync: Some(TextDocumentSync::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::Full),
            ..TextDocumentSyncOptions::default()
        })),
        code_action_provider: Some(CodeActionProvider::CodeActionOptions(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::QuickFix, CodeActionKind::new(FIX_ALL)]),
            ..CodeActionOptions::default()
        })),
        ..ServerCapabilities::default()
    }
}

/// An open `pyproject.toml` document.
struct Document {
    path: PathBuf,
    version: i32,
    text: String,
}

/// A lock file read before, with when it was last modified.
struct CachedLock {
    modified: Option<SystemTime>,
    lock: Result<Lock, String>,
}

struct Server {
    encoding: Encoding,
    documents: HashMap<Uri, Document>,
    /// Lock files by path and name, reread when they change.
    locks: HashMap<(PathBuf, String), CachedLock>,
}

impl Server {
    fn request(&mut self, request: Request) -> Response {
        match request.method.as_str() {
            "textDocument/codeAction" => {
                match serde_json::from_value::<CodeActionParams>(request.params) {
                    Ok(params) => {
                        let actions = self.code_actions(&params);
                        Response::new_ok(request.id, actions)
                    }
                    Err(error) => invalid_params(request.id, &error),
                }
            }
            method => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("pyprojx does not handle `{method}`"),
            ),
        }
    }

    /// Handles a notification, returning the messages to send in reply.
    fn notification(&mut self, notification: Notification) -> Vec<Message> {
        match notification.method.as_str() {
            "textDocument/didOpen" => {
                let Ok(params) =
                    serde_json::from_value::<DidOpenTextDocumentParams>(notification.params)
                else {
                    return Vec::new();
                };
                let item = params.text_document;
                let Some(path) = pyproject_path(&item.uri) else {
                    return Vec::new();
                };
                self.documents.insert(
                    item.uri.clone(),
                    Document {
                        path,
                        version: item.version,
                        text: item.text,
                    },
                );
                self.publish(&item.uri).into_iter().collect()
            }
            "textDocument/didChange" => {
                let Ok(params) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(notification.params)
                else {
                    return Vec::new();
                };
                let uri = params.text_document.text_document_identifier.uri;
                let Some(document) = self.documents.get_mut(&uri) else {
                    return Vec::new();
                };
                // With full synchronization, each change is the whole document.
                let Some(TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(
                    change,
                )) = params.content_changes.into_iter().last()
                else {
                    return Vec::new();
                };
                document.text = change.text;
                document.version = params.text_document.version;
                self.publish(&uri).into_iter().collect()
            }
            "textDocument/didClose" => {
                let Ok(params) =
                    serde_json::from_value::<DidCloseTextDocumentParams>(notification.params)
                else {
                    return Vec::new();
                };
                let uri = params.text_document.uri;
                if self.documents.remove(&uri).is_none() {
                    return Vec::new();
                }
                vec![publish_diagnostics(uri, None, Vec::new())]
            }
            _ => Vec::new(),
        }
    }

    /// Checks the document at `uri` and returns the diagnostics to publish.
    fn publish(&mut self, uri: &Uri) -> Option<Message> {
        let checked = self.check(uri)?;
        let document = &self.documents[uri];
        let index = LineIndex::new(&checked.text, self.encoding);
        let diagnostics = checked
            .diagnostics
            .iter()
            .map(|diagnostic| convert::diagnostic(diagnostic, uri, &index))
            .collect();
        Some(publish_diagnostics(
            uri.clone(),
            Some(document.version),
            diagnostics,
        ))
    }

    fn check(&mut self, uri: &Uri) -> Option<Checked> {
        let document = self.documents.get(uri)?;
        let lock = self.lock(&document.path.clone(), &document.text.clone());
        let document = &self.documents[uri];
        Some(pyprojx_core::check_with_lock(
            document.text.clone().into_bytes(),
            lock.as_ref(),
        ))
    }

    /// The lock file for the project at `path`, reading each lock file again
    /// only when it changes.
    fn lock(&mut self, path: &Path, text: &str) -> Option<Lock> {
        let locks = &mut self.locks;
        let found = find_with(path, text, "", &mut |path, kind, name| {
            load(locks, path, kind, name)
        })?;
        // Unreadable lock files are left out; the CLI reports them.
        found.ok()
    }

    fn code_actions(&mut self, params: &CodeActionParams) -> Vec<CodeActionResponse> {
        let uri = &params.text_document.uri;
        let Some(checked) = self.check(uri) else {
            return Vec::new();
        };
        let only = params.context.only.as_ref();
        let wants = |kind: &str| {
            only.is_none_or(|kinds| {
                kinds.iter().any(|wanted| {
                    let wanted = String::from(wanted.clone());
                    kind == wanted || kind.starts_with(&format!("{wanted}."))
                })
            })
        };
        let index = LineIndex::new(&checked.text, self.encoding);
        let requested = index.offset(params.range.start)..index.offset(params.range.end);
        let mut actions = Vec::new();
        if wants("quickfix") {
            for diagnostic in &checked.diagnostics {
                let Some(fix) = &diagnostic.fix else {
                    continue;
                };
                // Actions for diagnostics that touch the requested range.
                let span = &diagnostic.span;
                if span.start > requested.end || requested.start > span.end {
                    continue;
                }
                let edits = fix
                    .edits
                    .iter()
                    .map(|edit| TextEdit {
                        range: index.range(edit.range.clone()),
                        new_text: edit.replacement.clone(),
                    })
                    .collect();
                actions.push(CodeActionResponse::CodeAction(CodeAction {
                    title: format!("pyprojx: {}", fix.title),
                    kind: Some(CodeActionKind::QuickFix),
                    diagnostics: Some(vec![convert::diagnostic(diagnostic, uri, &index)]),
                    is_preferred: Some(fix.applicability == Applicability::Safe),
                    edit: Some(workspace_edit(uri, edits)),
                    ..CodeAction::default()
                }));
            }
        }
        if wants(FIX_ALL) {
            let path = self.documents[uri].path.clone();
            let lock = self.lock(&path, &checked.text);
            let fixed = pyprojx_core::fix::fix(
                checked.text.clone().into_bytes(),
                lock.as_ref(),
                Applicability::Safe,
            );
            if fixed.fixed > 0 {
                let whole = TextEdit {
                    range: index.range(0..checked.text.len()),
                    new_text: fixed.text,
                };
                actions.push(CodeActionResponse::CodeAction(CodeAction {
                    title: "pyprojx: fix all safe problems".to_owned(),
                    kind: Some(CodeActionKind::new(FIX_ALL)),
                    edit: Some(workspace_edit(uri, vec![whole])),
                    ..CodeAction::default()
                }));
            }
        }
        actions
    }
}

/// Reads a lock file, or reuses it if it has not changed since it was read.
fn load(
    locks: &mut HashMap<(PathBuf, String), CachedLock>,
    path: &Path,
    kind: LockKind,
    name: String,
) -> Result<Lock, String> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key = (path.to_path_buf(), name.clone());
    if let Some(cached) = locks.get(&key)
        && cached.modified.is_some()
        && cached.modified == modified
    {
        return cached.lock.clone();
    }
    let lock = std::fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| Lock::parse(kind, name, &text));
    locks.insert(
        key,
        CachedLock {
            modified,
            lock: lock.clone(),
        },
    );
    lock
}

/// The path of a `file:` URI whose file is named `pyproject.toml`.
fn pyproject_path(uri: &Uri) -> Option<PathBuf> {
    let path = uri.to_file_path().ok()?;
    (path.file_name()? == "pyproject.toml").then_some(path)
}

fn workspace_edit(uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(HashMap::from([(uri.clone(), edits)])),
        ..WorkspaceEdit::default()
    }
}

fn publish_diagnostics(
    uri: Uri,
    version: Option<i32>,
    diagnostics: Vec<lsp_types::Diagnostic>,
) -> Message {
    let params = PublishDiagnosticsParams {
        uri,
        version,
        diagnostics,
    };
    Message::Notification(Notification::new(
        "textDocument/publishDiagnostics".to_owned(),
        params,
    ))
}

fn invalid_params(id: RequestId, error: &serde_json::Error) -> Response {
    Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string())
}
