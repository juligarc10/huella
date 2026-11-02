//! Revisión por tarea: lo que se revisa es exactamente lo que se va a mergear.
//!
//! Una tarea (de JIRA o de donde sea) se implementa en una rama con el mismo
//! nombre en cada repositorio que toca, y cada rama se mergea en su destino
//! (una rama de sprint, `develop`…). La revisión junta todas: en cada
//! repositorio, desde el `merge-base` con el destino del remoto recién traído
//! hasta lo que hay en commits en la rama. Lo que no está en un commit no
//! entra, y si la rama local y la del remoto no coinciden se avisa.
//!
//! Las sesiones de los agentes ya no son la unidad: sus ediciones solo sirven
//! para atribuir cada línea y contar por qué se escribió.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};
use huella_review::SessionReview;
use huella_store::{Store, TaskRepo};
use serde::Serialize;

use crate::branch::{RealPaths, unix_seconds};
use crate::paths::normalize_path;
use crate::workspace::git;
use crate::{EngineError, Result};

/// Ficheros más grandes (o binarios) no se revisan línea a línea.
const MAX_FILE_BYTES: usize = 1024 * 1024;
/// Ramas del remoto que se miran para proponer el destino, las más recientes.
const TARGET_CANDIDATES: &str = "60";
/// Sesiones de las que se sacan los repositorios conocidos.
const KNOWN_SESSIONS: usize = 500;

/// Identificador de la revisión de una tarea, para guardar veredictos y guías.
#[must_use]
pub fn review_id(key: &str) -> String {
    format!("task:{key}")
}

/// La clave de la tarea en el nombre de una rama (`feature/DENMS-1234-filtros`
/// → `DENMS-1234`): letras, un guion y dígitos.
#[must_use]
pub fn key_of(branch: &str) -> Option<String> {
    let chars: Vec<char> = branch.chars().collect();
    let mut start = 0;
    while start < chars.len() {
        let letters = chars[start..].iter().take_while(|c| c.is_ascii_alphanumeric()).count();
        let word = &chars[start..start + letters];
        let at_boundary = start == 0 || !chars[start - 1].is_ascii_alphanumeric();
        if at_boundary
            && word.first().is_some_and(char::is_ascii_alphabetic)
            && chars.get(start + letters) == Some(&'-')
        {
            let digits = chars[start + letters + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            let ends_well = chars.get(start + letters + 1 + digits).is_none_or(|c| !c.is_ascii_alphanumeric());
            if digits > 0 && ends_well {
                let key: String = chars[start..start + letters + 1 + digits].iter().collect();
                return Some(key.to_ascii_uppercase());
            }
        }
        start += letters.max(1);
    }
    None
}

/// Cómo está un repositorio de la tarea respecto al remoto.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepoSync {
    pub repo: String,
    pub branch: String,
    pub target: Option<String>,
    /// Se trajo el remoto antes de mirar.
    pub fetched: bool,
    /// Por qué no se pudo traer el remoto, si falló (se sigue con lo que hay).
    pub fetch_error: Option<String>,
    /// Lo que se revisa: la rama local o, si no está, la del remoto.
    pub branch_ref: Option<String>,
    /// Contra qué: el destino del remoto o, si no está, el local.
    pub target_ref: Option<String>,
    /// Commits de la rama local que no están en el remoto (sin subir).
    pub unpushed: usize,
    /// Commits del remoto que la rama local no tiene (sin bajar).
    pub unpulled: usize,
    /// La rama no existe en el remoto.
    pub not_pushed: bool,
    /// Rama que tiene el repositorio en la copia de trabajo.
    pub checked_out: Option<String>,
    /// Ficheros con cambios sin commit, si la copia de trabajo está en la rama:
    /// no entran en la revisión.
    pub uncommitted: Vec<String>,
    /// Commits de la rama sobre el destino: lo que se va a mergear.
    pub commits: usize,
    /// Commits del destino posteriores al punto de partida de la rama.
    pub target_ahead: usize,
    /// El problema que impide revisarlo, si lo hay.
    pub error: Option<String>,
}

impl RepoSync {
    /// Avisos para la persona, de lo que hace que la revisión no sea lo que se mergea.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(error) = &self.fetch_error {
            out.push(format!("no se pudo traer el remoto ({error}): puede estar desactualizado"));
        } else if !self.fetched {
            out.push("sin traer el remoto: el destino puede estar desactualizado".into());
        }
        if self.not_pushed {
            out.push("la rama no está en el remoto: se revisa la local".into());
        }
        if self.unpushed > 0 {
            out.push(format!("{} commits sin subir: la PR aún no los tiene", self.unpushed));
        }
        if self.unpulled > 0 {
            out.push(format!("{} commits en el remoto que la rama local no tiene", self.unpulled));
        }
        if !self.uncommitted.is_empty() {
            out.push(format!("{} ficheros con cambios sin commit: no entran en la revisión", self.uncommitted.len()));
        }
        out
    }
}

/// Repositorios de las sesiones de los agentes: donde buscar las ramas de una tarea.
///
/// # Errors
/// Si falla la base de datos.
pub fn known_repos(store: &Store) -> Result<Vec<PathBuf>> {
    let mut repos = BTreeSet::new();
    for session in store.sessions(KNOWN_SESSIONS)? {
        let Some(project) = crate::decisions::project_from_paths(&store.session_paths(&session.id)?, None) else {
            continue;
        };
        if let Ok(root) = git(Path::new(&project), &["rev-parse", "--show-toplevel"]) {
            repos.insert(PathBuf::from(root.trim()));
        }
    }
    Ok(repos.into_iter().collect())
}

/// Ramas cuyo nombre lleva la clave de la tarea, una por repositorio (la local
/// si la hay; si no, la del remoto).
#[must_use]
pub fn find_branches(repos: &[PathBuf], key: &str) -> Vec<(PathBuf, String)> {
    let wanted = key.to_ascii_uppercase();
    let mut found = Vec::new();
    for repo in repos {
        let Ok(refs) = git(repo, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes/origin"]) else {
            continue;
        };
        let names: Vec<(bool, String)> = refs
            .lines()
            .filter_map(|name| {
                name.strip_prefix("refs/heads/")
                    .map(|branch| (true, branch.to_owned()))
                    .or_else(|| name.strip_prefix("refs/remotes/origin/").map(|branch| (false, branch.to_owned())))
            })
            .filter(|(_, branch)| branch != "HEAD" && key_of(branch).as_deref() == Some(wanted.as_str()))
            .collect();
        let local = names.iter().find(|(local, _)| *local).or_else(|| names.first());
        if let Some((_, branch)) = local {
            found.push((repo.clone(), branch.clone()));
        }
    }
    found
}

/// Un posible destino de la rama.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetCandidate {
    pub name: String,
    /// Cuándo se separó la rama de él (segundos Unix del `merge-base`).
    pub forked_at: i64,
    /// Commits de la rama sobre él.
    pub commits: usize,
}

/// Destinos posibles de la rama, el más probable primero: la rama del remoto
/// de la que se separó más recientemente (y, a igualdad, sobre la que tiene
/// menos commits). `preferred` va delante si existe: el destino que ya tienen
/// otros repositorios de la tarea.
///
/// # Errors
/// Si el repositorio no es de git.
pub fn suggest_targets(repo: &Path, branch: &str, preferred: Option<&str>) -> Result<Vec<TargetCandidate>> {
    let branch_ref = branch_ref(repo, branch).ok_or_else(|| EngineError::Git(format!("no existe la rama {branch}")))?;
    let key = key_of(branch);
    let refs = git(
        repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            &format!("--count={TARGET_CANDIDATES}"),
            "--format=%(refname:short)",
            "refs/remotes/origin",
        ],
    )?;
    let mut candidates = Vec::new();
    for name in refs.lines().filter_map(|name| name.strip_prefix("origin/")) {
        if name == "HEAD" || name == branch || (key.is_some() && key_of(name) == key) {
            continue;
        }
        let remote = format!("origin/{name}");
        let Ok(base) = git(repo, &["merge-base", &remote, &branch_ref]) else { continue };
        let base = base.trim();
        let forked_at =
            git(repo, &["show", "-s", "--format=%ct", base]).ok().and_then(|at| at.trim().parse().ok()).unwrap_or(0);
        candidates.push(TargetCandidate { name: name.to_owned(), forked_at, commits: count(repo, base, &branch_ref) });
    }
    candidates.sort_by_key(|candidate| {
        (Some(candidate.name.as_str()) != preferred, std::cmp::Reverse(candidate.forked_at), candidate.commits)
    });
    candidates.truncate(5);
    Ok(candidates)
}

/// Trae del remoto el destino y la rama (si falla, se sigue sin red) y mira
/// cómo está todo.
#[must_use]
pub fn sync(repo: &Path, branch: &str, target: Option<&str>, fetch: bool) -> RepoSync {
    let mut state = RepoSync {
        repo: repo.display().to_string(),
        branch: branch.to_owned(),
        target: target.map(str::to_owned),
        ..RepoSync::default()
    };
    state.fetched = fetch;
    if fetch {
        // Por separado: si la rama no está en el remoto, el destino se trae igual.
        for name in target.into_iter().chain([branch]) {
            if let Err(error) = git(repo, &["fetch", "--quiet", "origin", name])
                && name != branch
            {
                state.fetch_error = Some(error.to_string());
            }
        }
    }
    state.checked_out = checked_out(repo);
    let local = exists(repo, &format!("refs/heads/{branch}"));
    let remote = exists(repo, &format!("refs/remotes/origin/{branch}"));
    state.not_pushed = !remote;
    if local && remote {
        state.unpushed = count(repo, &format!("origin/{branch}"), branch);
        state.unpulled = count(repo, branch, &format!("origin/{branch}"));
    }
    if state.checked_out.as_deref() == Some(branch) {
        state.uncommitted = git(repo, &["status", "--porcelain", "--untracked-files=normal"])
            .map(|status| status.lines().filter_map(|line| line.get(3..)).map(str::to_owned).collect())
            .unwrap_or_default();
    }
    state.branch_ref = branch_ref(repo, branch);
    let Some(target) = target else {
        state.error = Some("falta elegir a qué rama se mergea".into());
        return state;
    };
    state.target_ref = target_ref(repo, target);
    match (&state.branch_ref, &state.target_ref) {
        (None, _) => state.error = Some(format!("no existe la rama {branch}")),
        (_, None) => state.error = Some(format!("no existe la rama de destino {target}")),
        (Some(branch_ref), Some(target_ref)) => match git(repo, &["merge-base", target_ref, branch_ref]) {
            Ok(base) => {
                state.commits = count(repo, base.trim(), branch_ref);
                state.target_ahead = count(repo, base.trim(), target_ref);
            }
            Err(error) => state.error = Some(error.to_string()),
        },
    }
    state
}

/// La revisión de una tarea y el estado de cada repositorio.
#[derive(Debug, Clone)]
pub struct TaskReview {
    pub review: SessionReview,
    pub repos: Vec<RepoSync>,
    /// Ficheros que no se revisan (binarios o demasiado grandes), con su ruta absoluta.
    pub skipped: Vec<String>,
}

/// Revisión de la tarea: en cada repositorio con destino, del `merge-base`
/// con el destino a lo que hay en commits en la rama. Las rutas son absolutas,
/// para que los pasos de distintos repositorios no se confundan.
///
/// # Errors
/// Si la tarea no existe, falla la base de datos o no se pueden leer las sesiones.
pub fn task_review(store: &Store, sources: &crate::Sources, key: &str, fetch: bool) -> Result<TaskReview> {
    store.task(key)?.ok_or_else(|| EngineError::Git(format!("no hay ninguna tarea {key}")))?;
    let repos = store.task_repos(key)?;
    let mut real = RealPaths::default();
    let agent_edits = agent_edits_by_path(sources, &mut real)?;
    let mut edits: Vec<FileEdit> = Vec::new();
    let mut after: HashMap<String, String> = HashMap::new();
    let mut states = Vec::new();
    let mut skipped = Vec::new();
    for TaskRepo { repo, branch, target } in repos {
        let root = Path::new(&repo);
        let state = sync(root, &branch, target.as_deref(), fetch);
        if let (None, Some(branch_ref), Some(target_ref)) = (&state.error, &state.branch_ref, &state.target_ref) {
            collect(root, branch_ref, target_ref, &agent_edits, &mut real, &mut edits, &mut after, &mut skipped)?;
        }
        states.push(state);
    }
    // En orden de tiempo; el punto de partida de cada fichero (sin fecha) va primero.
    edits.sort_by(|a, b| a.origin.timestamp.cmp(&b.origin.timestamp));
    let review = huella_review::build(
        &review_id(key),
        &edits,
        |edit| edit.path.display().to_string(),
        |path| after.get(path).cloned(),
    );
    Ok(TaskReview { review, repos: states, skipped })
}

/// Ediciones de todos los agentes, por la ruta real del fichero.
fn agent_edits_by_path(sources: &crate::Sources, real: &mut RealPaths) -> Result<HashMap<PathBuf, Vec<FileEdit>>> {
    let mut by_path: HashMap<PathBuf, Vec<FileEdit>> = HashMap::new();
    for agent in sources.agents() {
        for file in agent.sessions()? {
            for edit in agent.read(&file)?.edits {
                let key = real.of(Path::new(&normalize_path(&edit.path, edit.origin.cwd.as_deref())));
                by_path.entry(key).or_default().push(edit);
            }
        }
    }
    Ok(by_path)
}

/// Añade los ficheros que cambian en la rama de un repositorio: su contenido en
/// el `merge-base`, las ediciones de agentes posteriores y el de la rama.
#[allow(clippy::too_many_arguments)]
fn collect(
    repo: &Path,
    branch_ref: &str,
    target_ref: &str,
    agent_edits: &HashMap<PathBuf, Vec<FileEdit>>,
    real: &mut RealPaths,
    edits: &mut Vec<FileEdit>,
    after: &mut HashMap<String, String>,
    skipped: &mut Vec<String>,
) -> Result<()> {
    let base = git(repo, &["merge-base", target_ref, branch_ref])?.trim().to_owned();
    let since: i64 = git(repo, &["show", "-s", "--format=%ct", &base])?.trim().parse().unwrap_or(0);
    let changed = git(repo, &["diff", "--name-status", "--no-renames", &base, branch_ref])?;
    for line in changed.lines() {
        let Some((status, path)) = line.split_once('\t') else { continue };
        let absolute = real.of(&repo.join(path));
        let key = absolute.display().to_string();
        let before = (!status.starts_with('A')).then(|| git(repo, &["show", &format!("{base}:{path}")]).ok()).flatten();
        let result = if status.starts_with('D') {
            Some(String::new())
        } else {
            git(repo, &["show", &format!("{branch_ref}:{path}")]).ok()
        };
        let reviewable = |text: &String| text.len() <= MAX_FILE_BYTES && !text.contains('\0');
        let (Some(result), true) = (result, before.as_ref().is_none_or(reviewable)) else {
            skipped.push(key);
            continue;
        };
        if !reviewable(&result) {
            skipped.push(key);
            continue;
        }
        edits.push(FileEdit {
            origin: EditOrigin {
                agent: "base",
                session_id: String::new(),
                event_id: format!("base:{key}"),
                timestamp: None,
                cwd: None,
            },
            path: absolute.clone(),
            kind: if before.is_some() { EditKind::Replace } else { EditKind::Create },
            after: AfterState::Known(before.clone().unwrap_or_default()),
            before,
            user_modified: false,
            context: EditContext::default(),
        });
        for edit in agent_edits.get(&absolute).into_iter().flatten() {
            if edit.origin.timestamp.as_deref().and_then(unix_seconds).is_some_and(|at| at >= since) {
                edits.push(FileEdit { path: absolute.clone(), ..edit.clone() });
            }
        }
        after.insert(key, result);
    }
    Ok(())
}

/// Raíz del repositorio que contiene `dir`.
#[must_use]
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).ok().map(|root| PathBuf::from(root.trim()))
}

/// Rama que tiene la copia de trabajo (ninguna si está en un commit suelto).
#[must_use]
pub fn checked_out(repo: &Path) -> Option<String> {
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).ok().map(|b| b.trim().to_owned()).filter(|b| b != "HEAD")
}

/// Lo que se revisa de la rama: la local si existe; si no, la del remoto.
fn branch_ref(repo: &Path, branch: &str) -> Option<String> {
    if exists(repo, &format!("refs/heads/{branch}")) {
        Some(branch.to_owned())
    } else {
        exists(repo, &format!("refs/remotes/origin/{branch}")).then(|| format!("origin/{branch}"))
    }
}

/// Contra qué se compara: el destino del remoto (el que importa al mergear) o el local.
fn target_ref(repo: &Path, target: &str) -> Option<String> {
    if exists(repo, &format!("refs/remotes/origin/{target}")) {
        Some(format!("origin/{target}"))
    } else {
        exists(repo, &format!("refs/heads/{target}")).then(|| target.to_owned())
    }
}

fn exists(repo: &Path, reference: &str) -> bool {
    git(repo, &["rev-parse", "--verify", "--quiet", reference]).is_ok()
}

/// Commits en `to` que no están en `from`.
fn count(repo: &Path, from: &str, to: &str) -> usize {
    git(repo, &["rev-list", "--count", &format!("{from}..{to}")]).ok().and_then(|n| n.trim().parse().ok()).unwrap_or(0)
}

/// Repositorios de la tarea con su destino, completados con lo que se encuentre.
/// Devuelve los que se añaden.
///
/// # Errors
/// Si falla la base de datos.
pub fn discover(store: &Store, key: &str, extra: &[PathBuf]) -> Result<Vec<TaskRepo>> {
    let mut repos = known_repos(store)?;
    // Con la ruta que da git, la misma que la de los repositorios conocidos.
    repos.extend(
        extra
            .iter()
            .filter_map(|dir| git(dir, &["rev-parse", "--show-toplevel"]).ok())
            .map(|root| PathBuf::from(root.trim())),
    );
    repos.sort();
    repos.dedup();
    let current: BTreeMap<String, TaskRepo> =
        store.task_repos(key)?.into_iter().map(|repo| (repo.repo.clone(), repo)).collect();
    let mut added = Vec::new();
    for (repo, branch) in find_branches(&repos, key) {
        let repo = repo.display().to_string();
        if current.contains_key(&repo) {
            continue;
        }
        let found = TaskRepo { repo, branch, target: None };
        store.set_task_repo(key, &found)?;
        added.push(found);
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use huella_core::{Agent, AgentBatch, AgentError, Conversation, TranscriptCursor};

    use super::*;
    use crate::Sources;

    #[test]
    fn keys_come_from_branch_names() {
        assert_eq!(key_of("feature/DENMS-1234").as_deref(), Some("DENMS-1234"));
        assert_eq!(key_of("bugfix/denms-38-filtros").as_deref(), Some("DENMS-38"));
        assert_eq!(key_of("DAAS-7").as_deref(), Some("DAAS-7"));
        assert_eq!(
            key_of("sprint-42"),
            Some("SPRINT-42".into()),
            "el patrón no distingue; el destino se excluye por nombre"
        );
        assert_eq!(key_of("main"), None);
        assert_eq!(key_of("feature/x-1a"), None);
    }

    struct Fake(Vec<FileEdit>);

    impl Agent for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn sessions(&self) -> std::result::Result<Vec<PathBuf>, AgentError> {
            Ok(vec![PathBuf::from("sesion")])
        }
        fn read_from(&self, _: &Path, _: &TranscriptCursor) -> std::result::Result<AgentBatch, AgentError> {
            Ok(AgentBatch { edits: self.0.clone(), resume: TranscriptCursor::default() })
        }
        fn conversations(&self, _: &Path) -> std::result::Result<Vec<Conversation>, AgentError> {
            Ok(Vec::new())
        }
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Ana")
            .env("GIT_AUTHOR_EMAIL", "ana@example.com")
            .env("GIT_COMMITTER_NAME", "Ana")
            .env("GIT_COMMITTER_EMAIL", "ana@example.com")
            .env("GIT_COMMITTER_DATE", "2026-10-01T10:00:00Z")
            .env("GIT_AUTHOR_DATE", "2026-10-01T10:00:00Z")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn commit(dir: &Path, file: &str, content: &str, message: &str) {
        std::fs::write(dir.join(file), content).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-q", "-m", message]);
    }

    /// Un remoto con `main` y `sprint/42`, y un clon con la rama de la tarea
    /// sacada del sprint. Después, el sprint avanza en el remoto con un cambio
    /// que la rama también trae (se mergeó el sprint en ella), y el sprint
    /// local se queda viejo: es el caso en que aparecían cambios de más.
    fn fixture(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("huella-task-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (origin, work, other) = (root.join("origin"), root.join("api"), root.join("other"));
        std::fs::create_dir_all(&origin).unwrap();
        run(&origin, &["init", "-q", "--bare", "-b", "main"]);
        run(&root, &["clone", "-q", origin.to_str().unwrap(), "api"]);
        commit(&work, "a.ts", "uno\n", "base");
        run(&work, &["push", "-q", "origin", "main"]);
        run(&work, &["checkout", "-q", "-b", "sprint/42"]);
        commit(&work, "a.ts", "uno\nsprint\n", "sprint");
        run(&work, &["push", "-q", "origin", "sprint/42"]);
        run(&work, &["checkout", "-q", "-b", "feature/DENMS-1"]);
        commit(&work, "b.ts", "tarea\n", "tarea");
        run(&work, &["push", "-q", "-u", "origin", "feature/DENMS-1"]);
        // Otra persona añade algo al sprint en el remoto…
        run(&root, &["clone", "-q", "-b", "sprint/42", origin.to_str().unwrap(), "other"]);
        commit(&other, "c.ts", "de otra tarea\n", "otra tarea");
        run(&other, &["push", "-q", "origin", "sprint/42"]);
        // …y la rama de la tarea lo trae, mientras el sprint local sigue viejo.
        run(&work, &["fetch", "-q", "origin"]);
        run(&work, &["merge", "-q", "--no-edit", "origin/sprint/42"]);
        run(&work, &["update-ref", "refs/remotes/origin/sprint/42", "sprint/42"]);
        (root, work)
    }

    #[test]
    fn the_review_is_what_gets_merged_against_the_fresh_target() {
        let (root, work) = fixture("merge");
        // Un cambio sin commit no entra.
        std::fs::write(work.join("b.ts"), "tarea\nsin commit\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        store.save_task("DENMS-1", None, "Añade b").unwrap();
        let added = discover(&store, "DENMS-1", std::slice::from_ref(&work)).unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].branch, "feature/DENMS-1");
        let repo = PathBuf::from(&added[0].repo);
        let candidates = suggest_targets(&repo, "feature/DENMS-1", None).unwrap();
        assert_eq!(candidates[0].name, "sprint/42", "{candidates:?}");
        store.set_task_repo("DENMS-1", &TaskRepo { target: Some("sprint/42".into()), ..added[0].clone() }).unwrap();

        // Sin traer el remoto, el destino viejo mete el cambio de la otra tarea.
        let sources = Sources::new(vec![Box::new(Fake(Vec::new())) as Box<dyn Agent>]);
        let stale = task_review(&store, &sources, "DENMS-1", false).unwrap();
        let files = |review: &TaskReview| -> Vec<String> {
            review.review.files.iter().map(|f| f.path.rsplit('/').next().unwrap().to_owned()).collect()
        };
        assert_eq!(files(&stale), vec!["b.ts", "c.ts"]);
        // Con el remoto al día, solo lo de la tarea, y en su versión con commit.
        let fresh = task_review(&store, &sources, "DENMS-1", true).unwrap();
        assert_eq!(files(&fresh), vec!["b.ts"]);
        assert_eq!(fresh.review.files[0].after, "tarea\n");
        let api = &fresh.repos[0];
        assert_eq!((api.error.as_deref(), api.uncommitted.len()), (None, 1));
        assert!(api.warnings().iter().any(|w| w.contains("sin commit")), "{:?}", api.warnings());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unpushed_work_and_missing_targets_are_reported() {
        let (root, work) = fixture("sync");
        run(&work, &["push", "-q", "origin", "feature/DENMS-1"]);
        commit(&work, "b.ts", "tarea\nmás\n", "sin subir");
        let state = sync(&work, "feature/DENMS-1", None, false);
        assert_eq!(state.unpushed, 1);
        assert!(state.error.as_deref().is_some_and(|e| e.contains("falta elegir")));
        let state = sync(&work, "feature/DENMS-1", Some("sprint/99"), false);
        assert!(state.error.as_deref().is_some_and(|e| e.contains("sprint/99")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
