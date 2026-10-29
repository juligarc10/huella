//! Indexado incremental de transcripciones.

use huella_anchoring::anchor_edit;
use huella_core::TranscriptCursor;
use huella_store::{IndexedEdit, Store};

use crate::paths::normalize_path;
use crate::sources::Sources;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IndexReport {
    pub transcripts: usize,
    /// Transcripciones con líneas nuevas desde el último indexado.
    pub updated: usize,
    pub new_edits: usize,
    /// Transcripciones que encogieron (reescritas o truncadas) y se releyeron enteras.
    pub restarted: usize,
    /// Transcripciones que no se pudieron leer; el resto se indexa igualmente.
    pub failures: Vec<String>,
}

/// Indexa lo nuevo de cada sesión de cada agente. Cada fichero de sesión se
/// guarda en su propia transacción: un fallo en uno no pierde los demás, y un
/// agente que no se puede leer no impide indexar los otros.
///
/// # Errors
/// Si falla la base de datos.
pub fn index(store: &mut Store, sources: &Sources) -> crate::Result<IndexReport> {
    let mut report = IndexReport::default();
    for agent in sources.agents() {
        let sessions = match agent.sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                report.failures.push(error.to_string());
                continue;
            }
        };
        index_agent(store, agent.as_ref(), &sessions, &mut report)?;
    }
    Ok(report)
}

fn index_agent(
    store: &mut Store,
    agent: &dyn huella_core::Agent,
    sessions: &[std::path::PathBuf],
    report: &mut IndexReport,
) -> crate::Result<()> {
    for transcript in sessions {
        report.transcripts += 1;
        let key = transcript.display().to_string();
        let Ok(metadata) = std::fs::metadata(transcript) else {
            report.failures.push(format!("{key}: desapareció durante el indexado"));
            continue;
        };
        let mut cursor = store.cursor(&key)?.unwrap_or_default();
        if metadata.len() < cursor.offset {
            cursor = TranscriptCursor::default();
            report.restarted += 1;
        }
        if metadata.len() == cursor.offset {
            continue;
        }
        let mut parsed = match agent.read_from(transcript, &cursor) {
            Ok(parsed) => parsed,
            Err(error) => {
                report.failures.push(error.to_string());
                continue;
            }
        };
        if parsed.resume.offset == cursor.offset {
            continue;
        }
        // Nada se guarda con secretos: ni el contexto de cada edición ni el
        // que queda en el cursor para la próxima lectura.
        for edit in &mut parsed.edits {
            redact_context(&mut edit.context);
        }
        redact_context(&mut parsed.resume.context);
        let anchored: Vec<_> = parsed
            .edits
            .iter()
            .map(|edit| {
                let anchor = anchor_edit(edit, |path| std::fs::read_to_string(path).ok());
                (normalize_path(&edit.path, edit.origin.cwd.as_deref()), anchor)
            })
            .collect();
        let indexed: Vec<IndexedEdit> = parsed
            .edits
            .iter()
            .zip(&anchored)
            .map(|(edit, (path, anchor))| IndexedEdit {
                edit,
                path,
                outcome: &anchor.outcome,
                changed_lines: &anchor.changed_lines,
            })
            .collect();
        store.save(&key, &parsed.resume, &indexed)?;
        report.updated += 1;
        report.new_edits += indexed.len();
    }
    Ok(())
}

fn redact_context(context: &mut huella_core::EditContext) {
    for text in [&mut context.prompt, &mut context.rationale].into_iter().flatten() {
        if let std::borrow::Cow::Owned(clean) = huella_core::redact::redact(text) {
            *text = clean;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Línea de transcripción con un `Write` que crea `path`.
    fn edit_line(uuid: &str, path: &str, content: &str) -> String {
        format!(
            r#"{{"type":"user","uuid":"{uuid}","sessionId":"s1","timestamp":"2026-10-25T10:00:00Z","cwd":"/repo","toolUseResult":{{"type":"create","filePath":"{path}","content":"{content}"}}}}{}"#,
            '\n'
        )
    }

    #[test]
    fn indexes_only_new_lines_and_restarts_truncated_transcripts() {
        let dir = std::env::temp_dir().join(format!("huella-index-{}", std::process::id()));
        let project = dir.join("-repo");
        std::fs::create_dir_all(&project).unwrap();
        let transcript = project.join("s1.jsonl");
        let first = edit_line("e1", "/repo/a.ts", "export function a() {}");
        std::fs::write(&transcript, &first).unwrap();

        let mut store = Store::open_in_memory().unwrap();
        let report = index(&mut store, &Sources::claude_code(&dir)).unwrap();
        assert_eq!((report.updated, report.new_edits), (1, 1));
        assert_eq!(store.history("/repo/a.ts", "a").unwrap().len(), 1);

        assert_eq!(index(&mut store, &Sources::claude_code(&dir)).unwrap().updated, 0, "sin líneas nuevas no se relee");

        let second = edit_line("e2", "/repo/.claude/worktrees/w/b.ts", "export function b() {}");
        std::fs::write(&transcript, format!("{first}{second}")).unwrap();
        let report = index(&mut store, &Sources::claude_code(&dir)).unwrap();
        assert_eq!(report.new_edits, 1);
        assert_eq!(store.history("/repo/b.ts", "b").unwrap().len(), 1, "ruta de worktree normalizada");

        std::fs::write(&transcript, &first).unwrap();
        let report = index(&mut store, &Sources::claude_code(&dir)).unwrap();
        assert_eq!(report.restarted, 1);
        assert_eq!(store.stats().unwrap().edits, 2, "releer no duplica");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
