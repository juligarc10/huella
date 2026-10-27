//! Extracción de decisiones de proyecto de las sesiones indexadas.

use std::collections::BTreeMap;
use std::path::Path;

use huella_claude_code::{discover_transcripts, read_conversations};
use huella_core::Conversation;
use huella_decisions::{DEFAULT_CHUNK_CHARS, DecisionExtractor, ExtractionRequest, chunk_dialogue};
use huella_store::{NewDecision, SessionDecisions, Store};

use crate::paths::normalize_path;

/// Sesiones con menos mensajes no suelen contener decisiones.
const MIN_TURNS: usize = 4;

/// Cómo extraer: cuántas sesiones como mucho y de qué tamaño los trozos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionOptions {
    pub limit: Option<usize>,
    /// Caracteres por trozo de diálogo; ajústalo al contexto del modelo
    /// (unos 3,5 caracteres por token en español).
    pub chunk_chars: usize,
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self { limit: None, chunk_chars: DEFAULT_CHUNK_CHARS }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExtractionReport {
    pub sessions: usize,
    pub extracted: usize,
    /// Sesiones que no han crecido desde su última extracción.
    pub unchanged: usize,
    pub calls: usize,
    pub decisions: usize,
    /// Sesiones que fallaron; se reintentan la próxima vez.
    pub failures: Vec<String>,
}

/// Extrae las decisiones de las sesiones nuevas o que han crecido.
/// `options.limit` acota cuántas sesiones se procesan (útil para probar un modelo);
/// `progress` recibe cada sesión antes de procesarla.
///
/// # Errors
/// Si no se pueden leer las transcripciones o falla la base de datos. Los
/// fallos del modelo no son errores: se anotan en el informe.
pub fn extract_decisions(
    store: &mut Store,
    projects: &Path,
    extractor: &dyn DecisionExtractor,
    options: &ExtractionOptions,
    mut progress: impl FnMut(&Conversation),
) -> crate::Result<ExtractionReport> {
    let mut report = ExtractionReport::default();
    let extractor_id = extractor.id();
    for conversation in conversations(projects)? {
        if conversation.turns.len() < MIN_TURNS {
            continue;
        }
        report.sessions += 1;
        let project = session_project(store, &conversation)?;
        if store.extracted_turns(&conversation.session_id)? == Some(conversation.turns.len()) {
            store.set_decisions_project(&conversation.session_id, project.as_deref())?;
            report.unchanged += 1;
            continue;
        }
        if options.limit.is_some_and(|limit| report.extracted + report.failures.len() >= limit) {
            continue;
        }
        progress(&conversation);
        let mut decisions: Vec<NewDecision> = Vec::new();
        let mut failure = None;
        for dialogue in chunk_dialogue(&conversation, options.chunk_chars) {
            report.calls += 1;
            let known_titles = decisions.iter().map(|d| d.title.clone()).collect();
            match extractor.extract(&ExtractionRequest { dialogue, known_titles }) {
                Ok(found) => decisions.extend(found.into_iter().map(|d| NewDecision {
                    title: d.title,
                    decision: d.decision,
                    rationale: d.rationale,
                    alternatives: d.alternatives,
                    entities: d.entities,
                    status: d.status.as_str().to_owned(),
                    supersedes: d.supersedes,
                })),
                Err(error) => {
                    failure = Some(format!("{}: {error}", conversation.session_id));
                    break;
                }
            }
        }
        if let Some(failure) = failure {
            report.failures.push(failure);
            continue;
        }
        store.replace_decisions(&SessionDecisions {
            session_id: &conversation.session_id,
            project: project.as_deref(),
            extractor: &extractor_id,
            turns: conversation.turns.len(),
            decided_at: conversation.turns.iter().rev().find_map(|turn| turn.timestamp.as_deref()),
            decisions: &decisions,
        })?;
        report.extracted += 1;
        report.decisions += decisions.len();
    }
    Ok(report)
}

/// Proyecto de una sesión: el repositorio git donde editó más ficheros. El
/// directorio de arranque no sirve si se lanzó desde una carpeta que agrupa
/// varios proyectos. Sin ediciones indexadas, se usa ese directorio.
fn session_project(store: &Store, conversation: &Conversation) -> crate::Result<Option<String>> {
    let paths = store.session_paths(&conversation.session_id)?;
    let fallback = conversation.cwd.as_deref().map(|cwd| normalize_path(cwd, None));
    Ok(project_from_paths(&paths).or(fallback))
}

fn project_from_paths(paths: &[String]) -> Option<String> {
    let mut votes: BTreeMap<String, usize> = BTreeMap::new();
    for path in paths {
        if let Some(root) = Path::new(path).ancestors().skip(1).find(|dir| dir.join(".git").exists()) {
            *votes.entry(root.display().to_string()).or_default() += 1;
        }
    }
    votes.into_iter().max_by_key(|(_, count)| *count).map(|(root, _)| root)
}

/// Diálogo de todas las sesiones. Una sesión continuada puede repartirse
/// entre varias transcripciones: sus mensajes se juntan en orden.
fn conversations(projects: &Path) -> crate::Result<Vec<Conversation>> {
    let mut by_session: BTreeMap<String, Conversation> = BTreeMap::new();
    for transcript in discover_transcripts(projects)? {
        for conversation in read_conversations(&transcript)? {
            match by_session.get_mut(&conversation.session_id) {
                Some(existing) => existing.turns.extend(conversation.turns),
                None => {
                    by_session.insert(conversation.session_id.clone(), conversation);
                }
            }
        }
    }
    let mut all: Vec<Conversation> = by_session.into_values().collect();
    for conversation in &mut all {
        conversation.turns.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fmt::Write as _;

    use huella_decisions::{DecisionStatus, ExtractError, ExtractedDecision};
    use huella_store::DecisionFilter;

    use super::*;

    /// Extractor de pega: una decisión por cada línea `DECIDE:` del diálogo.
    struct Fake {
        requests: RefCell<Vec<ExtractionRequest>>,
        fail: bool,
    }

    impl DecisionExtractor for Fake {
        fn id(&self) -> String {
            "fake:1".into()
        }

        fn extract(&self, request: &ExtractionRequest) -> Result<Vec<ExtractedDecision>, ExtractError> {
            self.requests.borrow_mut().push(request.clone());
            if self.fail {
                return Err(ExtractError::Truncated);
            }
            Ok(request
                .dialogue
                .lines()
                .filter_map(|line| line.strip_prefix("DECIDE: "))
                .map(|title| ExtractedDecision {
                    title: title.into(),
                    decision: title.into(),
                    rationale: "motivo".into(),
                    alternatives: vec![],
                    entities: vec!["rust".into()],
                    status: DecisionStatus::Accepted,
                    supersedes: String::new(),
                })
                .collect())
        }
    }

    fn write_session(dir: &Path, session: &str, messages: &[&str]) {
        let project = dir.join("-repo");
        std::fs::create_dir_all(&project).unwrap();
        let lines = messages.iter().enumerate().fold(String::new(), |mut out, (index, text)| {
            let kind = if index % 2 == 0 { "user" } else { "assistant" };
            let content = if kind == "user" {
                serde_json::json!(text)
            } else {
                serde_json::json!([{"type": "text", "text": text}])
            };
            let line = serde_json::json!({"type": kind, "sessionId": session, "cwd": "/repo",
                                          "timestamp": format!("2026-10-25T10:00:0{index}Z"),
                                          "message": {"content": content}});
            let _ = writeln!(out, "{line}");
            out
        });
        std::fs::write(project.join(format!("{session}.jsonl")), lines).unwrap();
    }

    #[test]
    fn extracts_new_sessions_once_and_again_when_they_grow() {
        let dir = std::env::temp_dir().join(format!("huella-decisions-{}", std::process::id()));
        write_session(&dir, "s1", &["¿Rust?", "DECIDE: Rust", "vale", "hecho"]);
        write_session(&dir, "corta", &["hola", "hola"]);
        let mut store = Store::open_in_memory().unwrap();
        let fake = Fake { requests: RefCell::new(vec![]), fail: false };

        let report = extract_decisions(&mut store, &dir, &fake, &ExtractionOptions::default(), |_| {}).unwrap();
        assert_eq!((report.sessions, report.extracted, report.decisions), (1, 1, 1));
        let stored = store.decisions(&DecisionFilter::default()).unwrap();
        assert_eq!(stored[0].project.as_deref(), Some("/repo"));
        assert_eq!(stored[0].extractor, "fake:1");

        let again = extract_decisions(&mut store, &dir, &fake, &ExtractionOptions::default(), |_| {}).unwrap();
        assert_eq!((again.unchanged, again.calls), (1, 0), "sin mensajes nuevos no se llama al modelo");

        write_session(&dir, "s1", &["¿Rust?", "DECIDE: Rust", "vale", "hecho", "¿y SQLite?", "DECIDE: SQLite"]);
        let grown = extract_decisions(&mut store, &dir, &fake, &ExtractionOptions::default(), |_| {}).unwrap();
        assert_eq!(grown.extracted, 1);
        assert_eq!(store.decisions(&DecisionFilter::default()).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_project_is_the_repository_with_most_edited_files() {
        let dir = std::env::temp_dir().join(format!("huella-project-{}", std::process::id()));
        for repo in ["canjea", "lua"] {
            std::fs::create_dir_all(dir.join(repo).join(".git")).unwrap();
        }
        let file = |repo: &str, name: &str| dir.join(repo).join("src").join(name).display().to_string();
        let paths = [file("canjea", "a.ts"), file("canjea", "b.ts"), file("lua", "c.ts"), "/fuera/de/git.ts".into()];
        assert_eq!(project_from_paths(&paths), Some(dir.join("canjea").display().to_string()));
        assert_eq!(project_from_paths(&["/fuera/de/git.ts".into()]), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_sessions_are_not_saved_and_respect_the_limit() {
        let dir = std::env::temp_dir().join(format!("huella-decisions-fail-{}", std::process::id()));
        write_session(&dir, "s1", &["a", "DECIDE: A", "b", "c"]);
        write_session(&dir, "s2", &["a", "DECIDE: B", "b", "c"]);
        let mut store = Store::open_in_memory().unwrap();
        let failing = Fake { requests: RefCell::new(vec![]), fail: true };
        let report = extract_decisions(
            &mut store,
            &dir,
            &failing,
            &ExtractionOptions { limit: Some(1), ..ExtractionOptions::default() },
            |_| {},
        )
        .unwrap();
        assert_eq!((report.failures.len(), failing.requests.borrow().len()), (1, 1));
        assert_eq!(store.extracted_turns("s1").unwrap(), None, "se reintentará");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
