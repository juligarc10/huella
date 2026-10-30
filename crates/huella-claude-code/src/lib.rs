//! Adaptador de transcripciones de Claude Code (`~/.claude/projects/**/*.jsonl`).
//!
//! Cada línea es un evento JSON. Las ediciones se leen del campo `toolUseResult`
//! de los resultados de `Edit`, `MultiEdit` y `Write`, que guardan el fichero
//! original completo: así se reconstruye el fichero exacto tras la edición sin
//! depender del estado actual del disco. El formato no está documentado y
//! cambia entre versiones, por eso todo lo desconocido se ignora y se cuenta.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

mod activity;

use huella_core::{
    Activity, AfterState, Agent, AgentBatch, AgentError, Conversation, EditContext, EditKind, EditOrigin, FileEdit,
    Speaker, TranscriptCursor, Turn,
};
use serde::Deserialize;
use serde_json::{Map, Value};

pub use activity::read_activity;

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
    /// Dónde seguir leyendo la próxima vez.
    pub resume: TranscriptCursor,
}

/// Longitud máxima guardada de una petición o explicación.
const MAX_CONTEXT_CHARS: usize = 2000;

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
    read_transcript_from(path, &TranscriptCursor::default())
}

/// Lee una transcripción a partir de `from`, para indexar solo lo nuevo.
///
/// Una última línea sin salto de línea puede estar escribiéndose todavía: no se
/// consume y se leerá entera la próxima vez.
///
/// # Errors
/// Si el fichero no se puede abrir o leer.
pub fn read_transcript_from(path: &Path, from: &TranscriptCursor) -> Result<ParsedTranscript, TranscriptError> {
    let io_error = |source| TranscriptError::Io { path: path.to_path_buf(), source };
    let mut file = File::open(path).map_err(io_error)?;
    file.seek(SeekFrom::Start(from.offset)).map_err(io_error)?;
    let mut reader = BufReader::new(file);
    let mut parser = Parser { parsed: ParsedTranscript::default(), context: from.context.clone() };
    let mut offset = from.offset;
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(io_error)?;
        if read == 0 || !line.ends_with('\n') {
            break;
        }
        offset += read as u64;
        parser.parse_line(&line);
    }
    let Parser { mut parsed, context } = parser;
    parsed.resume = TranscriptCursor { offset, context };
    Ok(parsed)
}

/// Campos de una línea que nos interesan; el resto se ignora.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    is_meta: Option<bool>,
    message: Option<Value>,
    uuid: Option<String>,
    session_id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<PathBuf>,
    tool_use_result: Option<Value>,
}

/// Lector línea a línea que sigue el hilo de la conversación para saber qué
/// se pidió y qué explicó el agente antes de cada edición.
struct Parser {
    parsed: ParsedTranscript,
    context: EditContext,
}

impl Parser {
    fn parse_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        self.parsed.stats.lines += 1;
        let Ok(event) = serde_json::from_str::<RawEvent>(line) else {
            self.parsed.stats.invalid_json += 1;
            return;
        };
        self.track_conversation(&event);

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
        match edit_from_result(result, origin, PathBuf::from(file_path), self.context.clone()) {
            Some(edit) => {
                self.parsed.stats.edits += 1;
                self.parsed.edits.push(edit);
            }
            None => self.parsed.stats.unknown_edit_shapes += 1,
        }
    }

    /// Un mensaje de la persona abre un turno nuevo; el último texto del
    /// asistente es la explicación de lo que va a hacer a continuación.
    fn track_conversation(&mut self, event: &RawEvent) {
        if event.is_meta == Some(true) {
            return;
        }
        let Some(content) = event.message.as_ref().and_then(|message| message.get("content")) else { return };
        match event.kind.as_deref() {
            Some("user") => {
                if let Some(prompt) = user_prompt(content) {
                    self.context = EditContext { prompt: Some(truncate(&prompt)), rationale: None };
                }
            }
            Some("assistant") => {
                if let Some(text) = last_text_block(content) {
                    self.context.rationale = Some(truncate(&text));
                }
            }
            _ => {}
        }
    }
}

/// Texto escrito por la persona. Los resultados de herramientas y las salidas
/// de comandos o avisos internos (empiezan por `<`) no cuentan.
fn user_prompt(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            if blocks.iter().any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result")) {
                return None;
            }
            let texts: Vec<&str> = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect();
            texts.join("\n")
        }
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| text.to_owned())
}

/// Último texto del agente en un mensaje. Los modelos recientes guardan a
/// menudo la explicación entre herramientas como un bloque `thinking` con un
/// resumen del progreso en lugar de como `text`; ambos valen.
fn last_text_block(content: &Value) -> Option<String> {
    content
        .as_array()?
        .iter()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => block.get("text").and_then(Value::as_str),
            Some("thinking") => block.get("thinking").and_then(Value::as_str),
            _ => None,
        })
        .map(str::trim)
        .rfind(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Diálogo de cada sesión de una transcripción, en orden: mensajes de la
/// persona y textos del agente. Las herramientas y sus resultados se omiten.
///
/// # Errors
/// Si el fichero no se puede abrir o leer.
pub fn read_conversations(path: &Path) -> Result<Vec<Conversation>, TranscriptError> {
    let io_error = |source| TranscriptError::Io { path: path.to_path_buf(), source };
    let reader = BufReader::new(File::open(path).map_err(io_error)?);
    let mut conversations: Vec<Conversation> = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(io_error)?;
        let Ok(event) = serde_json::from_str::<RawEvent>(&line) else { continue };
        if event.is_meta == Some(true) {
            continue;
        }
        let Some(content) = event.message.as_ref().and_then(|message| message.get("content")) else { continue };
        let (speaker, text) = match event.kind.as_deref() {
            Some("user") => (Speaker::Person, user_prompt(content)),
            Some("assistant") => (Speaker::Agent, all_text_blocks(content)),
            _ => continue,
        };
        let (Some(text), Some(session_id)) = (text, event.session_id) else { continue };
        let turn = Turn { speaker, text, timestamp: event.timestamp };
        match conversations.iter_mut().find(|conversation| conversation.session_id == session_id) {
            Some(conversation) => conversation.turns.push(turn),
            None => conversations.push(Conversation { session_id, cwd: event.cwd, turns: vec![turn] }),
        }
    }
    Ok(conversations)
}

fn all_text_blocks(content: &Value) -> Option<String> {
    let texts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

fn truncate(text: &str) -> String {
    match text.char_indices().nth(MAX_CONTEXT_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

fn edit_from_result(
    result: &Map<String, Value>,
    origin: EditOrigin,
    path: PathBuf,
    context: EditContext,
) -> Option<FileEdit> {
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
    Some(FileEdit { origin, path, kind, before, after, user_modified, context })
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

/// Claude Code como [`Agent`]: sus transcripciones están bajo `projects`
/// (`~/.claude/projects` por defecto).
pub struct ClaudeCode {
    pub projects: PathBuf,
}

impl Agent for ClaudeCode {
    fn name(&self) -> &'static str {
        AGENT
    }

    fn sessions(&self) -> Result<Vec<PathBuf>, AgentError> {
        if !self.projects.exists() {
            return Ok(Vec::new());
        }
        discover_transcripts(&self.projects).map_err(|error| AgentError::new(AGENT, &self.projects, &error))
    }

    fn read_from(&self, path: &Path, from: &TranscriptCursor) -> Result<AgentBatch, AgentError> {
        let parsed = read_transcript_from(path, from).map_err(|error| AgentError::new(AGENT, path, &error))?;
        Ok(AgentBatch { edits: parsed.edits, resume: parsed.resume })
    }

    fn conversations(&self, path: &Path) -> Result<Vec<Conversation>, AgentError> {
        read_conversations(path).map_err(|error| AgentError::new(AGENT, path, &error))
    }

    fn activity(&self, path: &Path) -> Result<Vec<Activity>, AgentError> {
        read_activity(path).map_err(|error| AgentError::new(AGENT, path, &error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(lines: &[&str]) -> ParsedTranscript {
        let mut parser = Parser { parsed: ParsedTranscript::default(), context: EditContext::default() };
        for line in lines {
            parser.parse_line(line);
        }
        parser.parsed
    }

    fn message_line(kind: &str, content: &Value) -> String {
        serde_json::json!({"type": kind, "message": {"role": kind, "content": content}}).to_string()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-cc-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
    fn edits_carry_the_prompt_and_the_agent_rationale() {
        let write = edit_line(&serde_json::json!({"type": "create", "filePath": "/repo/a.ts", "content": "x"}));
        let lines = [
            message_line("user", &serde_json::json!("Añade validación del email")),
            message_line(
                "assistant",
                &serde_json::json!([{"type": "text", "text": "Valido el email antes de guardar."}, {"type": "tool_use"}]),
            ),
            message_line("user", &serde_json::json!([{"type": "tool_result", "content": "ok"}])),
            message_line("user", &serde_json::json!("<command-name>/clear</command-name>")),
            write.clone(),
            message_line("user", &serde_json::json!([{"type": "text", "text": "Ahora los tests"}])),
            write,
        ];
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let edits = parse(&lines).edits;
        assert_eq!(
            edits[0].context,
            EditContext {
                prompt: Some("Añade validación del email".into()),
                rationale: Some("Valido el email antes de guardar.".into())
            }
        );
        assert_eq!(edits[1].context, EditContext { prompt: Some("Ahora los tests".into()), rationale: None });
    }

    #[test]
    fn progress_notes_stored_as_thinking_count_as_rationale() {
        let write = edit_line(&serde_json::json!({"type": "create", "filePath": "/repo/a.ts", "content": "x"}));
        let lines = [
            message_line("user", &serde_json::json!("Crea el almacenamiento")),
            message_line("assistant", &serde_json::json!([{"type": "text", "text": "Primero miro el esquema."}])),
            message_line("assistant", &serde_json::json!([{"type": "thinking", "thinking": ""}])),
            message_line(
                "assistant",
                &serde_json::json!([{"type": "thinking", "thinking": "Voy a crear el crate con SQLite."}, {"type": "tool_use"}]),
            ),
            write,
        ];
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let edits = parse(&lines).edits;
        assert_eq!(edits[0].context.rationale.as_deref(), Some("Voy a crear el crate con SQLite."));
    }

    #[test]
    fn long_context_is_truncated_on_a_char_boundary() {
        let long = "ñ".repeat(MAX_CONTEXT_CHARS + 10);
        let truncated = truncate(&long);
        assert_eq!(truncated.chars().count(), MAX_CONTEXT_CHARS + 1);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn resumes_reading_without_consuming_a_partial_line() {
        let dir = temp_dir("resume");
        let path = dir.join("s.jsonl");
        let prompt = message_line("user", &serde_json::json!("Primera petición"));
        let edit = edit_line(&serde_json::json!({"type": "create", "filePath": "/repo/a.ts", "content": "x"}));
        let (complete, partial) = edit.split_at(20);
        std::fs::write(&path, format!("{prompt}\n{complete}")).unwrap();

        let first = read_transcript_from(&path, &TranscriptCursor::default()).unwrap();
        assert!(first.edits.is_empty());
        assert_eq!(first.resume.offset, prompt.len() as u64 + 1);
        assert_eq!(first.resume.context.prompt.as_deref(), Some("Primera petición"));

        std::fs::write(&path, format!("{prompt}\n{complete}{partial}\n")).unwrap();
        let second = read_transcript_from(&path, &first.resume).unwrap();
        assert_eq!(second.edits.len(), 1);
        assert_eq!(second.edits[0].context.prompt.as_deref(), Some("Primera petición"));
        assert_eq!(second.resume.offset, std::fs::metadata(&path).unwrap().len());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn conversations_keep_only_the_dialogue_grouped_by_session() {
        let dir = temp_dir("conversation");
        let path = dir.join("s.jsonl");
        let line = |session: &str, kind: &str, content: Value| {
            serde_json::json!({"type": kind, "sessionId": session, "timestamp": "t", "message": {"content": content}})
                .to_string()
        };
        let lines = [
            line("s1", "user", serde_json::json!("¿Rust o Python?")),
            line(
                "s1",
                "assistant",
                serde_json::json!([{"type": "text", "text": "Rust."}, {"type": "tool_use"}, {"type": "text", "text": "Por el binario único."}]),
            ),
            line("s1", "user", serde_json::json!([{"type": "tool_result", "content": "salida"}])),
            line("s1", "user", serde_json::json!("<command-name>/clear</command-name>")),
            line("s2", "user", serde_json::json!("Otra sesión")),
            "{roto".to_owned(),
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();

        let conversations = read_conversations(&path).unwrap();
        assert_eq!(conversations.len(), 2);
        let texts: Vec<(Speaker, &str)> =
            conversations[0].turns.iter().map(|turn| (turn.speaker, turn.text.as_str())).collect();
        assert_eq!(
            texts,
            vec![(Speaker::Person, "¿Rust o Python?"), (Speaker::Agent, "Rust.\n\nPor el binario único.")]
        );
        assert_eq!(conversations[1].session_id, "s2");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_fixture_file_and_discovers_it() {
        let dir = temp_dir("discover");
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
