//! Tests del protocolo: un cliente en memoria hace de editor.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

use serde_json::{Value, json};

use super::*;

const V1: &str = "export class UserService {\n  register(email: string) {\n    return save(email);\n  }\n}\n";
const V2: &str = "export class UserService {\n  register(email: string) {\n    return save(email.trim());\n  }\n}\n";

struct Fixture {
    dir: PathBuf,
    db: PathBuf,
    projects: PathBuf,
    source: PathBuf,
}

impl Fixture {
    /// Transcripción con dos ediciones de `user.ts`: crearlo y recortar el email.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("huella-lsp-{name}-{}", std::process::id()));
        let projects = dir.join("projects");
        std::fs::create_dir_all(projects.join("-repo")).unwrap();
        let source = dir.join("repo/user.ts");
        let lines = [
            json!({"type": "user", "message": {"role": "user", "content": "Crea el servicio de usuarios"}}),
            json!({"type": "user", "uuid": "e1", "sessionId": "s1", "timestamp": "2026-10-20T10:00:00Z",
                   "toolUseResult": {"type": "create", "filePath": source, "content": V1}}),
            json!({"type": "user", "message": {"role": "user", "content": "Recorta los espacios del email"}}),
            json!({"type": "assistant", "message": {"role": "assistant",
                   "content": [{"type": "text", "text": "Aplico trim() antes de guardar."}]}}),
            json!({"type": "user", "uuid": "e2", "sessionId": "s2", "timestamp": "2026-10-25T10:00:00Z",
                   "toolUseResult": {"filePath": source, "oldString": "save(email)", "newString": "save(email.trim())",
                                     "originalFile": V1}}),
        ];
        let transcript: String = lines.iter().fold(String::new(), |mut out, line| {
            let _ = writeln!(out, "{line}");
            out
        });
        std::fs::write(projects.join("-repo/s.jsonl"), transcript).unwrap();
        Self { db: dir.join("huella.db"), projects, source, dir }
    }

    fn index(&self) {
        let mut store = Store::open(&self.db).unwrap();
        huella_engine::index(&mut store, &self.projects).unwrap();
    }

    fn uri(&self) -> Value {
        json!(format!("file://{}", self.source.display().to_string().replace(' ', "%20")))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Editor simulado sobre una conexión en memoria.
struct Client {
    connection: Connection,
    server: Option<thread::JoinHandle<Result<()>>>,
    next_id: i32,
}

impl Client {
    fn start(config: Config, refresh_support: bool) -> Self {
        let (server_side, client_side) = Connection::memory();
        let server = thread::spawn(move || serve(&server_side, &config));
        let mut client = Self { connection: client_side, server: Some(server), next_id: 0 };
        let capabilities = json!({"workspace": {"codeLens": {"refreshSupport": refresh_support}}});
        let result =
            client.request("initialize", json!({"processId": null, "rootUri": null, "capabilities": capabilities}));
        assert_eq!(result["serverInfo"]["name"], "huella");
        assert_eq!(result["capabilities"]["hoverProvider"], true);
        assert_eq!(result["capabilities"]["textDocumentSync"], 1, "sincronización completa");
        client.notify("initialized", json!({}));
        client
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        let request = Request::new(id.clone(), method.into(), params);
        self.connection.sender.send(Message::Request(request)).unwrap();
        loop {
            match self.connection.receiver.recv_timeout(Duration::from_secs(10)).expect("el servidor no respondió") {
                Message::Response(response) if response.id == id => {
                    return response.response_result.unwrap_or_else(|error| panic!("error en {method}: {error:?}"));
                }
                _ => {}
            }
        }
    }

    fn notify(&self, method: &str, params: Value) {
        self.connection.sender.send(Message::Notification(Notification::new(method.into(), params))).unwrap();
    }

    fn open(&self, uri: &Value, text: &str) {
        let document = json!({"uri": uri, "languageId": "typescript", "version": 1, "text": text});
        self.notify("textDocument/didOpen", json!({"textDocument": document}));
    }

    fn hover(&mut self, uri: &Value, line: u32) -> Value {
        self.request(
            "textDocument/hover",
            json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": 4}}),
        )
    }

    /// Espera la siguiente notificación `method` e ignora el resto de mensajes.
    fn next_notification(&self, method: &str) -> Value {
        loop {
            let message =
                self.connection.receiver.recv_timeout(Duration::from_secs(10)).expect("no llegó la notificación");
            if let Message::Notification(notification) = message
                && notification.method == method
            {
                return notification.params;
            }
        }
    }

    fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        self.server.take().unwrap().join().unwrap().unwrap();
    }
}

fn config(fixture: &Fixture, projects: Option<&Path>) -> Config {
    Config { db: fixture.db.clone(), projects: projects.map(Path::to_path_buf), index_every: Duration::from_secs(3600) }
}

#[test]
fn hover_shows_prompts_and_rationale_of_the_symbol_under_the_cursor() {
    let fixture = Fixture::new("hover");
    fixture.index();
    let mut client = Client::start(config(&fixture, None), false);
    let uri = fixture.uri();
    client.open(&uri, V2);

    let hover = client.hover(&uri, 2);
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("`UserService.register` · 2 ediciones en 2 sesiones"), "{text}");
    assert!(text.contains("_«Recorta los espacios del email»_\n> Aplico trim() antes de guardar."), "{text}");
    assert!(!text.contains("ha cambiado"));
    assert_eq!(hover["range"], json!({"start": {"line": 1, "character": 0}, "end": {"line": 4, "character": 0}}));

    assert_eq!(client.hover(&uri, 5), Value::Null, "fuera de símbolos no hay hover");
    client.shutdown();
}

#[test]
fn hover_uses_the_unsaved_buffer_and_warns_about_drift() {
    let fixture = Fixture::new("drift");
    fixture.index();
    let mut client = Client::start(config(&fixture, None), false);
    let uri = fixture.uri();
    client.open(&uri, V2);
    let edited = V2.replace("trim()", "toLowerCase()");
    client.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": edited}]}),
    );
    let text = client.hover(&uri, 2)["contents"]["value"].as_str().unwrap().to_owned();
    assert!(text.contains("⚠️ El código ha cambiado"), "{text}");
    client.shutdown();
}

#[test]
fn code_lenses_summarize_activity_per_symbol() {
    let fixture = Fixture::new("lens");
    fixture.index();
    let mut client = Client::start(config(&fixture, None), false);
    let uri = fixture.uri();
    client.open(&uri, V2);
    let lenses = client.request("textDocument/codeLens", json!({"textDocument": {"uri": uri}}));
    let titles: Vec<(u64, &str)> = lenses
        .as_array()
        .unwrap()
        .iter()
        .map(|lens| (lens["range"]["start"]["line"].as_u64().unwrap(), lens["command"]["title"].as_str().unwrap()))
        .collect();
    assert_eq!(
        titles,
        vec![
            (0, "Huella · 1 edición · 1 sesión · última 2026-10-20"),
            (1, "Huella · 2 ediciones · 2 sesiones · última 2026-10-25"),
        ]
    );
    client.shutdown();
}

#[test]
fn background_indexing_asks_the_editor_to_refresh_lenses() {
    let fixture = Fixture::new("refresh");
    let mut client = Client::start(config(&fixture, Some(&fixture.projects)), true);
    let refresh = loop {
        let message = client.connection.receiver.recv_timeout(Duration::from_secs(10)).expect("no llegó el refresco");
        if let Message::Request(request) = message {
            break request;
        }
    };
    assert_eq!(refresh.method, "workspace/codeLens/refresh");
    client.connection.sender.send(Message::Response(Response::new_ok(refresh.id, Value::Null))).unwrap();

    let uri = fixture.uri();
    client.open(&uri, V2);
    assert!(client.hover(&uri, 2)["contents"]["value"].as_str().unwrap().contains("2 ediciones"));
    client.shutdown();
}

#[test]
fn hover_p95_stays_under_50_ms_on_a_large_file() {
    let fixture = Fixture::new("perf");
    fixture.index();
    let methods: String = (0..300).fold(String::new(), |mut out, i| {
        let _ = write!(
            out,
            "  method{i}(value: number) {{\n    const doubled = value * 2;\n    return doubled + {i};\n  }}\n"
        );
        out
    });
    let large = V2.replace("export class UserService {\n", &format!("export class UserService {{\n{methods}"));
    let register_line = u32::try_from(large.lines().position(|line| line.contains("register(")).unwrap()).unwrap();

    let mut client = Client::start(config(&fixture, None), false);
    let uri = fixture.uri();
    client.open(&uri, &large);
    let mut timings: Vec<Duration> = (0..60)
        .map(|_| {
            let started = Instant::now();
            assert_ne!(client.hover(&uri, register_line + 1), Value::Null);
            started.elapsed()
        })
        .collect();
    timings.sort();
    let p95 = timings[timings.len() * 95 / 100];
    eprintln!("p95 del hover: {p95:?} · mediana {:?} · {} líneas", timings[timings.len() / 2], large.lines().count());
    assert!(p95 < Duration::from_millis(50), "p95 del hover: {p95:?} sobre {} líneas", large.lines().count());
    client.shutdown();
}

#[test]
fn the_active_review_shows_pending_steps_that_a_code_action_marks_as_reviewed() {
    let fixture = Fixture::new("review");
    fixture.index();
    Store::open(&fixture.db).unwrap().set_active_review(Some("s2")).unwrap();
    let mut client = Client::start(config(&fixture, Some(&fixture.projects)), false);
    let uri = fixture.uri();
    client.open(&uri, V2);

    let published = client.next_notification("textDocument/publishDiagnostics");
    let diagnostics = published["diagnostics"].as_array().unwrap().clone();
    assert_eq!(diagnostics.len(), 1, "{published}");
    let message = diagnostics[0]["message"].as_str().unwrap();
    assert!(message.contains("paso 1 de 1 pendiente: UserService.register"), "{message}");
    assert!(message.contains("Aplico trim() antes de guardar."), "{message}");
    assert_eq!(diagnostics[0]["range"]["start"]["line"], 1);

    let actions = client.request(
        "textDocument/codeAction",
        json!({"textDocument": {"uri": uri}, "range": diagnostics[0]["range"], "context": {"diagnostics": diagnostics}}),
    );
    let command = &actions[0]["command"];
    assert_eq!(command["command"], "huella.markReviewed");
    client
        .request("workspace/executeCommand", json!({"command": command["command"], "arguments": command["arguments"]}));
    let republished = client.next_notification("textDocument/publishDiagnostics");
    assert_eq!(republished["diagnostics"], json!([]), "revisado: ya no está pendiente");
    client.shutdown();
}

#[test]
fn without_an_active_review_there_are_no_diagnostics() {
    let fixture = Fixture::new("noreview");
    fixture.index();
    let client = Client::start(config(&fixture, Some(&fixture.projects)), false);
    client.open(&fixture.uri(), V2);
    assert_eq!(client.next_notification("textDocument/publishDiagnostics")["diagnostics"], json!([]));
    client.shutdown();
}

#[test]
fn file_uris_are_decoded_into_absolute_paths() {
    assert_eq!(uri_to_path("file:///Users/ana/mi%20repo/%C3%B1.ts"), Some(PathBuf::from("/Users/ana/mi repo/ñ.ts")));
    assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    assert_eq!(uri_to_path("file://relative/a.ts"), None);
}
