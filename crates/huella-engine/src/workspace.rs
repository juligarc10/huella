//! Worktree de revisión: la sesión preparada para revisarse con las
//! herramientas de git del editor (en Zed, su vista de cambios).
//!
//! En un worktree aparte del repositorio, en una rama temporal, se hace un
//! commit con el estado de cada fichero al empezar la sesión y se deja su
//! estado final como cambios sin commitear. El diff de git es así exactamente
//! lo que hizo la sesión; preparar (stage) un fichero entero lo marca como
//! revisado. El worktree vive en `.claude/worktrees/`, cuyas rutas Huella ya
//! lleva al repositorio principal, así que la memoria y los avisos funcionan.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use huella_review::flow::FlowStep;
use huella_review::{FileStatus, SessionReview};
use huella_store::{ReviewWorkspace, Store};

use crate::{EngineError, Result};

/// Identidad de los commits internos: no depende de la configuración de git
/// de la persona, y `commit-tree` no ejecuta sus hooks.
const AUTHOR: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Huella"),
    ("GIT_AUTHOR_EMAIL", "huella@localhost"),
    ("GIT_COMMITTER_NAME", "Huella"),
    ("GIT_COMMITTER_EMAIL", "huella@localhost"),
];

/// Resultado de preparar el worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedWorkspace {
    pub workspace: ReviewWorkspace,
    /// Ficheros de la sesión incluidos (los del repositorio).
    pub files: usize,
    /// Ficheros de la sesión fuera del repositorio, que no se incluyen.
    pub outside: Vec<String>,
    /// Ya existía: se reutiliza tal cual.
    pub reused: bool,
}

/// Prepara (o reutiliza) el worktree de revisión de la sesión.
///
/// # Errors
/// Si la sesión no tiene un repositorio git o falla git.
pub fn prepare_workspace(store: &Store, review: &SessionReview) -> Result<PreparedWorkspace> {
    let session = &review.session_id;
    let short = &session[..8.min(session.len())];
    if let Some(workspace) = store.review_workspace(session)?
        && Path::new(&workspace.path).exists()
    {
        let files = review.files.iter().filter(|f| f.path.starts_with(&workspace.repo)).count();
        return Ok(PreparedWorkspace { workspace, files, outside: Vec::new(), reused: true });
    }
    let repo = crate::session_project(store, session)?
        .filter(|project| Path::new(project).join(".git").exists())
        .ok_or_else(|| EngineError::Git(format!("la sesión {short} no tiene un repositorio git claro")))?;
    let repo_path = PathBuf::from(&repo);
    let path = repo_path.join(".claude").join("worktrees").join(format!("huella-review-{short}"));
    let branch = format!("huella/review-{short}");
    ignore_review_worktrees(&repo_path)?;
    git(&repo_path, &["worktree", "add", "--quiet", "-b", &branch, path_str(&path)?, "HEAD"])?;

    let mut included = Vec::new();
    let mut outside = Vec::new();
    for file in &review.files {
        match Path::new(&file.path).strip_prefix(&repo_path) {
            Ok(relative) => included.push((relative.to_path_buf(), file)),
            Err(_) => outside.push(file.path.clone()),
        }
    }

    // Base: el estado de cada fichero al empezar la sesión.
    for (relative, file) in &included {
        let target = path.join(relative);
        if file.status == FileStatus::Created {
            let _ = std::fs::remove_file(&target);
        } else {
            write(&target, &file.before)?;
        }
    }
    // Lo que existía al empezar se añade; lo que creó la sesión se quita del
    // índice por si estaba versionado (si no, no hay nada que quitar).
    let (existing, created): (Vec<_>, Vec<_>) =
        included.iter().partition(|(_, file)| file.status != FileStatus::Created);
    for (paths, base) in
        [(existing, vec!["add", "--all", "--"]), (created, vec!["rm", "--cached", "--quiet", "--ignore-unmatch", "--"])]
    {
        if paths.is_empty() {
            continue;
        }
        let relatives: Vec<String> = paths.iter().map(|(relative, _)| relative.display().to_string()).collect();
        let mut args = base;
        args.extend(relatives.iter().map(String::as_str));
        git(&path, &args)?;
    }
    let tree = git(&path, &["write-tree"])?;
    let message = format!("Huella: estado al empezar la sesión {short}");
    let commit = git(&path, &["commit-tree", tree.trim(), "-p", "HEAD", "-m", &message])?;
    git(&path, &["reset", "--quiet", commit.trim()])?;

    // Cambios sin commitear: el estado final de la sesión.
    for (relative, file) in &included {
        write(&path.join(relative), &file.after)?;
    }

    let workspace = ReviewWorkspace { session_id: session.clone(), repo, path: path_str(&path)?.to_owned(), branch };
    store.save_review_workspace(&workspace)?;
    Ok(PreparedWorkspace { workspace, files: included.len(), outside, reused: false })
}

/// Marca como revisados los pasos de los ficheros que se han preparado
/// enteros en git. Devuelve cuántos pasos se marcaron ahora.
///
/// # Errors
/// Si falla git o la base de datos.
pub fn sync_workspace_marks(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    workspace: &ReviewWorkspace,
) -> Result<usize> {
    let path = Path::new(&workspace.path);
    let lines =
        |args: &[&str]| -> Result<BTreeSet<String>> { Ok(git(path, args)?.lines().map(str::to_owned).collect()) };
    let staged = lines(&["diff", "--cached", "--name-only"])?;
    let pending: BTreeSet<String> = lines(&["diff", "--name-only"])?
        .into_iter()
        .chain(lines(&["ls-files", "--others", "--exclude-standard"])?)
        .collect();
    let marks = store.review_marks(&review.session_id)?;
    let mut marked = 0;
    for step in plan {
        let file = &review.files[step.file];
        let Some(relative) = file.path.strip_prefix(&workspace.repo).map(|r| r.trim_start_matches('/')) else {
            continue;
        };
        if !staged.contains(relative) || pending.contains(relative) {
            continue;
        }
        let (key, hash) = (step.key(review), step.content_hash());
        if marks.get(&key) != Some(&hash) {
            store.set_review_mark(&review.session_id, &key, &hash, true)?;
            marked += 1;
        }
    }
    Ok(marked)
}

/// Borra el worktree y su rama temporal. Devuelve si había alguno.
///
/// # Errors
/// Si falla git o la base de datos.
pub fn remove_workspace(store: &Store, session_id: &str) -> Result<bool> {
    let Some(workspace) = store.review_workspace(session_id)? else { return Ok(false) };
    let repo = Path::new(&workspace.repo);
    if Path::new(&workspace.path).exists() {
        git(repo, &["worktree", "remove", "--force", &workspace.path])?;
    }
    // La rama solo contiene el commit interno de la revisión.
    let _ = git(repo, &["branch", "-D", &workspace.branch]);
    store.delete_review_workspace(session_id)?;
    Ok(true)
}

/// Que `.claude/worktrees/` no aparezca como cambio en el repositorio
/// principal: se añade a `.git/info/exclude`, local y sin versionar.
fn ignore_review_worktrees(repo: &Path) -> Result<()> {
    let probe = ".claude/worktrees/huella-review";
    if git(repo, &["check-ignore", "-q", probe]).is_ok() {
        return Ok(());
    }
    let common = git(repo, &["rev-parse", "--git-common-dir"])?;
    let common = repo.join(common.trim());
    let exclude = common.join("info").join("exclude");
    let mut content = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("# Worktrees de revisión de Huella\n/.claude/worktrees/\n");
    write(&exclude, &content)
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(AUTHOR)
        .output()
        .map_err(|error| EngineError::Git(format!("no se pudo ejecutar git: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EngineError::Git(format!("git {}: {}", args.join(" "), stderr.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| EngineError::Git(format!("{}: {error}", parent.display())))?;
    }
    std::fs::write(path, content).map_err(|error| EngineError::Git(format!("{}: {error}", path.display())))
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| EngineError::Git(format!("ruta no válida: {}", path.display())))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};
    use huella_store::{IndexedEdit, Store};

    use super::*;

    fn run(dir: &Path, args: &[&str]) -> String {
        git(dir, args).unwrap()
    }

    /// Repositorio con `a.ts` commiteado y una sesión que lo cambia y crea `b.ts`.
    fn setup(name: &str) -> (PathBuf, Store, SessionReview) {
        let repo = std::env::temp_dir().join(format!("huella-ws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let repo = repo.canonicalize().unwrap();
        run(&repo, &["init", "--quiet", "-b", "main"]);
        std::fs::write(repo.join("a.ts"), "export const a = 1;\n").unwrap();
        run(&repo, &["add", "a.ts"]);
        run(&repo, &["commit", "--quiet", "--no-verify", "-m", "inicio"]);

        let edit = |path: &str, before: Option<&str>, after: &str| FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "abcdef1234".into(),
                event_id: path.into(),
                timestamp: None,
                cwd: None,
            },
            path: repo.join(path),
            kind: if before.is_some() { EditKind::Replace } else { EditKind::Create },
            before: before.map(str::to_owned),
            after: AfterState::Known(after.into()),
            user_modified: false,
            context: EditContext { prompt: Some("cambia a y crea b".into()), rationale: None },
        };
        let edits = [
            edit("a.ts", Some("export const a = 1;\n"), "export const a = 2;\n"),
            edit("b.ts", None, "export const b = 1;\n"),
        ];
        let mut store = Store::open_in_memory().unwrap();
        for e in &edits {
            let path = e.path.display().to_string();
            let outcome = huella_core::AnchorOutcome::FileOnly;
            let indexed = [IndexedEdit { edit: e, path: &path, outcome: &outcome, changed_lines: &[] }];
            store.save(&format!("/t/{path}"), &huella_core::TranscriptCursor::default(), &indexed).unwrap();
        }
        let review = huella_review::build("abcdef1234", &edits, |e| e.path.display().to_string(), |_| None);
        (repo, store, review)
    }

    #[test]
    fn the_worktree_diff_is_exactly_the_session_and_staging_marks_files_reviewed() {
        let (repo, store, review) = setup("flow");
        let prepared = prepare_workspace(&store, &review).unwrap();
        assert_eq!((prepared.files, prepared.reused), (2, false));
        let path = PathBuf::from(&prepared.workspace.path);
        assert!(path.starts_with(repo.join(".claude/worktrees")));

        assert_eq!(run(&path, &["diff", "--name-only"]).trim(), "a.ts");
        assert_eq!(run(&path, &["ls-files", "--others", "--exclude-standard"]).trim(), "b.ts");
        assert!(run(&repo, &["status", "--porcelain"]).is_empty(), "el checkout principal no cambia");

        let plan = huella_review::flow::plan(&review);
        assert_eq!(sync_workspace_marks(&store, &review, &plan, &prepared.workspace).unwrap(), 0);
        run(&path, &["add", "a.ts"]);
        let marked = sync_workspace_marks(&store, &review, &plan, &prepared.workspace).unwrap();
        assert_eq!(marked, plan.iter().filter(|s| review.files[s.file].path.ends_with("a.ts")).count());

        assert!(prepare_workspace(&store, &review).unwrap().reused);
        assert!(remove_workspace(&store, &review.session_id).unwrap());
        assert!(!path.exists());
        assert!(!run(&repo, &["branch", "--list", "huella/*"]).contains("huella/review"));
        std::fs::remove_dir_all(&repo).unwrap();
    }
}
