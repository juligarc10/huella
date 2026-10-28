//! Extracción de decisiones de proyecto de las sesiones indexadas.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use huella_claude_code::{discover_transcripts, read_conversations};
use huella_core::Conversation;
use huella_decisions::{
    DEFAULT_CHUNK_CHARS, DecisionExtractor, DecisionScope, ExtractedDecision, ExtractionRequest, PreviousDecision,
    chunk_dialogue,
};
use huella_store::{DecisionFilter, NewDecision, SessionDecisions, Store};

use crate::paths::normalize_path;

/// Sesiones con menos mensajes no suelen contener decisiones.
const MIN_TURNS: usize = 4;
/// Decisiones vigentes de otras sesiones que ve el modelo: las más recientes.
/// Más alargan la petición sin que suelan cambiar en una sesión.
const MAX_PREVIOUS: usize = 40;

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
    extractor: &(impl DecisionExtractor + ?Sized),
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
        store.set_decisions_project(&conversation.session_id, project.as_deref())?;
        if store.extracted_turns(&conversation.session_id)? == Some(conversation.turns.len()) {
            report.unchanged += 1;
            continue;
        }
        if options.limit.is_some_and(|limit| report.extracted + report.failures.len() >= limit) {
            continue;
        }
        progress(&conversation);
        let (previous, uids) = previous_decisions(store, &conversation.session_id, project.as_deref())?;
        let mut decisions: Vec<NewDecision> = Vec::new();
        let mut failure = None;
        for dialogue in chunk_dialogue(&conversation, options.chunk_chars) {
            report.calls += 1;
            let known_titles = decisions.iter().map(|d| d.title.clone()).collect();
            let request = ExtractionRequest { dialogue, known_titles, previous: previous.clone() };
            match extractor.extract(&request) {
                Ok(found) => {
                    decisions.extend(found.into_iter().map(|d| new_decision(d, &uids, project.as_deref())));
                }
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

/// Decisiones vigentes de otras sesiones del proyecto (y las de la persona)
/// con la clave que verá el modelo, y qué id estable tiene cada clave.
fn previous_decisions(
    store: &Store,
    session_id: &str,
    project: Option<&str>,
) -> crate::Result<(Vec<PreviousDecision>, BTreeMap<String, String>)> {
    let filter = DecisionFilter { project: project.map(str::to_owned), ..DecisionFilter::default() };
    let active = store.decisions(&filter)?;
    let mut previous = Vec::new();
    let mut uids = BTreeMap::new();
    for (number, decision) in (1..).zip(active.into_iter().filter(|d| d.session_id != session_id).take(MAX_PREVIOUS)) {
        let key = format!("D{number}");
        uids.insert(key.clone(), decision.uid);
        previous.push(PreviousDecision { key, title: decision.title, decision: decision.decision });
    }
    Ok((previous, uids))
}

/// Decisión del modelo lista para guardar: sus claves de reemplazo pasan a
/// ids estables (las que no existen se ignoran) y su ruta, a absoluta.
fn new_decision(extracted: ExtractedDecision, uids: &BTreeMap<String, String>, project: Option<&str>) -> NewDecision {
    let scope_path = (extracted.scope == DecisionScope::Path).then(|| {
        let path = Path::new(&extracted.scope_path);
        match project {
            Some(project) if path.is_relative() => Path::new(project).join(path).display().to_string(),
            _ => extracted.scope_path.clone(),
        }
    });
    NewDecision {
        title: extracted.title,
        decision: extracted.decision,
        rationale: extracted.rationale,
        alternatives: extracted.alternatives,
        entities: extracted.entities,
        status: extracted.status.as_str().to_owned(),
        supersedes: extracted.supersedes,
        scope: extracted.scope.as_str().to_owned(),
        scope_path,
        replaces: extracted.replaces_previous.iter().filter_map(|key| uids.get(key).cloned()).collect(),
    }
}

/// Proyecto de una sesión: donde editó más ficheros. El directorio de arranque
/// no sirve si se lanzó desde una carpeta que agrupa varios proyectos. Sin
/// ediciones indexadas, se usa ese directorio.
fn session_project(store: &Store, conversation: &Conversation) -> crate::Result<Option<String>> {
    let paths = store.session_paths(&conversation.session_id)?;
    let cwd = conversation.cwd.as_deref().map(|cwd| normalize_path(cwd, None));
    Ok(project_from_paths(&paths, cwd.as_deref().map(Path::new)).or(cwd))
}

/// Cada fichero vota por su repositorio git o, si no lo tiene, por la
/// subcarpeta del directorio de arranque que lo contiene.
pub(crate) fn project_from_paths(paths: &[String], cwd: Option<&Path>) -> Option<String> {
    let mut votes: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for path in paths.iter().map(Path::new) {
        let repository = path.ancestors().skip(1).find(|dir| dir.join(".git").exists()).map(Path::to_path_buf);
        let subfolder = || {
            let cwd = cwd?;
            let mut inside = path.strip_prefix(cwd).ok()?.components();
            let folder = inside.next()?.as_os_str();
            let is_project = inside.next().is_some() && !folder.to_string_lossy().starts_with('.');
            is_project.then(|| cwd.join(folder))
        };
        if let Some(project) = repository.or_else(subfolder) {
            *votes.entry(project).or_default() += 1;
        }
    }
    votes.into_iter().max_by_key(|(_, count)| *count).map(|(project, _)| project.display().to_string())
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

    /// Extractor de pega: una decisión por cada línea `DECIDE: título` del
    /// diálogo, o `REPLACE D1: título` si reemplaza una de sesiones anteriores.
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
                .filter_map(|line| {
                    if let Some(title) = line.strip_prefix("DECIDE: ") {
                        return Some((title, Vec::new()));
                    }
                    let (key, title) = line.strip_prefix("REPLACE ")?.split_once(": ")?;
                    Some((title, vec![key.to_owned()]))
                })
                .map(|(title, replaces_previous)| ExtractedDecision {
                    title: title.into(),
                    decision: title.into(),
                    rationale: "motivo".into(),
                    alternatives: vec![],
                    entities: vec!["rust".into()],
                    status: DecisionStatus::Accepted,
                    supersedes: String::new(),
                    scope: DecisionScope::Project,
                    scope_path: String::new(),
                    replaces_previous,
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
    fn a_later_session_sees_and_replaces_decisions_of_earlier_ones() {
        let dir = std::env::temp_dir().join(format!("huella-decisions-later-{}", std::process::id()));
        write_session(&dir, "s1", &["¿base de datos?", "DECIDE: SQLite en local", "vale", "hecho"]);
        let mut store = Store::open_in_memory().unwrap();
        let fake = Fake { requests: RefCell::new(vec![]), fail: false };
        extract_decisions(&mut store, &dir, &fake, &ExtractionOptions::default(), |_| {}).unwrap();

        write_session(&dir, "s2", &["mejor DuckDB", "REPLACE D1: DuckDB en local", "vale", "hecho"]);
        extract_decisions(&mut store, &dir, &fake, &ExtractionOptions::default(), |_| {}).unwrap();
        let seen = fake.requests.borrow().last().unwrap().previous.clone();
        assert_eq!(
            seen.iter().map(|p| (p.key.as_str(), p.title.as_str())).collect::<Vec<_>>(),
            [("D1", "SQLite en local")]
        );

        let active: Vec<String> =
            store.decisions(&DecisionFilter::default()).unwrap().into_iter().map(|d| d.title).collect();
        assert_eq!(active, vec!["DuckDB en local"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn relative_path_scopes_are_resolved_against_the_project() {
        let extracted = ExtractedDecision {
            title: "Kysely en la API".into(),
            decision: "d".into(),
            rationale: "r".into(),
            alternatives: vec![],
            entities: vec![],
            status: DecisionStatus::Accepted,
            supersedes: String::new(),
            scope: DecisionScope::Path,
            scope_path: "apps/api".into(),
            replaces_previous: vec!["D9".into()],
        };
        let decision = new_decision(extracted, &BTreeMap::new(), Some("/repo"));
        assert_eq!(decision.scope_path.as_deref(), Some("/repo/apps/api"));
        assert_eq!(decision.replaces, Vec::<String>::new(), "una clave que no existe se ignora");
    }

    #[test]
    fn the_project_is_the_repository_with_most_edited_files() {
        let dir = std::env::temp_dir().join(format!("huella-project-{}", std::process::id()));
        for repo in ["canjea", "lua"] {
            std::fs::create_dir_all(dir.join(repo).join(".git")).unwrap();
        }
        let file = |repo: &str, name: &str| dir.join(repo).join("src").join(name).display().to_string();
        let paths = [file("canjea", "a.ts"), file("canjea", "b.ts"), file("lua", "c.ts"), "/fuera/de/git.ts".into()];
        assert_eq!(project_from_paths(&paths, None), Some(dir.join("canjea").display().to_string()));
        assert_eq!(project_from_paths(&["/fuera/de/git.ts".into()], None), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_git_the_project_is_the_subfolder_of_the_session_directory() {
        let cwd = Path::new("/dev");
        let paths = [
            "/dev/web/css/a.css".to_owned(),
            "/dev/web/index.html".into(),
            "/dev/.claude/x.json".into(),
            "/dev/notas.md".into(),
        ];
        assert_eq!(project_from_paths(&paths, Some(cwd)), Some("/dev/web".into()));
        assert_eq!(
            project_from_paths(&["/dev/notas.md".into()], Some(cwd)),
            None,
            "un fichero suelto no define proyecto"
        );
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
