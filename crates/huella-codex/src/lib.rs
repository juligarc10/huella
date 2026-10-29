//! Adaptador de Codex (CLI y extensión): `~/.codex/sessions/**/rollout-*.jsonl`
//! (o `$CODEX_HOME/sessions`).
//!
//! Cada línea es `{timestamp, type, payload}`. La carpeta de trabajo llega en
//! `session_meta` o `turn_context`; las peticiones y respuestas, como
//! `response_item` de tipo `message`; y las ediciones, como llamadas a
//! `apply_patch` (herramienta propia o dentro de `shell`), con bloques
//! `*** Add File`, `*** Update File` y `*** Delete File`. De un fichero
//! nuevo se conoce el contenido entero; de uno modificado, el texto de cada
//! trozo tras el cambio, que el anclaje localiza en el fichero actual.
//!
//! Validado con transcripciones de ejemplo, no con sesiones reales con
//! ediciones: el formato no está documentado como estable.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use huella_core::agent::find_files;
use huella_core::{
    AfterState, Agent, AgentBatch, AgentError, Conversation, EditContext, EditKind, EditOrigin, FileEdit, Speaker,
    TranscriptCursor, Turn,
};
use serde_json::Value;

pub const AGENT: &str = "codex";

/// Longitud máxima guardada de una petición o explicación.
const MAX_CONTEXT_CHARS: usize = 2000;

pub struct Codex {
    /// `~/.codex/sessions`.
    pub root: PathBuf,
}

impl Codex {
    #[must_use]
    pub fn detect(home: &Path) -> Self {
        let base = std::env::var_os("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from);
        Self { root: base.join("sessions") }
    }
}

impl Agent for Codex {
    fn name(&self) -> &'static str {
        AGENT
    }

    fn sessions(&self) -> Result<Vec<PathBuf>, AgentError> {
        find_files(AGENT, &self.root, |path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            name.starts_with("rollout-") && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
        })
    }

    fn read_from(&self, path: &Path, _from: &TranscriptCursor) -> Result<AgentBatch, AgentError> {
        let events = load(path)?;
        let size = std::fs::metadata(path).map_err(|error| AgentError::new(AGENT, path, &error))?.len();
        Ok(AgentBatch {
            edits: edits(&events, path),
            resume: TranscriptCursor { offset: size, ..TranscriptCursor::default() },
        })
    }

    fn conversations(&self, path: &Path) -> Result<Vec<Conversation>, AgentError> {
        let events = load(path)?;
        let turns: Vec<Turn> = events
            .iter()
            .filter_map(|event| {
                let (speaker, text) = message(&event["payload"])?;
                Some(Turn { speaker, text, timestamp: event["timestamp"].as_str().map(str::to_owned) })
            })
            .collect();
        if turns.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![Conversation { session_id: session_id(&events, path), cwd: cwd(&events), turns }])
    }
}

fn load(path: &Path) -> Result<Vec<Value>, AgentError> {
    let text = std::fs::read_to_string(path).map_err(|error| AgentError::new(AGENT, path, &error))?;
    Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
}

fn session_id(events: &[Value], path: &Path) -> String {
    events
        .iter()
        .find(|event| event["type"] == "session_meta")
        .and_then(|event| event["payload"]["id"].as_str().map(str::to_owned))
        .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
}

fn cwd(events: &[Value]) -> Option<PathBuf> {
    events
        .iter()
        .filter(|event| matches!(event["type"].as_str(), Some("session_meta" | "turn_context")))
        .find_map(|event| event["payload"]["cwd"].as_str())
        .map(PathBuf::from)
}

/// Mensaje de la persona o del agente, sin el contexto que inyecta Codex
/// (`<environment_context>`, `<user_instructions>`…).
fn message(payload: &Value) -> Option<(Speaker, String)> {
    if payload["type"] != "message" {
        return None;
    }
    let speaker = match payload["role"].as_str()? {
        "user" => Speaker::Person,
        "assistant" => Speaker::Agent,
        _ => return None,
    };
    let text: Vec<&str> = payload["content"]
        .as_array()?
        .iter()
        .filter(|part| matches!(part["type"].as_str(), Some("input_text" | "output_text" | "text")))
        .filter_map(|part| part["text"].as_str())
        .collect();
    let text = text.join("\n");
    let injected = text.trim_start().starts_with('<');
    (!text.trim().is_empty() && !injected).then_some((speaker, text))
}

/// El parche de una llamada a `apply_patch`, venga como herramienta propia o
/// como orden `shell` (`["apply_patch", "*** Begin Patch…"]`).
fn patch_of(payload: &Value) -> Option<(String, String)> {
    let call = payload["call_id"].as_str()?.to_owned();
    match payload["type"].as_str()? {
        "custom_tool_call" if payload["name"] == "apply_patch" => Some((call, payload["input"].as_str()?.to_owned())),
        "function_call" => {
            let arguments: Value = serde_json::from_str(payload["arguments"].as_str()?).ok()?;
            let command = arguments["command"].as_array()?;
            let patch = command.iter().filter_map(Value::as_str).find(|part| part.contains("*** Begin Patch"))?;
            (payload["name"] == "apply_patch"
                || command.first().and_then(Value::as_str) == Some("apply_patch")
                || patch.starts_with("*** Begin Patch"))
            .then(|| (call, patch.to_owned()))
        }
        _ => None,
    }
}

/// Llamadas cuya salida dice que fallaron.
fn failed_calls(events: &[Value]) -> HashSet<String> {
    events
        .iter()
        .map(|event| &event["payload"])
        .filter(|payload| matches!(payload["type"].as_str(), Some("function_call_output" | "custom_tool_call_output")))
        .filter(|payload| {
            let output = &payload["output"];
            let text = output.as_str().unwrap_or_default();
            let parsed: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            let exit = parsed["metadata"]["exit_code"].as_i64().or_else(|| output["metadata"]["exit_code"].as_i64());
            exit.is_some_and(|code| code != 0) || text.trim_start().to_lowercase().starts_with("error")
        })
        .filter_map(|payload| payload["call_id"].as_str().map(str::to_owned))
        .collect()
}

fn edits(events: &[Value], path: &Path) -> Vec<FileEdit> {
    let session = session_id(events, path);
    let cwd = cwd(events);
    let failed = failed_calls(events);
    let mut prompt: Option<String> = None;
    let mut rationale: Option<String> = None;
    let mut found = Vec::new();
    for event in events {
        let payload = &event["payload"];
        if let Some((speaker, text)) = message(payload) {
            match speaker {
                Speaker::Person => {
                    prompt = Some(clip(&text));
                    rationale = None;
                }
                Speaker::Agent => rationale = Some(clip(&text)),
            }
            continue;
        }
        let Some((call, diff)) = patch_of(payload) else { continue };
        if failed.contains(&call) {
            continue;
        }
        for (index, change) in parse_patch(&diff).into_iter().enumerate() {
            let file = match &cwd {
                Some(cwd) if Path::new(&change.path).is_relative() => cwd.join(&change.path),
                _ => PathBuf::from(&change.path),
            };
            found.push(FileEdit {
                origin: EditOrigin {
                    agent: AGENT,
                    session_id: session.clone(),
                    event_id: format!("{call}:{index}"),
                    timestamp: event["timestamp"].as_str().map(str::to_owned),
                    cwd: cwd.clone(),
                },
                path: file,
                kind: change.kind,
                before: None,
                after: change.after,
                user_modified: false,
                context: EditContext { prompt: prompt.clone(), rationale: rationale.clone() },
            });
        }
    }
    found
}

/// Un cambio de un parche: un fichero nuevo, o un trozo de uno modificado.
struct Change {
    path: String,
    kind: EditKind,
    after: AfterState,
}

/// Interpreta el formato de `apply_patch`. Cada trozo (`@@`) de un fichero
/// modificado es un cambio: su texto tras aplicarlo (contexto y líneas
/// añadidas) es lo que se busca en el fichero.
/// Fichero, tipo y líneas del cambio que se está leyendo.
type Pending = Option<(String, EditKind, Vec<String>)>;

fn parse_patch(diff: &str) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut current: Pending = None;
    let flush = |current: &mut Pending, changes: &mut Vec<Change>| {
        if let Some((path, kind, lines)) = current.take() {
            let text = lines.join("\n");
            let after = match kind {
                EditKind::Create => Some(AfterState::Known(if text.is_empty() { text } else { format!("{text}\n") })),
                _ => (!text.trim().is_empty()).then_some(AfterState::FragmentOnly(text)),
            };
            if let Some(after) = after {
                changes.push(Change { path, kind, after });
            }
        }
    };
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            flush(&mut current, &mut changes);
            current = Some((path.trim().to_owned(), EditKind::Create, Vec::new()));
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            flush(&mut current, &mut changes);
            current = Some((path.trim().to_owned(), EditKind::Replace, Vec::new()));
        } else if line.starts_with("*** Delete File: ") || line.starts_with("*** End Patch") {
            flush(&mut current, &mut changes);
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if let Some((target, ..)) = current.as_mut() {
                path.trim().clone_into(target);
            }
        } else if line.starts_with("@@") {
            // Nuevo trozo del mismo fichero: el anterior es un cambio aparte.
            if let Some((file, EditKind::Replace, lines)) = &current
                && !lines.is_empty()
            {
                let file = file.clone();
                flush(&mut current, &mut changes);
                current = Some((file, EditKind::Replace, Vec::new()));
            }
        } else if let Some((_, kind, lines)) = current.as_mut() {
            // Las añadidas siempre; el contexto, solo en ficheros modificados.
            match (line.chars().next(), *kind) {
                (Some('+'), _) | (Some(' '), EditKind::Replace) => lines.push(line[1..].to_owned()),
                (None, EditKind::Replace) => lines.push(String::new()),
                _ => {}
            }
        }
    }
    flush(&mut current, &mut changes);
    changes
}

fn clip(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(MAX_CONTEXT_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "*** Begin Patch\n*** Add File: src/email.ts\n+export function normalize(email: string) {\n+  return email.trim();\n+}\n*** Update File: src/user.ts\n@@ class UserService\n   register(email: string) {\n-    return save(email);\n+    return save(normalize(email));\n   }\n@@\n-import { save } from './db';\n+import { save } from './db';\n+import { normalize } from './email';\n*** Delete File: src/old.ts\n*** End Patch";

    fn rollout(patch_call: &Value, output: &Value) -> Vec<Value> {
        [
            serde_json::json!({"timestamp": "2026-06-27T21:40:10.202Z", "type": "session_meta", "payload": {"id": "019f", "cwd": "/repo"}}),
            serde_json::json!({"timestamp": "2026-06-27T21:40:11Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>…</environment_context>"}]}}),
            serde_json::json!({"timestamp": "2026-06-27T21:40:12Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "normaliza el email al registrar"}]}}),
            serde_json::json!({"timestamp": "2026-06-27T21:40:13Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Extraigo la normalización a su propio módulo."}]}}),
            serde_json::json!({"timestamp": "2026-06-27T21:40:14Z", "type": "response_item", "payload": patch_call}),
            serde_json::json!({"timestamp": "2026-06-27T21:40:15Z", "type": "response_item", "payload": output}),
        ]
        .into()
    }

    #[test]
    fn a_custom_apply_patch_becomes_one_edit_per_file_and_hunk() {
        let events = rollout(
            &serde_json::json!({"type": "custom_tool_call", "name": "apply_patch", "call_id": "c1", "input": PATCH}),
            &serde_json::json!({"type": "custom_tool_call_output", "call_id": "c1", "output": "Success. Updated the following files:"}),
        );
        let found = edits(&events, Path::new("/x/rollout-1.jsonl"));
        let summary: Vec<_> = found.iter().map(|e| (e.path.display().to_string(), e.kind)).collect();
        assert_eq!(
            summary,
            vec![
                ("/repo/src/email.ts".to_owned(), EditKind::Create),
                ("/repo/src/user.ts".to_owned(), EditKind::Replace),
                ("/repo/src/user.ts".to_owned(), EditKind::Replace),
            ]
        );
        assert_eq!(
            found[0].after,
            AfterState::Known("export function normalize(email: string) {\n  return email.trim();\n}\n".into())
        );
        assert_eq!(
            found[1].after,
            AfterState::FragmentOnly("  register(email: string) {\n    return save(normalize(email));\n  }".into())
        );
        assert_eq!(
            found[1].context.prompt.as_deref(),
            Some("normaliza el email al registrar"),
            "sin el contexto inyectado"
        );
        assert_eq!(found[1].context.rationale.as_deref(), Some("Extraigo la normalización a su propio módulo."));
        assert_eq!(found[1].origin.event_id, "c1:1");
    }

    #[test]
    fn a_patch_through_shell_counts_unless_it_failed() {
        let call = |id: &str| {
            serde_json::json!({"type": "function_call", "name": "shell", "call_id": id,
                "arguments": serde_json::json!({"command": ["apply_patch", PATCH]}).to_string()})
        };
        let ok = rollout(
            &call("c2"),
            &serde_json::json!({"type": "function_call_output", "call_id": "c2",
            "output": serde_json::json!({"output": "Done", "metadata": {"exit_code": 0}}).to_string()}),
        );
        assert_eq!(edits(&ok, Path::new("/x/r.jsonl")).len(), 3);
        let failed = rollout(
            &call("c3"),
            &serde_json::json!({"type": "function_call_output", "call_id": "c3",
            "output": serde_json::json!({"output": "patch failed", "metadata": {"exit_code": 1}}).to_string()}),
        );
        assert_eq!(edits(&failed, Path::new("/x/r.jsonl")).len(), 0);
    }

    #[test]
    fn the_dialogue_skips_injected_context() {
        let events = rollout(&serde_json::json!({}), &serde_json::json!({}));
        let turns: Vec<_> = events.iter().filter_map(|e| message(&e["payload"])).map(|(_, text)| text).collect();
        assert_eq!(turns, vec!["normaliza el email al registrar", "Extraigo la normalización a su propio módulo."]);
    }
}
