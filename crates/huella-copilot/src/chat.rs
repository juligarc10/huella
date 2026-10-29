//! Chat y modo agente de Copilot en VS Code.
//!
//! Cada conversación es un fichero en
//! `…/Code/User/workspaceStorage/<hash>/chatSessions/`: un `.json` con el
//! estado entero o, en versiones recientes, un `.jsonl` con las operaciones
//! que lo construyen (`kind` 0 = estado inicial, 1 = asignar en una ruta,
//! 2 = añadir a una lista). Ambos se leen enteros cada vez que cambian: VS
//! Code los reescribe y no tiene sentido leer solo el final.
//!
//! Las ediciones son los `textEditGroup` de cada respuesta: rangos del
//! fichero sustituidos por texto. Si el fichero lo creó la propia sesión
//! (`copilot_createFile`), aplicarlos sobre un fichero vacío da su contenido
//! exacto, y las ediciones siguientes de ese fichero se aplican sobre él: se
//! conocen el antes y el después. De los demás ficheros se conserva el texto
//! insertado.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use huella_core::agent::find_files;
use huella_core::{
    AfterState, Agent, AgentBatch, AgentError, Conversation, EditContext, EditKind, EditOrigin, FileEdit, Speaker,
    TranscriptCursor, Turn,
};
use serde_json::Value;

use crate::{clip, iso_from_millis};

pub const CHAT_AGENT: &str = "copilot";

/// Copilot en VS Code (y sus variantes Insiders y `VSCodium`).
pub struct CopilotChat {
    /// Carpetas `…/User` de cada instalación de VS Code.
    pub user_dirs: Vec<PathBuf>,
}

impl CopilotChat {
    /// Las carpetas de usuario de VS Code de esta máquina.
    #[must_use]
    pub fn detect(home: &Path) -> Self {
        let flavours = ["Code", "Code - Insiders", "VSCodium"];
        let bases = if cfg!(target_os = "macos") {
            vec![home.join("Library/Application Support")]
        } else if cfg!(target_os = "windows") {
            vec![std::env::var_os("APPDATA").map_or_else(|| home.join("AppData/Roaming"), PathBuf::from)]
        } else {
            vec![home.join(".config")]
        };
        let user_dirs = bases.iter().flat_map(|base| flavours.iter().map(move |f| base.join(f).join("User"))).collect();
        Self { user_dirs }
    }
}

impl Agent for CopilotChat {
    fn name(&self) -> &'static str {
        CHAT_AGENT
    }

    fn sessions(&self) -> Result<Vec<PathBuf>, AgentError> {
        let mut found = Vec::new();
        for user in &self.user_dirs {
            let is_session = |path: &Path| {
                let in_chat_dir = path
                    .parent()
                    .and_then(|dir| dir.file_name())
                    .is_some_and(|name| name == "chatSessions" || name == "emptyWindowChatSessions");
                let extension = path.extension().and_then(|e| e.to_str());
                in_chat_dir && matches!(extension, Some("json" | "jsonl"))
            };
            found.extend(find_files(CHAT_AGENT, &user.join("workspaceStorage"), is_session)?);
            found.extend(find_files(CHAT_AGENT, &user.join("globalStorage/emptyWindowChatSessions"), is_session)?);
        }
        found.sort();
        Ok(found)
    }

    fn read_from(&self, path: &Path, _from: &TranscriptCursor) -> Result<AgentBatch, AgentError> {
        let state = load(path)?;
        let size = std::fs::metadata(path).map_err(|error| AgentError::new(CHAT_AGENT, path, &error))?.len();
        Ok(AgentBatch {
            edits: edits(&state, path, workspace_folder(path).as_deref()),
            resume: TranscriptCursor { offset: size, ..TranscriptCursor::default() },
        })
    }

    fn conversations(&self, path: &Path) -> Result<Vec<Conversation>, AgentError> {
        let state = load(path)?;
        let mut turns = Vec::new();
        for request in requests(&state) {
            let timestamp = request["timestamp"].as_i64().map(iso_from_millis);
            if let Some(text) = request["message"]["text"].as_str().filter(|t| !t.trim().is_empty()) {
                turns.push(Turn { speaker: Speaker::Person, text: text.to_owned(), timestamp: timestamp.clone() });
            }
            let answer: Vec<&str> = response(request).filter_map(markdown).collect();
            if !answer.is_empty() {
                turns.push(Turn { speaker: Speaker::Agent, text: answer.join("\n"), timestamp });
            }
        }
        if turns.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![Conversation { session_id: session_id(&state, path), cwd: workspace_folder(path), turns }])
    }
}

/// El estado de la conversación, de un `.json` o reproduciendo un `.jsonl`.
fn load(path: &Path) -> Result<Value, AgentError> {
    let text = std::fs::read_to_string(path).map_err(|error| AgentError::new(CHAT_AGENT, path, &error))?;
    if path.extension().is_some_and(|e| e == "jsonl") {
        return Ok(replay(&text));
    }
    serde_json::from_str(&text).map_err(|error| AgentError::new(CHAT_AGENT, path, &error))
}

/// Aplica las operaciones de un `.jsonl`. Las líneas que no se entienden se
/// saltan: es mejor perder una edición que la conversación entera.
fn replay(text: &str) -> Value {
    let mut state = Value::Null;
    for operation in text.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        let value = operation["v"].clone();
        match operation["kind"].as_i64() {
            Some(0) => state = value,
            Some(1) => {
                if let Some(slot) = slot(&mut state, &operation["k"]) {
                    *slot = value;
                }
            }
            Some(2) => {
                if let Some(list) = slot(&mut state, &operation["k"]).and_then(Value::as_array_mut) {
                    if let Some(start) = operation["i"].as_u64().and_then(|i| usize::try_from(i).ok()) {
                        list.truncate(start);
                    }
                    list.extend(value.as_array().cloned().unwrap_or_default());
                }
            }
            _ => {}
        }
    }
    state
}

/// El valor en la ruta `k` (`["requests", 3, "response"]`), creando los
/// objetos que falten.
fn slot<'a>(state: &'a mut Value, keys: &Value) -> Option<&'a mut Value> {
    let mut current = state;
    for key in keys.as_array()? {
        current = match key {
            Value::String(name) => {
                if !current.is_object() {
                    *current = Value::Object(serde_json::Map::new());
                }
                current.as_object_mut()?.entry(name.clone()).or_insert(Value::Null)
            }
            Value::Number(index) => current.as_array_mut()?.get_mut(usize::try_from(index.as_u64()?).ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn requests(state: &Value) -> impl Iterator<Item = &Value> {
    state["requests"].as_array().into_iter().flatten()
}

fn response(request: &Value) -> impl Iterator<Item = &Value> {
    request["response"].as_array().into_iter().flatten()
}

/// El texto de una parte de la respuesta, si es texto del modelo.
fn markdown(item: &Value) -> Option<&str> {
    let is_text = item.get("kind").is_none_or(|kind| kind == "markdownContent");
    is_text.then(|| item["value"].as_str()).flatten().filter(|text| !text.trim().is_empty())
}

fn session_id(state: &Value, path: &Path) -> String {
    state["sessionId"]
        .as_str()
        .map_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), str::to_owned)
}

/// La carpeta abierta en VS Code: `workspaceStorage/<hash>/workspace.json`.
fn workspace_folder(session: &Path) -> Option<PathBuf> {
    let file = session.parent()?.parent()?.join("workspace.json");
    let workspace: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    file_uri_to_path(workspace["folder"].as_str()?)
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?, 16)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

fn edits(state: &Value, path: &Path, cwd: Option<&Path>) -> Vec<FileEdit> {
    let session = session_id(state, path);
    let mut found = Vec::new();
    // Contenido conocido de los ficheros que creó la sesión, tras cada edición.
    let mut known: HashMap<String, String> = HashMap::new();
    for (request_index, request) in requests(state).enumerate() {
        let request_id = request["requestId"].as_str().map_or_else(|| format!("r{request_index}"), str::to_owned);
        let timestamp = request["timestamp"].as_i64().map(iso_from_millis);
        let prompt = request["message"]["text"].as_str().map(clip).filter(|p| !p.is_empty());
        let mut rationale: Option<String> = None;
        let mut created: HashSet<String> = HashSet::new();
        for (item_index, item) in response(request).enumerate() {
            if let Some(text) = markdown(item) {
                rationale = Some(clip(text));
                continue;
            }
            match item["kind"].as_str() {
                Some("toolInvocationSerialized") if item["toolId"] == "copilot_createFile" => {
                    created.extend(invocation_paths(item));
                }
                Some("textEditGroup") => {
                    let Some(file) = item["uri"]["fsPath"].as_str().or_else(|| item["uri"]["path"].as_str()) else {
                        continue;
                    };
                    let pieces = edit_pieces(item);
                    let (kind, before, after) = if created.remove(file) {
                        let content = apply(String::new(), &pieces);
                        known.insert(file.to_owned(), content.clone());
                        (EditKind::Create, None, AfterState::Known(content))
                    } else if let Some(previous) = known.get(file).cloned() {
                        let content = apply(previous.clone(), &pieces);
                        known.insert(file.to_owned(), content.clone());
                        (EditKind::Replace, Some(previous), AfterState::Known(content))
                    } else {
                        let inserted: String = pieces.iter().map(|piece| piece.text.as_str()).collect();
                        if inserted.trim().is_empty() {
                            continue;
                        }
                        (EditKind::Replace, None, AfterState::FragmentOnly(inserted.trim_matches('\n').to_owned()))
                    };
                    found.push(FileEdit {
                        origin: EditOrigin {
                            agent: CHAT_AGENT,
                            session_id: session.clone(),
                            event_id: format!("{request_id}:{item_index}"),
                            timestamp: timestamp.clone(),
                            cwd: cwd.map(Path::to_path_buf),
                        },
                        path: PathBuf::from(file),
                        kind,
                        before,
                        after,
                        user_modified: false,
                        context: EditContext { prompt: prompt.clone(), rationale: rationale.clone() },
                    });
                }
                _ => {}
            }
        }
    }
    found
}

/// Rutas de los ficheros que nombra una invocación de herramienta.
fn invocation_paths(item: &Value) -> Vec<String> {
    ["invocationMessage", "pastTenseMessage"]
        .iter()
        .filter_map(|key| item[key]["uris"].as_object())
        .flat_map(|uris| uris.values().filter_map(|uri| uri["path"].as_str().map(str::to_owned)))
        .collect()
}

/// Un rango (líneas y columnas desde 1, fin exclusivo) sustituido por texto.
struct Piece {
    text: String,
    start: (usize, usize),
    end: (usize, usize),
}

fn edit_pieces(group: &Value) -> Vec<Piece> {
    let position = |range: &Value, line: &str, column: &str| {
        let get = |key: &str| range[key].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(1);
        (get(line), get(column))
    };
    group["edits"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|batch| batch.as_array().into_iter().flatten())
        .map(|edit| Piece {
            text: edit["text"].as_str().unwrap_or_default().to_owned(),
            start: position(&edit["range"], "startLineNumber", "startColumn"),
            end: position(&edit["range"], "endLineNumber", "endColumn"),
        })
        .collect()
}

/// Aplica las sustituciones, en orden, sobre `text`. Las columnas de VS Code
/// cuentan unidades UTF-16; se toman como caracteres, que coincide salvo con
/// emojis y similares.
fn apply(mut text: String, pieces: &[Piece]) -> String {
    for piece in pieces {
        let start = offset(&text, piece.start);
        let end = offset(&text, piece.end).max(start);
        text.replace_range(start..end, &piece.text);
    }
    text
}

/// Byte de la posición (línea, columna) desde 1, recortada al final del texto.
fn offset(text: &str, (line, column): (usize, usize)) -> usize {
    let mut line_start = 0;
    for _ in 1..line {
        match text[line_start..].find('\n') {
            Some(newline) => line_start += newline + 1,
            None => return text.len(),
        }
    }
    let line_end = text[line_start..].find('\n').map_or(text.len(), |n| line_start + n);
    text[line_start..line_end]
        .char_indices()
        .nth(column.saturating_sub(1))
        .map_or(line_end, |(index, _)| line_start + index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Línea y columna, desde 1.
    type Position = (usize, usize);

    fn group(path: &str, edits: &[(&str, Position, Position)]) -> Value {
        let edits: Vec<Value> = edits
            .iter()
            .map(|(text, start, end)| {
                serde_json::json!([{"text": text, "range": {"startLineNumber": start.0, "startColumn": start.1,
                                                           "endLineNumber": end.0, "endColumn": end.1}}])
            })
            .collect();
        serde_json::json!({"kind": "textEditGroup", "uri": {"fsPath": path, "path": path}, "edits": edits})
    }

    fn session(response: &[Value]) -> Value {
        serde_json::json!({
            "sessionId": "s1",
            "requests": [{
                "requestId": "request_1",
                "timestamp": 1_765_567_277_197_i64,
                "message": {"text": "crea el servicio y valida el email"},
                "response": response,
            }]
        })
    }

    #[test]
    fn a_created_file_is_rebuilt_exactly_from_its_streamed_lines() {
        let created = serde_json::json!({"kind": "toolInvocationSerialized", "toolId": "copilot_createFile",
            "invocationMessage": {"uris": {"file:///repo/user.ts": {"path": "/repo/user.ts"}}}});
        let state = session(&[
            serde_json::json!({"value": "Creo el servicio."}),
            created,
            group(
                "/repo/user.ts",
                &[("export class A {", (1, 1), (1, 1)), ("\n  run() {}", (2, 1), (2, 1)), ("\n}", (3, 1), (3, 1))],
            ),
        ]);
        let found = edits(&state, Path::new("/x/s1.json"), Some(Path::new("/repo")));
        assert_eq!(found.len(), 1);
        let edit = &found[0];
        assert_eq!(edit.kind, EditKind::Create);
        assert_eq!(edit.after, AfterState::Known("export class A {\n  run() {}\n}".into()));
        assert_eq!(edit.context.prompt.as_deref(), Some("crea el servicio y valida el email"));
        assert_eq!(edit.context.rationale.as_deref(), Some("Creo el servicio."));
        assert_eq!(edit.origin.timestamp.as_deref(), Some("2025-12-12T19:21:17.197Z"));
        assert_eq!(edit.origin.event_id, "request_1:2");
    }

    #[test]
    fn an_edit_to_an_existing_file_keeps_the_inserted_text() {
        let state = session(&[
            serde_json::json!({"value": "Añado la validación."}),
            group("/repo/user.ts", &[("  if (!email) throw new Error();\n", (3, 1), (3, 1))]),
            group("/repo/user.ts", &[("", (5, 1), (7, 1))]),
        ]);
        let found = edits(&state, Path::new("/x/s1.json"), None);
        assert_eq!(found.len(), 1, "un borrado puro no deja nada que anclar");
        assert_eq!(found[0].after, AfterState::FragmentOnly("  if (!email) throw new Error();".into()));
        assert_eq!(found[0].kind, EditKind::Replace);
    }

    #[test]
    fn later_edits_of_a_file_the_session_created_are_known_exactly() {
        let created = serde_json::json!({"kind": "toolInvocationSerialized", "toolId": "copilot_createFile",
            "invocationMessage": {"uris": {"file:///repo/a.py": {"path": "/repo/a.py"}}}});
        let state = session(&[
            created,
            group("/repo/a.py", &[("def f():\n    return 1\n", (1, 1), (1, 1))]),
            group("/repo/a.py", &[("2", (2, 12), (2, 13))]),
        ]);
        let found = edits(&state, Path::new("/x/s1.json"), None);
        assert_eq!(found[1].before.as_deref(), Some("def f():\n    return 1\n"));
        assert_eq!(found[1].after, AfterState::Known("def f():\n    return 2\n".into()));
    }

    #[test]
    fn jsonl_operations_rebuild_the_same_state() {
        let log = [
            r#"{"kind":0,"v":{"sessionId":"s1","requests":[]}}"#,
            r#"{"kind":2,"k":["requests"],"v":[{"requestId":"r1","message":{"text":"hola"},"response":[]}]}"#,
            r#"{"kind":2,"k":["requests",0,"response"],"v":[{"value":"uno"},{"value":"dos"}]}"#,
            r#"{"kind":2,"k":["requests",0,"response"],"i":1,"v":[{"value":"tres"}]}"#,
            r#"{"kind":1,"k":["customTitle"],"v":"Título"}"#,
            "no es json",
        ]
        .join("\n");
        let state = replay(&log);
        assert_eq!(state["customTitle"], "Título");
        let texts: Vec<&str> = response(&state["requests"][0]).filter_map(markdown).collect();
        assert_eq!(texts, vec!["uno", "tres"]);
    }

    #[test]
    fn workspace_uris_are_decoded() {
        assert_eq!(file_uri_to_path("file:///Users/ana/mi%20repo"), Some(PathBuf::from("/Users/ana/mi repo")));
        assert_eq!(file_uri_to_path("vscode-remote://x"), None);
    }

    #[test]
    fn positions_past_the_end_are_clamped() {
        assert_eq!(offset("ab\ncd", (2, 2)), 4);
        assert_eq!(offset("ab\ncd", (9, 1)), 5);
        assert_eq!(offset("ab", (1, 9)), 2);
    }
}
