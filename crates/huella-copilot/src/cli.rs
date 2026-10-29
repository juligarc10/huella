//! La CLI `copilot`: `~/.copilot/session-state/<sesión>/events.jsonl`.
//!
//! Cada línea es un evento. Las ediciones son `tool.execution_start` de las
//! herramientas `edit` (texto anterior y nuevo) y `create` (fichero entero);
//! solo cuentan si su `tool.execution_complete` dice que salieron bien. El
//! fichero es pequeño y la pareja inicio y fin puede quedar a ambos lados
//! de un indexado, así que se lee entero cada vez que crece.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use huella_core::agent::find_files;
use huella_core::{
    AfterState, Agent, AgentBatch, AgentError, Conversation, EditContext, EditKind, EditOrigin, FileEdit, Speaker,
    TranscriptCursor, Turn,
};
use serde_json::Value;

use crate::clip;

pub const CLI_AGENT: &str = "copilot-cli";

pub struct CopilotCli {
    /// `~/.copilot/session-state`.
    pub root: PathBuf,
}

impl CopilotCli {
    #[must_use]
    pub fn detect(home: &Path) -> Self {
        Self { root: home.join(".copilot").join("session-state") }
    }
}

impl Agent for CopilotCli {
    fn name(&self) -> &'static str {
        CLI_AGENT
    }

    fn sessions(&self) -> Result<Vec<PathBuf>, AgentError> {
        find_files(CLI_AGENT, &self.root, |path| path.file_name().is_some_and(|name| name == "events.jsonl"))
    }

    fn read_from(&self, path: &Path, _from: &TranscriptCursor) -> Result<AgentBatch, AgentError> {
        let events = load(path)?;
        let size = std::fs::metadata(path).map_err(|error| AgentError::new(CLI_AGENT, path, &error))?.len();
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
                let speaker = match event["type"].as_str()? {
                    "user.message" => Speaker::Person,
                    "assistant.message" => Speaker::Agent,
                    _ => return None,
                };
                let text = event["data"]["content"].as_str().filter(|t| !t.trim().is_empty())?;
                Some(Turn { speaker, text: text.to_owned(), timestamp: event["timestamp"].as_str().map(str::to_owned) })
            })
            .collect();
        if turns.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![Conversation { session_id: session_id(&events, path), cwd: cwd(&events), turns }])
    }
}

fn load(path: &Path) -> Result<Vec<Value>, AgentError> {
    let text = std::fs::read_to_string(path).map_err(|error| AgentError::new(CLI_AGENT, path, &error))?;
    Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
}

fn session_id(events: &[Value], path: &Path) -> String {
    events
        .iter()
        .find_map(|event| event["data"]["sessionId"].as_str().map(str::to_owned))
        .or_else(|| path.parent()?.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

fn cwd(events: &[Value]) -> Option<PathBuf> {
    events.iter().find_map(|event| event["data"]["context"]["cwd"].as_str()).map(PathBuf::from)
}

fn edits(events: &[Value], path: &Path) -> Vec<FileEdit> {
    let session = session_id(events, path);
    let cwd = cwd(events);
    let succeeded: HashSet<&str> = events
        .iter()
        .filter(|event| event["type"] == "tool.execution_complete" && event["data"]["success"] == true)
        .filter_map(|event| event["data"]["toolCallId"].as_str())
        .collect();
    let mut prompt: Option<String> = None;
    let mut rationale: Option<String> = None;
    let mut found = Vec::new();
    for event in events {
        let data = &event["data"];
        match event["type"].as_str() {
            Some("user.message") => {
                prompt = data["content"].as_str().map(clip).filter(|p| !p.is_empty());
                rationale = None;
            }
            Some("assistant.message") => {
                if let Some(text) = data["content"].as_str().filter(|t| !t.trim().is_empty()) {
                    rationale = Some(clip(text));
                }
            }
            Some("tool.execution_start") => {
                let Some(call) = data["toolCallId"].as_str().filter(|id| succeeded.contains(id)) else { continue };
                let arguments = &data["arguments"];
                let Some(file) = arguments["path"].as_str() else { continue };
                let (kind, after) = match data["toolName"].as_str() {
                    Some("create") => (
                        EditKind::Create,
                        AfterState::Known(arguments["file_text"].as_str().unwrap_or_default().to_owned()),
                    ),
                    Some("edit") => match arguments["new_str"].as_str().filter(|text| !text.trim().is_empty()) {
                        Some(new) => (EditKind::Replace, AfterState::FragmentOnly(new.to_owned())),
                        None => continue,
                    },
                    _ => continue,
                };
                found.push(FileEdit {
                    origin: EditOrigin {
                        agent: CLI_AGENT,
                        session_id: session.clone(),
                        event_id: call.to_owned(),
                        timestamp: event["timestamp"].as_str().map(str::to_owned),
                        cwd: cwd.clone(),
                    },
                    path: PathBuf::from(file),
                    kind,
                    before: None,
                    after,
                    user_modified: false,
                    context: EditContext { prompt: prompt.clone(), rationale: rationale.clone() },
                });
            }
            _ => {}
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events() -> Vec<Value> {
        [
            r#"{"type":"session.start","data":{"sessionId":"s1","context":{"cwd":"/repo"}},"timestamp":"2026-03-11T19:18:01.930Z"}"#,
            r#"{"type":"user.message","data":{"content":"arregla el test de la base de datos"},"timestamp":"2026-03-11T19:18:05.000Z"}"#,
            r#"{"type":"assistant.message","data":{"content":"El contenedor no crea el esquema: lo configuro."},"timestamp":"2026-03-11T19:18:09.000Z"}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"t1","toolName":"edit","arguments":{"path":"/repo/a.yml","old_str":"url: x","new_str":"url: x?TC=1"}},"timestamp":"2026-03-11T19:18:10.000Z"}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"t1","success":true}}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"t2","toolName":"edit","arguments":{"path":"/repo/b.yml","old_str":"a","new_str":"b"}},"timestamp":"2026-03-11T19:18:11.000Z"}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"t2","success":false}}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"t3","toolName":"create","arguments":{"path":"/repo/c.yml","file_text":"spring: {}\n"}},"timestamp":"2026-03-11T19:18:12.000Z"}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"t3","success":true}}"#,
        ]
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
    }

    #[test]
    fn successful_edits_and_creations_with_their_context() {
        let found = edits(&events(), Path::new("/x/s1/events.jsonl"));
        let summary: Vec<_> = found.iter().map(|e| (e.origin.event_id.as_str(), e.kind)).collect();
        assert_eq!(summary, vec![("t1", EditKind::Replace), ("t3", EditKind::Create)], "la edición fallida no cuenta");
        assert_eq!(found[0].after, AfterState::FragmentOnly("url: x?TC=1".into()));
        assert_eq!(found[0].context.prompt.as_deref(), Some("arregla el test de la base de datos"));
        assert_eq!(found[0].context.rationale.as_deref(), Some("El contenedor no crea el esquema: lo configuro."));
        assert_eq!(found[0].origin.cwd.as_deref(), Some(Path::new("/repo")));
        assert_eq!(found[1].after, AfterState::Known("spring: {}\n".into()));
    }
}
