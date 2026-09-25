//! Tests of the language server over an in-memory connection, playing the
//! client's part.

use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A running server and the client end of its connection.
struct Client {
    connection: Connection,
    server: Option<JoinHandle<Result<(), String>>>,
    next_id: i32,
    dir: TempDir,
}

impl Client {
    /// Starts a server and initializes it with the client capabilities given.
    fn start(capabilities: Value) -> (Self, Value) {
        let (server, connection) = Connection::memory();
        let server = std::thread::spawn(move || pyprojx_server::serve(&server));
        let mut client = Self {
            connection,
            server: Some(server),
            next_id: 0,
            dir: TempDir::new().unwrap(),
        };
        let result = client.request(
            "initialize",
            json!({ "processId": null, "rootUri": null, "capabilities": capabilities }),
        );
        client.notify("initialized", json!({}));
        (client, result)
    }

    fn uri(&self, path: &str) -> String {
        let path = self.dir.path().join(path);
        lsp_types::Uri::from_file_path(path).unwrap().to_string()
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn notify(&self, method: &str, params: Value) {
        let notification = Notification::new(method.to_owned(), params);
        self.connection
            .sender
            .send(Message::Notification(notification))
            .unwrap();
    }

    /// Sends a request and returns its result, skipping notifications.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        let request = Request::new(id.clone(), method.to_owned(), params);
        self.connection
            .sender
            .send(Message::Request(request))
            .unwrap();
        loop {
            if let Message::Response(response) = self.receive() {
                assert_eq!(response.id, id);
                return response
                    .response_result
                    .unwrap_or_else(|error| panic!("{method} failed: {error:?}"));
            }
        }
    }

    fn receive(&self) -> Message {
        self.connection
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the server replies")
    }

    /// The next diagnostics the server publishes.
    fn diagnostics(&self) -> Value {
        loop {
            if let Message::Notification(notification) = self.receive()
                && notification.method == "textDocument/publishDiagnostics"
            {
                return notification.params;
            }
        }
    }

    fn open(&self, path: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": self.uri(path), "languageId": "toml", "version": 1, "text": text
            }}),
        );
    }

    /// Replaces the project's paths in `value` so that snapshots are stable.
    fn redact(&self, value: &Value) -> String {
        let text = serde_json::to_string_pretty(value).unwrap();
        let root = lsp_types::Uri::from_file_path(self.dir.path())
            .unwrap()
            .to_string();
        text.replace(&root, "file:///[project]")
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        let server = self.server.take().unwrap();
        assert_eq!(server.join().unwrap(), Ok(()));
    }
}

const PROJECT: &str = "[project]\nname = \"démo\"\nversion = \"0.1.0\"\ndescripton = \"𝄞\"\n\n[dependency-groups]\ndev = [\"ruff==0.16.8\"]\n\n[tool.ruff]\nselect = [\"PLR1701\"]\n";

fn range(line: u32, character: u32) -> Value {
    let position = json!({ "line": line, "character": character });
    json!({ "start": position, "end": position })
}

#[test]
fn negotiates_utf8_positions_when_offered() {
    let (_client, result) = Client::start(json!({}));
    assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
    let (_client, result) =
        Client::start(json!({ "general": { "positionEncodings": ["utf-8", "utf-16"] } }));
    assert_eq!(result["capabilities"]["positionEncoding"], "utf-8");
    insta::assert_snapshot!(serde_json::to_string_pretty(&result).unwrap());
}

#[test]
fn publishes_diagnostics_for_pyproject_toml() {
    let (client, _) = Client::start(json!({}));
    client.open("pyproject.toml", PROJECT);
    insta::assert_snapshot!(client.redact(&client.diagnostics()));
}

#[test]
fn counts_utf8_positions_when_negotiated() {
    let (client, _) = Client::start(json!({ "general": { "positionEncodings": ["utf-8"] } }));
    client.open(
        "pyproject.toml",
        "[project]\nname = \"d\"\nversion = \"1\"\ndescription = 𝄞\n",
    );
    let published = client.diagnostics();
    // The syntax error is after `𝄞`, 4 bytes but 2 UTF-16 units.
    assert_eq!(published["diagnostics"][0]["code"], "invalid-toml");
    let start = &published["diagnostics"][0]["range"]["start"];
    assert_eq!(start["character"], 14, "{published}");
}

#[test]
fn ignores_other_toml_files() {
    let (mut client, _) = Client::start(json!({}));
    client.open("Cargo.toml", "[package]\nnme = 1\n");
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": client.uri("Cargo.toml") },
                "range": range(1, 0), "context": { "diagnostics": [] } }),
    );
    assert_eq!(actions, json!([]));
    // The server published nothing for it before replying.
    assert!(client.connection.receiver.try_recv().is_err());
}

#[test]
fn republishes_on_change_and_clears_on_close() {
    let (client, _) = Client::start(json!({}));
    client.open("pyproject.toml", PROJECT);
    assert_eq!(
        client.diagnostics()["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": client.uri("pyproject.toml"), "version": 2 },
                "contentChanges": [{ "text": "[project]\nname = \"demo\"\nversion = \"1\"\n" }] }),
    );
    let published = client.diagnostics();
    assert_eq!(published["version"], 2);
    assert_eq!(published["diagnostics"], json!([]));
    client.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": client.uri("pyproject.toml") } }),
    );
    let published = client.diagnostics();
    assert_eq!(published["version"], Value::Null);
    assert_eq!(published["diagnostics"], json!([]));
}

#[test]
fn offers_quick_fixes_and_fix_all() {
    let (mut client, _) = Client::start(json!({}));
    client.open("pyproject.toml", PROJECT);
    client.diagnostics();
    let uri = client.uri("pyproject.toml");
    // On the typo's line: its unsafe fix, which is not preferred, and fix-all.
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range(3, 2),
                "context": { "diagnostics": [] } }),
    );
    insta::assert_snapshot!(client.redact(&actions));
    // Only fix-all, as editors ask for it on save.
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range(0, 0),
                "context": { "diagnostics": [], "only": ["source.fixAll"] } }),
    );
    let actions = actions.as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["kind"], "source.fixAll.pyprojx");
    let edit = &actions[0]["edit"]["changes"][&uri][0]["newText"];
    assert!(
        edit.as_str()
            .unwrap()
            .contains("lint.select = [\"SIM101\"]"),
        "{edit}"
    );
    assert!(
        edit.as_str().unwrap().contains("descripton"),
        "unsafe fixes are left out"
    );
}

#[test]
fn checks_against_locked_versions() {
    let (client, _) = Client::start(json!({}));
    client.write(
        "uv.lock",
        "version = 1\n[[package]]\nname = \"ruff\"\nversion = \"0.5.0\"\n",
    );
    client.open(
        "pyproject.toml",
        "[dependency-groups]\ndev = [\"ruff>=0.5\"]\n[tool.ruff.analyze]\ndetect-string-imports = true\n",
    );
    let published = client.diagnostics();
    let message = published["diagnostics"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("note: checked against Ruff 0.5.0, locked in uv.lock"),
        "{message}"
    );
    assert_eq!(published["diagnostics"][0]["severity"], 1);
    // The lock is reread when it changes.
    client.write(
        "uv.lock",
        "version = 1\n[[package]]\nname = \"ruff\"\nversion = \"0.16.8\"\n",
    );
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(client.dir.path().join("uv.lock"))
        .unwrap()
        .set_modified(later)
        .unwrap();
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": client.uri("pyproject.toml"), "version": 2 },
                "contentChanges": [{ "text": "[dependency-groups]\ndev = [\"ruff>=0.5\"]\n[tool.ruff.analyze]\ndetect-string-imports = true\n" }] }),
    );
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
}
