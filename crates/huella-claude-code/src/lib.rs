//! Adaptador de transcripciones de Claude Code (`~/.claude/projects/**/*.jsonl`).
//!
//! Cada línea es un evento JSON. Las ediciones se leen del campo `toolUseResult`
//! de los resultados de `Edit`, `MultiEdit` y `Write`, que guardan el fichero
//! original completo: así se reconstruye el fichero exacto tras la edición sin
//! depender del estado actual del disco. El formato no está documentado y
//! cambia entre versiones, por eso todo lo desconocido se ignora y se cuenta.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use huella_core::{AfterState, EditKind, EditOrigin, FileEdit};
use serde::Deserialize;
use serde_json::{Map, Value};

pub const AGENT: &str = "claude-code";

#[derive(Debug, thiserror::Error)]
pub enum TranscriptError {
    #[error("no se pudo leer {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

/// Contadores de lo que se leyó y de lo que se descartó, para que nada se
/// pierda en silencio.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParseStats {
    pub lines: usize,
    pub invalid_json: usize,
    pub edits: usize,
    /// Resultados con `filePath` pero con una forma que no reconocemos.
    pub unknown_edit_shapes: usize,
}

impl ParseStats {
    pub fn merge(&mut self, other: &Self) {
        self.lines += other.lines;
        self.invalid_json += other.invalid_json;
        self.edits += other.edits;
        self.unknown_edit_shapes += other.unknown_edit_shapes;
    }
}

#[derive(Debug, Default)]
pub struct ParsedTranscript {
    pub edits: Vec<FileEdit>,
    pub stats: ParseStats,
}

/// Busca todas las transcripciones `.jsonl` bajo `root`, ordenadas por ruta.
///
/// # Errors
/// Si algún directorio no se puede leer.
pub fn discover_transcripts(root: &Path) -> Result<Vec<PathBuf>, TranscriptError> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|source| TranscriptError::Io { path: dir.clone(), source })?;
        for entry in entries {
            let path = entry.map_err(|source| TranscriptError::Io { path: dir.clone(), source })?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Lee una transcripción y devuelve sus ediciones normalizadas.
///
/// # Errors
/// Si el fichero no se puede leer. Las líneas que no se entienden no son un
/// error: se cuentan en [`ParseStats`].
pub fn read_transcript(path: &Path) -> Result<ParsedTranscript, TranscriptError> {
    let io_error = |source| TranscriptError::Io { path: path.to_path_buf(), source };
    let reader = BufReader::new(File::open(path).map_err(io_error)?);
    let mut parsed = ParsedTranscript::default();
    for line in reader.lines() {
        let line = line.map_err(io_error)?;
        parse_line(&line, &mut parsed);
    }
    Ok(parsed)
}

/// Campos de una línea que nos interesan; el resto se ignora.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    uuid: Option<String>,
    session_id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<PathBuf>,
    tool_use_result: Option<Value>,
}

fn parse_line(line: &str, parsed: &mut ParsedTranscript) {
    if line.trim().is_empty() {
        return;
    }
    parsed.stats.lines += 1;
    let Ok(event) = serde_json::from_str::<RawEvent>(line) else {
        parsed.stats.invalid_json += 1;
        return;
    };
    // Los resultados de herramientas fallidas son cadenas de error: no son ediciones.
    let Some(Value::Object(result)) = &event.tool_use_result else { return };
    let Some(file_path) = str_field(result, "filePath") else { return };

    let origin = EditOrigin {
        agent: AGENT,
        session_id: event.session_id.clone().unwrap_or_default(),
        event_id: event.uuid.clone().unwrap_or_default(),
        timestamp: event.timestamp.clone(),
        cwd: event.cwd.clone(),
    };
    match edit_from_result(result, origin, PathBuf::from(file_path)) {
        Some(edit) => {
            parsed.stats.edits += 1;
            parsed.edits.push(edit);
        }
        None => parsed.stats.unknown_edit_shapes += 1,
    }
}

fn edit_from_result(result: &Map<String, Value>, origin: EditOrigin, path: PathBuf) -> Option<FileEdit> {
    let before = str_field(result, "originalFile").map(str::to_owned);
    let user_modified = result.get("userModified").and_then(Value::as_bool).unwrap_or(false);

    let (kind, before, after) = if let Some(content) = str_field(result, "content") {
        // Write: el contenido final viene completo.
        let kind = if str_field(result, "type") == Some("create") { EditKind::Create } else { EditKind::Rewrite };
        (kind, before, AfterState::Known(content.to_owned()))
    } else {
        let after = apply_replacements(before.as_deref(), &replacements(result)?);
        (EditKind::Replace, before, after)
    };
    Some(FileEdit { origin, path, kind, before, after, user_modified })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Replacement {
    old: String,
    new: String,
    replace_all: bool,
}

/// `Edit` trae un único reemplazo; `MultiEdit`, una lista `edits`.
fn replacements(result: &Map<String, Value>) -> Option<Vec<Replacement>> {
    if let Some(Value::Array(edits)) = result.get("edits") {
        return edits.iter().map(|edit| edit.as_object().and_then(replacement)).collect();
    }
    replacement(result).map(|single| vec![single])
}

fn replacement(fields: &Map<String, Value>) -> Option<Replacement> {
    let old = str_field(fields, "oldString").or_else(|| str_field(fields, "old_string"))?;
    let new = str_field(fields, "newString").or_else(|| str_field(fields, "new_string"))?;
    let replace_all =
        ["replaceAll", "replace_all"].iter().find_map(|key| fields.get(*key).and_then(Value::as_bool)).unwrap_or(false);
    Some(Replacement { old: old.to_owned(), new: new.to_owned(), replace_all })
}

/// Aplica los reemplazos al fichero original. Sin original, o si algún
/// fragmento no aparece, solo se conoce el texto insertado.
fn apply_replacements(before: Option<&str>, replacements: &[Replacement]) -> AfterState {
    let fragment_only = || {
        let inserted: Vec<&str> = replacements.iter().map(|r| r.new.as_str()).collect();
        AfterState::FragmentOnly(inserted.join("\n"))
    };
    let Some(mut text) = before.map(str::to_owned) else { return fragment_only() };
    for r in replacements {
        if r.old.is_empty() {
            // Claude Code permite crear un fichero vacío con old_string = "".
            if !text.is_empty() {
                return fragment_only();
            }
            text.clone_from(&r.new);
        } else if !text.contains(&r.old) {
            return fragment_only();
        } else if r.replace_all {
            text = text.replace(&r.old, &r.new);
        } else {
            text = text.replacen(&r.old, &r.new, 1);
        }
    }
    AfterState::Known(text)
}

fn str_field<'a>(fields: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    fields.get(key).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(lines: &[&str]) -> ParsedTranscript {
        let mut parsed = ParsedTranscript::default();
        for line in lines {
            parse_line(line, &mut parsed);
        }
        parsed
    }

    fn edit_line(result: &Value) -> String {
        serde_json::json!({
            "type": "user", "uuid": "e1", "sessionId": "s1",
            "timestamp": "2026-10-25T10:00:00Z", "cwd": "/repo",
            "toolUseResult": result,
        })
        .to_string()
    }

    #[test]
    fn edit_with_original_file_is_reconstructed_exactly() {
        let line = edit_line(&serde_json::json!({
            "filePath": "/repo/a.ts", "oldString": "return 1", "newString": "return 2",
            "originalFile": "function f() {\n  return 1\n}\n", "replaceAll": false, "userModified": false,
        }));
        let parsed = parse(&[&line]);
        let edit = &parsed.edits[0];
        assert_eq!(edit.kind, EditKind::Replace);
        assert_eq!(edit.after, AfterState::Known("function f() {\n  return 2\n}\n".into()));
        assert_eq!(edit.origin.session_id, "s1");
        assert_eq!(edit.origin.cwd.as_deref(), Some(Path::new("/repo")));
    }

    #[test]
    fn edit_without_original_file_keeps_only_the_fragment() {
        let line = edit_line(&serde_json::json!({
            "filePath": "/repo/a.ts", "oldString": "x", "newString": "y", "originalFile": null,
        }));
        assert_eq!(parse(&[&line]).edits[0].after, AfterState::FragmentOnly("y".into()));
    }

    #[test]
    fn edit_whose_old_string_is_missing_falls_back_to_fragment() {
        let line = edit_line(&serde_json::json!({
            "filePath": "/repo/a.ts", "oldString": "absent", "newString": "y", "originalFile": "x",
        }));
        assert_eq!(parse(&[&line]).edits[0].after, AfterState::FragmentOnly("y".into()));
    }

    #[test]
    fn replace_all_replaces_every_occurrence() {
        let line = edit_line(&serde_json::json!({
            "filePath": "/repo/a.ts", "oldString": "a", "newString": "b",
            "originalFile": "a a a", "replaceAll": true,
        }));
        assert_eq!(parse(&[&line]).edits[0].after, AfterState::Known("b b b".into()));
    }

    #[test]
    fn multi_edit_applies_replacements_in_order() {
        let line = edit_line(&serde_json::json!({
            "filePath": "/repo/a.ts", "originalFile": "one two",
            "edits": [
                {"old_string": "one", "new_string": "uno"},
                {"old_string": "uno two", "new_string": "uno dos"},
            ],
        }));
        assert_eq!(parse(&[&line]).edits[0].after, AfterState::Known("uno dos".into()));
    }

    #[test]
    fn write_distinguishes_create_from_rewrite() {
        let create = edit_line(&serde_json::json!({
            "type": "create", "filePath": "/repo/new.py", "content": "x = 1\n", "originalFile": null,
        }));
        let rewrite = edit_line(&serde_json::json!({
            "type": "update", "filePath": "/repo/old.py", "content": "x = 2\n", "originalFile": "x = 1\n",
        }));
        let parsed = parse(&[&create, &rewrite]);
        assert_eq!(parsed.edits[0].kind, EditKind::Create);
        assert_eq!(parsed.edits[0].before, None);
        assert_eq!(parsed.edits[1].kind, EditKind::Rewrite);
        assert_eq!(parsed.edits[1].before.as_deref(), Some("x = 1\n"));
    }

    #[test]
    fn noise_is_counted_not_fatal() {
        let failed_tool = edit_line(&serde_json::json!("Error: file not found"));
        let unknown = edit_line(&serde_json::json!({"filePath": "/repo/a.ts", "somethingNew": 1}));
        let parsed = parse(&["{not json", r#"{"type":"assistant"}"#, &failed_tool, &unknown, ""]);
        assert!(parsed.edits.is_empty());
        assert_eq!(parsed.stats, ParseStats { lines: 4, invalid_json: 1, edits: 0, unknown_edit_shapes: 1 });
    }

    #[test]
    fn reads_fixture_file_and_discovers_it() {
        let dir = std::env::temp_dir().join(format!("huella-cc-{}", std::process::id()));
        let project = dir.join("-repo");
        std::fs::create_dir_all(&project).unwrap();
        let line = edit_line(&serde_json::json!({
            "type": "create", "filePath": "/repo/a.rs", "content": "fn main() {}\n",
        }));
        std::fs::write(project.join("s1.jsonl"), format!("{line}\n")).unwrap();
        std::fs::write(project.join("notes.txt"), "ignored").unwrap();

        let found = discover_transcripts(&dir).unwrap();
        assert_eq!(found, vec![project.join("s1.jsonl")]);
        assert_eq!(read_transcript(&found[0]).unwrap().edits.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
