//! Revisión de una rama: lo que se va a entregar, no una sesión suelta.
//!
//! El diff es el de la rama contra su base (desde el `merge-base`, con lo que
//! aún no está en un commit) y cada línea añadida se atribuye a la sesión que
//! la escribió, de cualquier agente, o a la persona que la cambió a mano.
//!
//! Para atribuir se reutiliza la historia de líneas de la revisión: por cada
//! fichero se parte del contenido en el `merge-base`, se aplican en orden todas
//! las ediciones de agentes posteriores y se termina en el fichero actual. Lo
//! que cambia fuera de las ediciones es «a mano», y `git blame` dice de quién.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};
use huella_review::{DiffLine, Step};
use serde::Serialize;

use crate::paths::normalize_path;
use crate::workspace::git;
use crate::{EngineError, Result};

/// Ficheros más grandes no se atribuyen línea a línea.
const MAX_FILE_BYTES: usize = 1024 * 1024;
const UNCOMMITTED: &str = "Sin commit";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchReview {
    /// Raíz del repositorio.
    pub repo: String,
    pub branch: Option<String>,
    pub base: String,
    pub merge_base: String,
    pub files: Vec<BranchFile>,
    /// Sesiones que escribieron alguna línea de la rama, de más a menos líneas.
    pub sessions: Vec<BranchSession>,
    /// Líneas cambiadas a mano, por persona (o sin commit).
    pub people: Vec<(String, usize)>,
    /// Ficheros que no se atribuyen (binarios o demasiado grandes).
    pub skipped: Vec<String>,
    /// Copias de trabajo del repositorio (los agentes suelen usar worktrees).
    pub worktrees: Vec<Worktree>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchFile {
    /// Relativa a la raíz del repositorio.
    pub path: String,
    pub status: &'static str,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<BranchHunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<BranchLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BranchLine {
    Context { text: String },
    Removed { text: String },
    Added { text: String, origin: LineOrigin },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum LineOrigin {
    /// La escribió un agente en esta sesión.
    Session { session_id: String },
    /// Cambió fuera de las ediciones del agente (scripts, `sed`, formateadores)
    /// pero está en un commit que hizo esta sesión por terminal.
    Terminal { session_id: String },
    /// Cambiada a mano, fuera de las sesiones.
    Manual { author: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchSession {
    pub session_id: String,
    pub agent: &'static str,
    /// Líneas que escribió con sus ediciones.
    pub lines: usize,
    /// Líneas que cambió por terminal y confirmó en un commit.
    pub terminal_lines: usize,
    pub files: usize,
    pub first_at: Option<String>,
    pub last_at: Option<String>,
    /// Primera petición de la sesión que tocó la rama.
    pub prompt: Option<String>,
}

/// Revisión de la rama actual de `dir` contra `base` (por defecto, la rama
/// principal del remoto, `main` o `master`).
///
/// # Errors
/// Si `dir` no es un repositorio, la base no existe o falla la lectura de las sesiones.
pub fn branch_review(sources: &crate::Sources, dir: &Path, base: Option<&str>) -> Result<BranchReview> {
    let repo = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?.trim());
    let base = match base {
        Some(base) => base.to_owned(),
        None => default_base(&repo)?,
    };
    let merge_base = git(&repo, &["merge-base", &base, "HEAD"])?.trim().to_owned();
    let since: i64 = git(&repo, &["show", "-s", "--format=%ct", &merge_base])?.trim().parse().unwrap_or(0);
    let branch =
        git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).ok().map(|b| b.trim().to_owned()).filter(|b| b != "HEAD");

    let mut changed: Vec<(char, String)> = git(&repo, &["diff", "--name-status", "--no-renames", &merge_base])?
        .lines()
        .filter_map(|line| {
            line.split_once('\t').map(|(status, path)| (status.chars().next().unwrap_or('M'), path.to_owned()))
        })
        .collect();
    changed.extend(
        git(&repo, &["ls-files", "--others", "--exclude-standard"])?.lines().map(|path| ('A', path.to_owned())),
    );

    // Ediciones de agentes posteriores al merge-base, por fichero real.
    let mut real = RealPaths::default();
    let mut by_path: HashMap<PathBuf, Vec<FileEdit>> = HashMap::new();
    for agent in sources.agents() {
        for file in agent.sessions()? {
            for edit in agent.read(&file)?.edits {
                if edit.origin.timestamp.as_deref().and_then(unix_seconds).is_some_and(|at| at >= since) {
                    let key = real.of(Path::new(&normalize_path(&edit.path, edit.origin.cwd.as_deref())));
                    by_path.entry(key).or_default().push(edit);
                }
            }
        }
    }

    let session_ids: BTreeSet<&str> = by_path.values().flatten().map(|edit| edit.origin.session_id.as_str()).collect();
    let session_commits = commits_by_session(sources, &repo, &merge_base, &session_ids)?;

    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut sessions: BTreeMap<String, BranchSession> = BTreeMap::new();
    let mut people: BTreeMap<String, usize> = BTreeMap::new();
    for (status, path) in changed {
        let before = if status == 'A' { None } else { git(&repo, &["show", &format!("{merge_base}:{path}")]).ok() };
        let after = if status == 'D' { Some(String::new()) } else { std::fs::read_to_string(repo.join(&path)).ok() };
        let Some(after) = after.filter(|text| text.len() <= MAX_FILE_BYTES && !text.contains('\0')) else {
            skipped.push(path);
            continue;
        };
        if before.as_ref().is_some_and(|text| text.len() > MAX_FILE_BYTES || text.contains('\0')) {
            skipped.push(path);
            continue;
        }
        let key = real.of(Path::new(&normalize_path(&repo.join(&path), None)));
        let mut edits = by_path.remove(&key).unwrap_or_default();
        edits.sort_by(|a, b| a.origin.timestamp.cmp(&b.origin.timestamp));
        let found =
            attribute(&repo, &path, status, before, &after, &edits, &session_commits, &mut sessions, &mut people);
        if let Some(file) = found {
            files.push(file);
        }
    }

    let mut sessions: Vec<BranchSession> = sessions.into_values().collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.lines + session.terminal_lines));
    let mut people: Vec<(String, usize)> = people.into_iter().collect();
    people.sort_by_key(|(_, lines)| std::cmp::Reverse(*lines));
    Ok(BranchReview {
        repo: repo.display().to_string(),
        branch,
        base,
        merge_base: merge_base.chars().take(10).collect(),
        files,
        sessions,
        people,
        skipped,
        worktrees: worktrees(&repo),
    })
}

/// Copias de trabajo del repositorio, la principal primero.
fn worktrees(repo: &Path) -> Vec<Worktree> {
    let Ok(listing) = git(repo, &["worktree", "list", "--porcelain"]) else { return Vec::new() };
    let mut found: Vec<Worktree> = Vec::new();
    for line in listing.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            found.push(Worktree { path: path.to_owned(), branch: None });
        } else if let (Some(name), Some(last)) = (line.strip_prefix("branch "), found.last_mut()) {
            last.branch = Some(name.trim_start_matches("refs/heads/").to_owned());
        }
    }
    found
}

/// Diff de un fichero con cada línea añadida atribuida.
#[allow(clippy::too_many_arguments)]
fn attribute(
    repo: &Path,
    path: &str,
    status: char,
    before: Option<String>,
    after: &str,
    edits: &[FileEdit],
    session_commits: &HashMap<String, (String, &'static str)>,
    sessions: &mut BTreeMap<String, BranchSession>,
    people: &mut BTreeMap<String, usize>,
) -> Option<BranchFile> {
    // Ediciones que no guardaron el fichero original: se sabe qué texto
    // insertaron, y una línea con ese texto exacto es suya.
    let mut fragments: HashMap<&str, &FileEdit> = HashMap::new();
    for edit in edits {
        if let AfterState::FragmentOnly(text) = &edit.after {
            for line in text.lines().map(str::trim).filter(|line| line.chars().count() >= 4) {
                fragments.insert(line, edit);
            }
        }
    }
    // Una edición sintética con el contenido del merge-base: es el punto de partida.
    let start = FileEdit {
        origin: EditOrigin {
            agent: "base",
            session_id: String::new(),
            event_id: String::new(),
            timestamp: None,
            cwd: None,
        },
        path: PathBuf::from(path),
        kind: if before.is_some() { EditKind::Replace } else { EditKind::Create },
        after: AfterState::Known(before.clone().unwrap_or_default()),
        before,
        user_modified: false,
        context: EditContext::default(),
    };
    let all: Vec<FileEdit> = std::iter::once(start).chain(edits.iter().cloned()).collect();
    let review = huella_review::build("rama", &all, |_| path.to_owned(), |_| Some(after.to_owned()));
    let file = review.files.into_iter().next()?;
    let event_session: HashMap<&str, &FileEdit> =
        edits.iter().map(|edit| (edit.origin.event_id.as_str(), edit)).collect();
    let mut blame: Option<HashMap<usize, (String, String)>> = None;
    let (mut added, mut removed) = (0, 0);
    let mut touched_sessions: Vec<String> = Vec::new();

    let hunks = file
        .hunks
        .iter()
        .map(|hunk| {
            let mut new_line = hunk.new_start;
            let lines = hunk
                .lines
                .iter()
                .map(|line| match line {
                    DiffLine::Context(text) => {
                        new_line += 1;
                        BranchLine::Context { text: text.clone() }
                    }
                    DiffLine::Removed(text) => {
                        removed += 1;
                        BranchLine::Removed { text: text.clone() }
                    }
                    DiffLine::Added { text, source } => {
                        added += 1;
                        let edit = source
                            .and_then(|s| match &review.steps[s] {
                                Step::Agent { event_id, .. } => event_session.get(event_id.as_str()).copied(),
                                Step::External { .. } => None,
                            })
                            .or_else(|| fragments.get(text.trim()).copied());
                        let origin = if let Some(edit) = edit {
                            let first_in_file = !touched_sessions.contains(&edit.origin.session_id);
                            if first_in_file {
                                touched_sessions.push(edit.origin.session_id.clone());
                            }
                            credit_edit(sessions, edit, first_in_file)
                        } else {
                            let authors = blame.get_or_insert_with(|| blame_authors(repo, path));
                            let (commit, author) = authors
                                .get(&new_line)
                                .cloned()
                                .unwrap_or_else(|| (String::new(), UNCOMMITTED.to_owned()));
                            credit_outside_edits(sessions, people, session_commits.get(&commit), author)
                        };
                        new_line += 1;
                        BranchLine::Added { text: text.clone(), origin }
                    }
                })
                .collect();
            BranchHunk { old_start: hunk.old_start, new_start: hunk.new_start, lines }
        })
        .collect();
    let status = match status {
        'A' => "added",
        'D' => "deleted",
        _ => "modified",
    };
    Some(BranchFile { path: path.to_owned(), status, added, removed, hunks })
}

/// Ruta sin enlaces simbólicos (en macOS `/var` es `/private/var`): git da la
/// real y los agentes la que usaron. Se resuelve la carpeta, que existe aunque
/// el fichero se haya borrado, y se guarda para no repetirlo en cada edición.
#[derive(Default)]
struct RealPaths(HashMap<PathBuf, PathBuf>);

impl RealPaths {
    fn of(&mut self, path: &Path) -> PathBuf {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return path.to_owned() };
        let parent = self
            .0
            .entry(parent.to_owned())
            .or_insert_with(|| std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_owned()));
        parent.join(name)
    }
}

/// Apunta a su sesión una línea que escribió una edición del agente.
fn credit_edit(sessions: &mut BTreeMap<String, BranchSession>, edit: &FileEdit, first_in_file: bool) -> LineOrigin {
    let entry = sessions
        .entry(edit.origin.session_id.clone())
        .or_insert_with(|| empty_session(&edit.origin.session_id, edit.origin.agent));
    entry.lines += 1;
    entry.last_at = entry.last_at.clone().max(edit.origin.timestamp.clone());
    // La petición que se muestra es la de su primera edición en la rama.
    if entry.first_at.is_none() || edit.origin.timestamp < entry.first_at {
        entry.first_at.clone_from(&edit.origin.timestamp);
        entry.prompt.clone_from(&edit.context.prompt);
    }
    if first_in_file {
        entry.files += 1;
    }
    LineOrigin::Session { session_id: edit.origin.session_id.clone() }
}

/// Una línea que cambió fuera de las ediciones: de la sesión que hizo su
/// commit por terminal, o de quien la escribió a mano.
fn credit_outside_edits(
    sessions: &mut BTreeMap<String, BranchSession>,
    people: &mut BTreeMap<String, usize>,
    session_commit: Option<&(String, &'static str)>,
    author: String,
) -> LineOrigin {
    if let Some((session_id, agent)) = session_commit {
        sessions.entry(session_id.clone()).or_insert_with(|| empty_session(session_id, agent)).terminal_lines += 1;
        LineOrigin::Terminal { session_id: session_id.clone() }
    } else {
        *people.entry(author.clone()).or_default() += 1;
        LineOrigin::Manual { author }
    }
}

fn empty_session(session_id: &str, agent: &'static str) -> BranchSession {
    BranchSession {
        session_id: session_id.to_owned(),
        agent,
        lines: 0,
        terminal_lines: 0,
        files: 0,
        first_at: None,
        last_at: None,
        prompt: None,
    }
}

/// Commit y autor de cada línea del fichero de trabajo (1-based). Las que no
/// están en ningún commit quedan como «Sin commit».
fn blame_authors(repo: &Path, path: &str) -> HashMap<usize, (String, String)> {
    let Ok(porcelain) = git(repo, &["blame", "--porcelain", "--", path]) else { return HashMap::new() };
    let mut authors_by_commit: HashMap<String, String> = HashMap::new();
    let mut result = HashMap::new();
    let mut current: Option<(String, usize)> = None;
    for line in porcelain.lines() {
        if let Some(name) = line.strip_prefix("author ") {
            if let Some((commit, _)) = &current {
                authors_by_commit.insert(commit.clone(), name.to_owned());
            }
        } else if line.starts_with('\t') {
            if let Some((commit, final_line)) = current.take() {
                let author = if commit.chars().all(|c| c == '0') {
                    UNCOMMITTED.to_owned()
                } else {
                    authors_by_commit.get(&commit).cloned().unwrap_or_default()
                };
                result.insert(final_line, (commit, author));
            }
        } else {
            let mut parts = line.split(' ');
            if let (Some(commit), Some(_), Some(final_line)) = (parts.next(), parts.next(), parts.next())
                && commit.len() >= 40
                && let Ok(final_line) = final_line.parse()
            {
                current = Some((commit.to_owned(), final_line));
            }
        }
    }
    result
}

/// Commits de la rama que hizo una sesión: los que se crearon justo cuando
/// la sesión ejecutó `git commit`. Commit → (sesión, agente).
fn commits_by_session(
    sources: &crate::Sources,
    repo: &Path,
    merge_base: &str,
    sessions: &BTreeSet<&str>,
) -> Result<HashMap<String, (String, &'static str)>> {
    let commits: Vec<(String, i64)> = git(repo, &["log", "--format=%H %ct", &format!("{merge_base}..HEAD")])?
        .lines()
        .filter_map(|line| line.split_once(' ').and_then(|(sha, at)| Some((sha.to_owned(), at.parse().ok()?))))
        .collect();
    if commits.is_empty() || sessions.is_empty() {
        return Ok(HashMap::new());
    }
    let mut runs: Vec<(i64, String, &'static str)> = Vec::new();
    for agent in sources.agents() {
        for file in agent.sessions()? {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            if !sessions.iter().any(|session| text.contains(session)) {
                continue;
            }
            for activity in agent.activity(&file)? {
                if let huella_core::ActivityKind::Command { command, .. } = &activity.kind
                    && command.contains("git commit")
                    && sessions.contains(activity.session_id.as_str())
                    && let Some(at) = activity.at.as_deref().and_then(unix_seconds)
                {
                    runs.push((at, activity.session_id.clone(), agent.name()));
                }
            }
        }
    }
    // El commit se crea en los segundos siguientes a lanzar la orden.
    Ok(commits
        .into_iter()
        .filter_map(|(sha, at)| {
            runs.iter()
                .filter(|(run, _, _)| (run - 5..=run + 300).contains(&at))
                .min_by_key(|(run, _, _)| (at - run).abs())
                .map(|(_, session, agent)| (sha, (session.clone(), *agent)))
        })
        .collect())
}

/// Rama principal del remoto si la hay; si no, `main` o `master`.
fn default_base(repo: &Path) -> Result<String> {
    if let Ok(head) = git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        return Ok(head.trim().to_owned());
    }
    ["main", "master"]
        .into_iter()
        .find(|name| git(repo, &["rev-parse", "--verify", "--quiet", name]).is_ok())
        .map(str::to_owned)
        .ok_or_else(|| EngineError::Git("no se encontró la rama base: indícala (p. ej. main)".into()))
}

/// Segundos Unix de una fecha RFC 3339 (`2026-10-27T10:00:00.123Z` o con
/// desfase `+02:00`), sin depender de una biblioteca de fechas.
fn unix_seconds(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let mut ymd = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let (clock, offset) = match time.find(['Z', '+', '-']) {
        Some(at) => (&time[..at], &time[at..]),
        None => (time, "Z"),
    };
    let mut hms = clock.split(':');
    let hours: i64 = hms.next()?.parse().ok()?;
    let minutes: i64 = hms.next()?.parse().ok()?;
    let seconds: i64 = hms.next().unwrap_or("0").split('.').next()?.parse().ok()?;
    let offset_seconds = match offset.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let (h, m) = offset[1..].split_once(':').unwrap_or((&offset[1..], "0"));
            let value = h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60;
            if sign == '+' { value } else { -value }
        }
        _ => 0,
    };
    // Días desde 1970-01-01 (algoritmo de Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hours * 3600 + minutes * 60 + seconds - offset_seconds)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use huella_core::{Agent, AgentBatch, AgentError, Conversation, TranscriptCursor};

    use super::*;
    use crate::Sources;

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

    fn run(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
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

    #[test]
    fn each_added_line_is_attributed_to_its_session_or_to_a_person() {
        let repo = std::env::temp_dir().join(format!("huella-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.ts"), "uno\n").unwrap();
        run(&repo, &["add", "a.ts"]);
        run(&repo, &["commit", "-q", "-m", "base"]);
        run(&repo, &["checkout", "-q", "-b", "feature"]);

        // Un agente añade una línea…
        let agent_edit = FileEdit {
            origin: EditOrigin {
                agent: "fake",
                session_id: "sesion-1".into(),
                event_id: "e1".into(),
                timestamp: Some("2026-10-02T10:00:00Z".into()),
                cwd: None,
            },
            path: repo.join("a.ts"),
            kind: EditKind::Replace,
            before: Some("uno\n".into()),
            after: AfterState::Known("uno\ndos\n".into()),
            user_modified: false,
            context: EditContext { prompt: Some("añade dos".into()), rationale: None },
        };
        // …y la persona añade otra a mano, sin commit, más un fichero nuevo.
        std::fs::write(repo.join("a.ts"), "uno\ndos\ntres\n").unwrap();
        std::fs::write(repo.join("b.ts"), "nuevo\n").unwrap();

        let sources = Sources::new(vec![Box::new(Fake(vec![agent_edit])) as Box<dyn Agent>]);
        let review = branch_review(&sources, &repo, Some("main")).unwrap();
        assert_eq!(review.branch.as_deref(), Some("feature"));
        let a = review.files.iter().find(|f| f.path == "a.ts").unwrap();
        let origins: Vec<&LineOrigin> = a
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter_map(|line| match line {
                BranchLine::Added { origin, .. } => Some(origin),
                _ => None,
            })
            .collect();
        assert_eq!(
            origins,
            vec![
                &LineOrigin::Session { session_id: "sesion-1".into() },
                &LineOrigin::Manual { author: UNCOMMITTED.into() }
            ]
        );
        assert_eq!(review.sessions.len(), 1);
        assert_eq!((review.sessions[0].lines, review.sessions[0].prompt.as_deref()), (1, Some("añade dos")));
        assert!(review.files.iter().any(|f| f.path == "b.ts" && f.status == "added"));
        assert_eq!(review.people, vec![(UNCOMMITTED.to_owned(), 2)]);
        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn dates_with_offsets_compare_as_instants() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_seconds("2026-10-27T10:00:00.123Z"), unix_seconds("2026-10-27T12:00:00+02:00"));
        assert_eq!(unix_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
    }
}
