//! Actividad de una sesión de Claude Code: cada herramienta que usó el
//! agente, emparejada con su resultado, y lo que pensó entre medias.
//!
//! Lo que se guarda pasa por la redacción de secretos y se recorta: de la
//! salida de una orden basta el final, que es donde está el error o el
//! resumen de los tests.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use huella_core::activity::{head, tail};
use huella_core::redact::redact;
use huella_core::{Activity, ActivityKind, PlanItem};
use serde_json::Value;

use super::{RawEvent, TranscriptError, user_prompt};

const MAX_OUTPUT: usize = 1500;
const MAX_TEXT: usize = 2000;

/// Herramientas que editan ficheros: su resultado es una edición, no evidencia.
const EDIT_TOOLS: [&str; 4] = ["Edit", "MultiEdit", "Write", "NotebookEdit"];

/// Toda la actividad de una transcripción, de todas sus sesiones, en orden.
///
/// # Errors
/// Si el fichero no se puede abrir o leer.
pub fn read_activity(path: &Path) -> Result<Vec<Activity>, TranscriptError> {
    let io_error = |source| TranscriptError::Io { path: path.to_path_buf(), source };
    let reader = BufReader::new(File::open(path).map_err(io_error)?);
    let mut reader_state = ActivityReader::default();
    for line in reader.lines() {
        let line = line.map_err(io_error)?;
        if let Ok(event) = serde_json::from_str::<RawEvent>(&line) {
            reader_state.event(&event);
        }
    }
    Ok(reader_state.activity)
}

#[derive(Default)]
struct ActivityReader {
    activity: Vec<Activity>,
    /// Herramienta de cada `tool_use` pendiente de resultado, y su posición
    /// en `activity` si es una orden (para anotarle el resultado).
    pending: HashMap<String, (String, Option<usize>)>,
}

impl ActivityReader {
    fn event(&mut self, event: &RawEvent) {
        if event.is_meta == Some(true) {
            return;
        }
        let Some(content) = event.message.as_ref().and_then(|message| message.get("content")) else { return };
        let session = event.session_id.clone().unwrap_or_default();
        let at = event.timestamp.clone();
        let push = |activity: &mut Vec<Activity>, kind| {
            activity.push(Activity { session_id: session.clone(), at: at.clone(), kind });
        };
        match event.kind.as_deref() {
            Some("user") => {
                if let Some(prompt) = user_prompt(content) {
                    push(&mut self.activity, ActivityKind::Prompt { text: clean(&prompt, MAX_TEXT) });
                    return;
                }
                for block in content.as_array().into_iter().flatten() {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        self.tool_result(block, event, &session, at.as_ref());
                    }
                }
            }
            Some("assistant") => {
                for block in content.as_array().into_iter().flatten() {
                    match block.get("type").and_then(Value::as_str) {
                        Some("thinking") => {
                            if let Some(text) =
                                block.get("thinking").and_then(Value::as_str).filter(|t| !t.trim().is_empty())
                            {
                                push(
                                    &mut self.activity,
                                    ActivityKind::Reasoning { text: clean(text.trim(), MAX_TEXT) },
                                );
                            }
                        }
                        Some("text") => {
                            if let Some(text) =
                                block.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty())
                            {
                                push(
                                    &mut self.activity,
                                    ActivityKind::Reasoning { text: clean(text.trim(), MAX_TEXT) },
                                );
                            }
                        }
                        Some("tool_use") => self.tool_use(block, &session, at.as_ref()),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn tool_use(&mut self, block: &Value, session: &str, at: Option<&String>) {
        let (Some(id), Some(name)) =
            (block.get("id").and_then(Value::as_str), block.get("name").and_then(Value::as_str))
        else {
            return;
        };
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        let field = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
        let kind = match name {
            "Read" | "NotebookRead" => {
                field("file_path").or_else(|| field("notebook_path")).map(|target| ActivityKind::Read { target })
            }
            "WebFetch" => field("url").map(|target| ActivityKind::Read { target }),
            "Grep" | "Glob" => field("pattern").map(|query| ActivityKind::Search {
                query: clean(&query, 300),
                scope: field("path").or_else(|| field("glob")),
            }),
            "WebSearch" => field("query")
                .map(|query| ActivityKind::Search { query: clean(&query, 300), scope: Some("web".to_owned()) }),
            "Bash" => field("command").map(|command| ActivityKind::Command {
                command: clean(&command, MAX_TEXT),
                description: field("description"),
                ok: None,
                output: String::new(),
            }),
            "TodoWrite" => input.get("todos").and_then(Value::as_array).map(|todos| ActivityKind::Plan {
                items: todos
                    .iter()
                    .filter_map(|todo| {
                        Some(PlanItem {
                            text: clean(todo.get("content").and_then(Value::as_str)?, 300),
                            status: todo.get("status").and_then(Value::as_str).unwrap_or("pending").to_owned(),
                        })
                    })
                    .collect(),
            }),
            _ => None,
        };
        let position = kind.map(|kind| {
            self.activity.push(Activity { session_id: session.to_owned(), at: at.cloned(), kind });
            self.activity.len() - 1
        });
        self.pending.insert(id.to_owned(), (name.to_owned(), position));
    }

    fn tool_result(&mut self, block: &Value, event: &RawEvent, session: &str, at: Option<&String>) {
        let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else { return };
        let Some((name, position)) = self.pending.remove(id) else { return };
        let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        if EDIT_TOOLS.contains(&name.as_str()) {
            // El mismo evento que produce la edición: así se enlazan.
            let path = event.tool_use_result.as_ref().and_then(|result| result.get("filePath")).and_then(Value::as_str);
            if let (false, Some(path), Some(event_id)) = (failed, path, event.uuid.as_ref()) {
                self.activity.push(Activity {
                    session_id: session.to_owned(),
                    at: at.cloned(),
                    kind: ActivityKind::Edit { event_id: event_id.clone(), path: path.to_owned() },
                });
            }
            return;
        }
        if let Some(Activity { kind: ActivityKind::Command { ok, output, .. }, .. }) =
            position.and_then(|index| self.activity.get_mut(index))
        {
            *ok = Some(!failed);
            *output = tail(&redact(&result_text(block)), MAX_OUTPUT);
        }
    }
}

/// Texto de un `tool_result`: una cadena o una lista de bloques de texto.
fn result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => {
            parts.iter().filter_map(|part| part.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

fn clean(text: &str, max: usize) -> String {
    head(&redact(text), max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[Value]) -> Vec<ActivityKind> {
        let dir = std::env::temp_dir().join(format!("huella-cc-activity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.jsonl", lines.len()));
        let text: String = lines.iter().map(|line| line.to_string() + "\n").collect();
        std::fs::write(&path, text).unwrap();
        let activity = read_activity(&path).unwrap();
        assert!(activity.iter().all(|a| a.session_id == "s1"));
        activity.into_iter().map(|a| a.kind).collect()
    }

    fn assistant(content: &Value) -> Value {
        serde_json::json!({"type": "assistant", "sessionId": "s1", "uuid": "a", "message": {"role": "assistant", "content": content}})
    }

    fn user(uuid: &str, content: &Value, result: Option<Value>) -> Value {
        let mut line = serde_json::json!({"type": "user", "sessionId": "s1", "uuid": uuid, "message": {"role": "user", "content": content}});
        if let Some(result) = result {
            line["toolUseResult"] = result;
        }
        line
    }

    #[test]
    fn pairs_each_tool_with_its_result_and_marks_edits() {
        let activity = read(&[
            user("p", &serde_json::json!("Arregla el login"), None),
            assistant(&serde_json::json!([
                {"type": "thinking", "thinking": "Primero miro cómo valida."},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/repo/login.ts"}},
                {"type": "tool_use", "id": "t2", "name": "Grep", "input": {"pattern": "validate", "path": "/repo"}},
            ])),
            user("r1", &serde_json::json!([{"type": "tool_result", "tool_use_id": "t1", "content": "…"}]), None),
            assistant(
                &serde_json::json!([{"type": "tool_use", "id": "t3", "name": "Edit", "input": {"file_path": "/repo/login.ts"}}]),
            ),
            user(
                "edit-1",
                &serde_json::json!([{"type": "tool_result", "tool_use_id": "t3", "content": "ok"}]),
                Some(serde_json::json!({"filePath": "/repo/login.ts", "oldString": "a", "newString": "b"})),
            ),
            assistant(&serde_json::json!([{"type": "tool_use", "id": "t4", "name": "Bash",
                "input": {"command": "npm test -- login", "description": "Tests del login"}}])),
            user(
                "r4",
                &serde_json::json!([{"type": "tool_result", "tool_use_id": "t4", "is_error": true,
                    "content": "FAIL login.test.ts\nAPI_KEY=a8f3k2j9d0s7q1w4e6r5 expected 200"}]),
                None,
            ),
        ]);
        assert!(matches!(&activity[0], ActivityKind::Prompt { text } if text == "Arregla el login"));
        assert!(matches!(&activity[1], ActivityKind::Reasoning { .. }));
        assert!(matches!(&activity[2], ActivityKind::Read { target } if target == "/repo/login.ts"));
        assert!(matches!(&activity[3], ActivityKind::Search { query, .. } if query == "validate"));
        assert!(matches!(&activity[4], ActivityKind::Edit { event_id, .. } if event_id == "edit-1"));
        let ActivityKind::Command { command, ok, output, .. } = &activity[5] else { panic!("{:?}", activity[5]) };
        assert_eq!((command.as_str(), *ok), ("npm test -- login", Some(false)));
        assert!(output.contains("FAIL login.test.ts") && !output.contains("a8f3k2j9"), "{output}");
    }

    #[test]
    fn failed_edits_are_not_marked_and_plans_keep_their_status() {
        let activity = read(&[
            assistant(&serde_json::json!([
                {"type": "tool_use", "id": "t1", "name": "TodoWrite", "input": {"todos": [
                    {"content": "Validar email", "status": "completed"},
                    {"content": "Tests", "status": "pending"}]}},
                {"type": "tool_use", "id": "t2", "name": "Edit", "input": {}},
            ])),
            user(
                "r2",
                &serde_json::json!([{"type": "tool_result", "tool_use_id": "t2", "is_error": true,
                "content": "String to replace not found"}]),
                Some(serde_json::json!("Error")),
            ),
        ]);
        assert_eq!(activity.len(), 1);
        let ActivityKind::Plan { items } = &activity[0] else { panic!() };
        assert_eq!(items.iter().map(|i| i.status.as_str()).collect::<Vec<_>>(), ["completed", "pending"]);
    }
}
