//! Revisión de lo que cambió en una sesión.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use huella_claude_code::{discover_transcripts, read_transcript};
use huella_core::FileEdit;
use huella_review::SessionReview;
use huella_store::{SessionSummary, Store};

use crate::decisions::project_from_paths;
use crate::paths::normalize_path;
use crate::{EngineError, Result};

/// Sesión con su proyecto, para elegir cuál revisar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListing {
    pub summary: SessionSummary,
    pub project: Option<String>,
}

/// Sesiones recientes con ediciones.
///
/// # Errors
/// Si falla la base de datos.
pub fn list_sessions(store: &Store, limit: usize) -> Result<Vec<SessionListing>> {
    store
        .sessions(limit)?
        .into_iter()
        .map(|summary| {
            let project = project_from_paths(&store.session_paths(&summary.id)?, None);
            Ok(SessionListing { summary, project })
        })
        .collect()
}

/// Proyecto de una sesión: el repositorio (o carpeta) donde editó más ficheros.
///
/// # Errors
/// Si falla la base de datos.
pub fn session_project(store: &Store, session_id: &str) -> Result<Option<String>> {
    Ok(project_from_paths(&store.session_paths(session_id)?, None))
}

/// Revisión de la sesión cuyo id empieza por `prefix`. Se lee de las
/// transcripciones, que guardan el contenido completo de cada edición.
///
/// # Errors
/// Si no hay ninguna sesión con ese prefijo, hay varias, o falla la lectura.
pub fn session_review(projects: &Path, prefix: &str) -> Result<SessionReview> {
    let mut edits: Vec<FileEdit> = Vec::new();
    let mut sessions: BTreeSet<String> = BTreeSet::new();
    // Última edición de cada fichero en cualquier sesión, para saber si otra
    // sesión lo tocó después.
    let mut latest: BTreeMap<String, (Option<String>, String)> = BTreeMap::new();
    for transcript in discover_transcripts(projects)? {
        for edit in read_transcript(&transcript)?.edits {
            let path = normalize_path(&edit.path, edit.origin.cwd.as_deref());
            let entry = latest.entry(path).or_insert((None, String::new()));
            if edit.origin.timestamp >= entry.0 {
                *entry = (edit.origin.timestamp.clone(), edit.origin.session_id.clone());
            }
            if edit.origin.session_id.starts_with(prefix) {
                sessions.insert(edit.origin.session_id.clone());
                edits.push(edit);
            }
        }
    }
    let session = match sessions.len() {
        0 => return Err(EngineError::SessionNotFound(prefix.to_owned())),
        1 => sessions.pop_first().unwrap_or_default(),
        _ => {
            let candidates = sessions.into_iter().collect();
            return Err(EngineError::AmbiguousSession { prefix: prefix.to_owned(), candidates });
        }
    };
    // Una sesión continuada se reparte entre transcripciones: orden por tiempo,
    // estable para conservar el orden de escritura dentro de cada una.
    edits.sort_by(|a, b| a.origin.timestamp.cmp(&b.origin.timestamp));
    // El disco solo se compara si ninguna sesión posterior tocó el fichero: si
    // no, sus cambios se confundirían con los de esta.
    let current = |path: &str| {
        let last_session = latest.get(path).map(|(_, session)| session.as_str());
        (last_session == Some(session.as_str())).then(|| std::fs::read_to_string(path).ok()).flatten()
    };
    Ok(huella_review::build(&session, &edits, |edit| normalize_path(&edit.path, edit.origin.cwd.as_deref()), current))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn line(uuid: &str, session: &str, at: &str, result: &serde_json::Value) -> String {
        serde_json::json!({"type": "user", "uuid": uuid, "sessionId": session, "timestamp": at, "cwd": "/repo",
                           "toolUseResult": result})
        .to_string()
    }

    #[test]
    fn reviews_a_session_split_across_transcripts_and_resolves_prefixes() {
        let dir = std::env::temp_dir().join(format!("huella-review-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("-repo")).unwrap();
        let create = serde_json::json!({"type": "create", "filePath": "/repo/a.ts", "content": "a\nb\n"});
        let change =
            serde_json::json!({"filePath": "/repo/a.ts", "oldString": "b", "newString": "B", "originalFile": "a\nb\n"});
        let other = serde_json::json!({"type": "create", "filePath": "/repo/z.ts", "content": "z\n"});
        let mut first = String::new();
        let _ = writeln!(first, "{}", line("e1", "aaaa-1", "2026-10-27T10:00:00Z", &create));
        let _ = writeln!(first, "{}", line("e9", "bbbb-1", "2026-10-27T10:00:00Z", &other));
        std::fs::write(dir.join("-repo/t1.jsonl"), first).unwrap();
        std::fs::write(
            dir.join("-repo/t2.jsonl"),
            format!("{}\n", line("e2", "aaaa-1", "2026-10-27T11:00:00Z", &change)),
        )
        .unwrap();

        let review = session_review(&dir, "aaaa").unwrap();
        assert_eq!(review.session_id, "aaaa-1");
        assert_eq!(review.files.len(), 1);
        assert_eq!(review.files[0].after, "a\nB\n");
        assert!(matches!(session_review(&dir, "cccc"), Err(EngineError::SessionNotFound(_))));
        std::fs::write(
            dir.join("-repo/t3.jsonl"),
            format!("{}\n", line("e3", "aaaa-2", "2026-10-27T12:00:00Z", &other)),
        )
        .unwrap();
        assert!(matches!(session_review(&dir, "aaaa"), Err(EngineError::AmbiguousSession { .. })));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
