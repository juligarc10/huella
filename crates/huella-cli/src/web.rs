//! `huella review open`: la revisión en una página web local.
//!
//! El servidor solo escucha en 127.0.0.1 y exige en cada petición un token
//! aleatorio que va en la URL: así ninguna otra web abierta en el navegador
//! puede leer la revisión ni marcar pasos.

use std::fmt::Write as _;

use anyhow::{Context, Result};
use huella_review::flow::FlowStep;
use huella_review::{DiffLine, ExternalReason, SessionReview, Step};
use huella_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

const PAGE: &str = include_str!("review_page.html");

#[derive(Clone, Copy, clap::ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Editor {
    Zed,
    Vscode,
}

pub struct Options {
    pub editor: Editor,
    pub port: u16,
    pub open_browser: bool,
}

pub fn serve(store: &Store, review: &SessionReview, plan: &[FlowStep], options: &Options) -> Result<()> {
    let server = Server::http(("127.0.0.1", options.port))
        .map_err(|error| anyhow::anyhow!("no se pudo abrir el servidor local: {error}"))?;
    let port = server.server_addr().to_ip().context("dirección del servidor desconocida")?.port();
    let token = token(&review.session_id);
    let url = format!("http://127.0.0.1:{port}/?t={token}");
    println!("Revisión de la sesión {} en {url}", &review.session_id[..8.min(review.session_id.len())]);
    println!("Ctrl+C para terminar. Lo que marques como revisado se guarda al momento.");
    if options.open_browser {
        open_in_browser(&url);
    }
    for request in server.incoming_requests() {
        if let Err(error) = handle(request, store, review, plan, options, &token) {
            eprintln!("aviso: {error:#}");
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Mark {
    key: String,
    hash: String,
    reviewed: bool,
}

fn handle(
    mut request: Request,
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    options: &Options,
    token: &str,
) -> Result<()> {
    let (path, query) = request.url().split_once('?').unwrap_or((request.url(), ""));
    let path = path.to_owned();
    let authorized = query.split('&').any(|pair| pair == format!("t={token}"));
    if !authorized {
        return Ok(request.respond(Response::from_string("token no válido").with_status_code(403))?);
    }
    match (request.method(), path.as_str()) {
        (Method::Get, "/") => {
            let page = PAGE.replace("__TOKEN__", token);
            Ok(request.respond(Response::from_string(page).with_header(content_type("text/html; charset=utf-8")))?)
        }
        (Method::Get, "/api/review") => {
            let points = huella_review::attention::attention(review, plan);
            let narrations = huella_engine::narration::narrations(store, review, plan, &points)?;
            let mut value = payload(review, plan, &store.review_marks(&review.session_id)?, options.editor);
            value["narrations"] = serde_json::to_value(&narrations)?;
            let body = value.to_string();
            Ok(request.respond(Response::from_string(body).with_header(content_type("application/json")))?)
        }
        (Method::Post, "/api/mark") => {
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body)?;
            let mark: Mark = serde_json::from_str(&body).context("marca con formato inválido")?;
            store.set_review_mark(&review.session_id, &mark.key, &mark.hash, mark.reviewed)?;
            Ok(request.respond(Response::from_string("{}").with_header(content_type("application/json")))?)
        }
        _ => Ok(request.respond(Response::from_string("no encontrado").with_status_code(404))?),
    }
}

/// Todo lo que la página necesita, ya resuelto: cada paso con su diff, sus
/// orígenes en texto y si se revisó con el contenido actual.
pub fn payload(
    review: &SessionReview,
    plan: &[FlowStep],
    marks: &std::collections::HashMap<String, String>,
    editor: Editor,
) -> Value {
    let root = crate::review::common_root(review.files.iter().map(|file| file.path.as_str()));
    let steps: Vec<Value> = plan
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let file = &review.files[step.file];
            let key = step.key(review);
            let hash = step.content_hash();
            let sources: Vec<Value> = step
                .sources
                .iter()
                .map(|&s| match &review.steps[s] {
                    Step::Agent { turn, at, rationale, .. } => {
                        json!({"kind": "agent", "turn": turn, "at": at, "rationale": rationale})
                    }
                    Step::External { reason } => json!({"kind": "external", "reason": match reason {
                        ExternalReason::BetweenEdits => "Cambio entre ediciones del agente (terminal, formateador o a mano)",
                        ExternalReason::Unreconstructable => "Edición que la transcripción no permite reconstruir",
                        ExternalReason::AfterLastEdit => "Cambio posterior a la última edición del agente (terminal o a mano)",
                    }}),
                })
                .collect();
            let lines: Vec<Value> = step
                .lines
                .iter()
                .map(|line| match line {
                    DiffLine::Context(text) => json!({"kind": "context", "text": text}),
                    DiffLine::Removed(text) => json!({"kind": "removed", "text": text}),
                    DiffLine::Added { text, source } => json!({
                        "kind": "added",
                        "text": text,
                        "external": source.is_some_and(|s| matches!(review.steps[s], Step::External { .. })),
                    }),
                })
                .collect();
            json!({
                "index": index,
                "key": key,
                "hash": hash,
                "reviewed": marks.get(&key) == Some(&hash),
                "turn": step.turn,
                "role": step.role,
                "depth": step.depth,
                "symbol": step.symbol.as_ref().map(|s| &s.qualified_name),
                "kind": step.symbol.as_ref().map(|s| s.kind),
                "path": file.path,
                "display_path": file.path.strip_prefix(&root).unwrap_or(&file.path),
                // Sin símbolo (otros cambios) se abre el fichero al principio.
                "line": step.symbol.as_ref().map_or(1, |s| s.start_line),
                "added": step.added(),
                "removed": step.removed(),
                "calls": step.calls,
                "sources": sources,
                "lines": lines,
            })
        })
        .collect();
    json!({
        "session": review.session_id,
        "attention": huella_review::attention::attention(review, plan),
        "editor": match editor { Editor::Zed => "zed", Editor::Vscode => "vscode" },
        "turns": review.turns,
        "unreconstructable": review.unreconstructable,
        "steps": steps,
    })
}

fn content_type(value: &str) -> Header {
    Header::from_bytes("Content-Type", value).expect("cabecera válida")
}

/// Token difícil de adivinar para esta ejecución del servidor.
pub(crate) fn token(session: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(session.as_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    hasher.update(&nanos.to_le_bytes());
    let address = &raw const hasher as usize;
    hasher.update(&address.to_le_bytes());
    hasher.finalize().to_hex()[..32].to_owned()
}

/// Codifica un valor para la query de una URL.
pub(crate) fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

pub(crate) fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(error) = std::process::Command::new(opener)
        .arg(url)
        // Desde el servidor del editor, la salida estándar es el protocolo.
        .stdout(std::process::Stdio::null())
        .spawn()
    {
        eprintln!("No se pudo abrir el navegador ({error}); abre la URL a mano.");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    #[test]
    fn payload_resolves_sources_and_review_marks() {
        let edit = FileEdit {
            origin: EditOrigin { agent: "t", session_id: "s".into(), event_id: "e".into(), timestamp: None, cwd: None },
            path: PathBuf::from("/r/a.ts"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known("export function f() {\n  return 1;\n}\n".into()),
            user_modified: false,
            context: EditContext { prompt: Some("crea f".into()), rationale: Some("porque sí".into()) },
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let mut marks = std::collections::HashMap::new();
        marks.insert(plan[0].key(&review), plan[0].content_hash());
        let value = payload(&review, &plan, &marks, Editor::Zed);
        let step = &value["steps"][0];
        assert_eq!(step["symbol"], "f");
        assert_eq!(step["reviewed"], true);
        assert_eq!(step["sources"][0]["rationale"], "porque sí");
        assert_eq!(step["lines"][0]["kind"], "added");
        assert_eq!(value["turns"][0]["prompt"], "crea f");

        marks.insert(plan[0].key(&review), "otro-hash".into());
        assert_eq!(payload(&review, &plan, &marks, Editor::Zed)["steps"][0]["reviewed"], false, "el contenido cambió");
    }

    #[test]
    fn tokens_differ_between_runs() {
        assert_ne!(token("s"), token("s"));
        assert_eq!(token("s").len(), 32);
    }
}
