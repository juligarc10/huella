//! Las revisiones de una tarea a lo largo del tiempo: cuando el agente sigue
//! trabajando, ver solo lo que cambió desde un momento, con el resto de contexto.
//!
//! Un punto es un momento con el commit que tenía entonces cada repositorio. Se
//! guarda uno cada vez que se abre la tarea y la rama ha avanzado, y además se
//! ofrecen los commits de la rama. Lo nuevo se calcula comparando el contenido, no
//! los commits: sigue funcionando si el agente los reescribe.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use huella_review::flow::FlowStep;
use huella_review::{DiffLine, SessionReview};
use huella_store::Store;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::task::RepoSync;
use crate::workspace::git;

/// Puntos que se ofrecen como mucho.
const MAX_POINTS: usize = 40;
/// Líneas quitadas que se listan por paso.
const MAX_GONE: usize = 200;

/// El commit que tenía un repositorio en un momento.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub repo: String,
    pub sha: String,
}

/// Un momento desde el que se puede mirar lo nuevo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Point {
    pub id: String,
    /// Fecha ISO 8601.
    pub at: String,
    /// `seen`: cuando se abrió la tarea; `commit`: un commit de la rama.
    pub kind: &'static str,
    /// El mensaje del commit.
    pub label: Option<String>,
    pub heads: Vec<Head>,
    #[serde(skip)]
    order: i64,
}

fn current_heads(repos: &[RepoSync]) -> Vec<Head> {
    repos
        .iter()
        .filter_map(|repo| {
            let reference = repo.branch_ref.as_deref()?;
            let sha = git(Path::new(&repo.repo), &["rev-parse", reference]).ok()?;
            Some(Head { repo: repo.repo.clone(), sha: sha.trim().to_owned() })
        })
        .collect()
}

/// Guarda el momento si la rama ha avanzado desde el último guardado.
///
/// # Errors
/// Si falla la base de datos.
pub fn record(store: &Store, review_id: &str, repos: &[RepoSync]) -> Result<()> {
    let heads = current_heads(repos);
    if heads.is_empty() {
        return Ok(());
    }
    let text = serde_json::to_string(&heads).unwrap_or_default();
    let same = store.revisions(review_id)?.last().is_some_and(|last| last.heads == text);
    if !same {
        store.add_revision(review_id, &text)?;
    }
    Ok(())
}

/// El commit que tenía la rama en una fecha, o su punto de partida si es anterior.
fn head_at(repo: &RepoSync, unix: i64) -> Option<Head> {
    let dir = Path::new(&repo.repo);
    let reference = repo.branch_ref.as_deref()?;
    let target = repo.target_ref.as_deref()?;
    let base = git(dir, &["merge-base", target, reference]).ok()?;
    let range = format!("{}..{reference}", base.trim());
    let before = format!("--before={unix}");
    let found = git(dir, &["log", "-1", "--format=%H", &before, &range]).ok()?;
    let sha = Some(found.trim()).filter(|sha| !sha.is_empty()).unwrap_or(base.trim());
    Some(Head { repo: repo.repo.clone(), sha: sha.to_owned() })
}

/// Los momentos desde los que hay algo nuevo que ver, del más reciente al más antiguo.
///
/// # Errors
/// Si falla la base de datos.
pub fn points(store: &Store, review_id: &str, repos: &[RepoSync]) -> Result<Vec<Point>> {
    let now = current_heads(repos);
    let mut points = Vec::new();
    for revision in store.revisions(review_id)? {
        let Ok(heads) = serde_json::from_str::<Vec<Head>>(&revision.heads) else { continue };
        if heads == now {
            continue;
        }
        let order = crate::branch::unix_seconds(&revision.recorded_at).unwrap_or_default();
        points.push(Point {
            id: format!("seen:{}", revision.id),
            at: revision.recorded_at,
            kind: "seen",
            label: None,
            heads,
            order,
        });
    }
    for repo in repos {
        let dir = Path::new(&repo.repo);
        let (Some(reference), Some(target)) = (repo.branch_ref.as_deref(), repo.target_ref.as_deref()) else {
            continue;
        };
        let Ok(base) = git(dir, &["merge-base", target, reference]) else { continue };
        let range = format!("{}..{reference}", base.trim());
        let Ok(log) = git(dir, &["log", "--format=%H%x09%ct%x09%cI%x09%s", &range]) else { continue };
        // El último commit es el presente: desde él no hay nada nuevo.
        for line in log.lines().skip(1) {
            let mut parts = line.splitn(4, '\t');
            let (Some(sha), Some(unix), Some(at), Some(subject)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let order: i64 = unix.parse().unwrap_or_default();
            let heads = repos
                .iter()
                .filter_map(|other| {
                    if other.repo == repo.repo {
                        Some(Head { repo: other.repo.clone(), sha: sha.to_owned() })
                    } else {
                        head_at(other, order)
                    }
                })
                .collect();
            points.push(Point {
                id: format!("commit:{sha}"),
                at: at.to_owned(),
                kind: "commit",
                label: Some(subject.to_owned()),
                heads,
                order,
            });
        }
    }
    points.sort_by_key(|point| std::cmp::Reverse(point.order));
    // Un momento guardado y un commit con el mismo contenido son el mismo punto.
    let mut seen = BTreeSet::new();
    points.retain(|point| seen.insert(serde_json::to_string(&point.heads).unwrap_or_default()));
    points.truncate(MAX_POINTS);
    Ok(points)
}

/// Lo nuevo de un paso desde un momento.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StepChange {
    pub step: usize,
    /// Índices en las líneas del paso.
    pub fresh: Vec<usize>,
    /// Líneas que estaban entonces y ya no: no salen en el diff contra el destino.
    pub gone: Vec<String>,
}

/// Lo que cambió desde `heads`. Un repositorio sin punto no cuenta como cambiado.
#[must_use]
pub fn changes(review: &SessionReview, plan: &[FlowStep], heads: &[Head]) -> Vec<StepChange> {
    let mut steps_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, step) in plan.iter().enumerate() {
        steps_of.entry(step.file).or_default().push(index);
    }
    let mut found = Vec::new();
    for (file, indexes) in steps_of {
        let path = &review.files[file].path;
        let Some((head, inside)) =
            heads.iter().find_map(|head| Some((head, path.strip_prefix(&format!("{}/", head.repo))?)))
        else {
            continue;
        };
        // Sin el fichero entonces, todo lo suyo es nuevo.
        let then = git(Path::new(&head.repo), &["show", &format!("{}:{inside}", head.sha)]).unwrap_or_default();
        if then == review.files[file].after {
            continue;
        }
        found.extend(changes_in(&then, &review.files[file].after, plan, &indexes));
    }
    found.sort_by_key(|change| change.step);
    found
}

/// Las líneas de `then` se van tachando con las que siguen ahí: lo que no tiene
/// pareja es nuevo, y lo que queda sin tachar se ha quitado.
fn changes_in(then: &str, now: &str, plan: &[FlowStep], steps: &[usize]) -> Vec<StepChange> {
    let mut left: HashMap<&str, usize> = HashMap::new();
    for line in then.lines() {
        *left.entry(line.trim()).or_default() += 1;
    }
    let take = |left: &mut HashMap<&str, usize>, text: &str| match left.get_mut(text.trim()) {
        Some(count) if *count > 0 => {
            *count -= 1;
            true
        }
        _ => false,
    };
    // Lo de ahora que el diff no da por añadido no es nuevo, salga o no en pantalla. Se
    // tacha antes para que no se lo lleve otra línea igual.
    let mut kept: HashMap<&str, usize> = HashMap::new();
    for line in now.lines() {
        *kept.entry(line.trim()).or_default() += 1;
    }
    for &step in steps {
        for line in &plan[step].lines {
            if let DiffLine::Added { text, .. } = line
                && let Some(count) = kept.get_mut(text.trim())
            {
                *count = count.saturating_sub(1);
            }
        }
    }
    for (text, count) in kept {
        for _ in 0..count {
            take(&mut left, text);
        }
    }
    let mut result: Vec<StepChange> = steps
        .iter()
        .map(|&step| {
            let fresh = plan[step]
                .lines
                .iter()
                .enumerate()
                .filter(|(_, line)| match line {
                    DiffLine::Added { text, .. } => !take(&mut left, text) && !text.trim().is_empty(),
                    DiffLine::Removed(_) | DiffLine::Context(_) => false,
                })
                .map(|(index, _)| index)
                .collect();
            StepChange { step, fresh, gone: Vec::new() }
        })
        .collect();
    // Lo que el diff enseña como quitado y entonces seguía: se ha quitado después.
    for change in &mut result {
        for (index, line) in plan[change.step].lines.iter().enumerate() {
            if let DiffLine::Removed(text) = line
                && !text.trim().is_empty()
                && take(&mut left, text)
            {
                change.fresh.push(index);
            }
        }
        change.fresh.sort_unstable();
    }
    // Lo demás no sale en ningún diff: estaba añadido y ya no. Se cuenta en el primer paso.
    let mut gone: Vec<String> = Vec::new();
    for line in then.lines() {
        let text = line.trim();
        if !text.is_empty() && take(&mut left, text) && gone.len() < MAX_GONE {
            gone.push(line.to_owned());
        }
    }
    result.retain(|change| !change.fresh.is_empty());
    if !gone.is_empty() {
        match result.first_mut() {
            Some(first) => first.gone = gone,
            None => result.push(StepChange { step: steps[0], fresh: Vec::new(), gone }),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    fn run(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    }

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-revisions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        dir
    }

    fn commit(repo: &Path, path: &str, content: &str, message: &str) -> String {
        std::fs::write(repo.join(path), content).unwrap();
        run(repo, &["add", path]);
        run(repo, &["commit", "-q", "-m", message]);
        String::from_utf8(Command::new("git").arg("-C").arg(repo).args(["rev-parse", "HEAD"]).output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_owned()
    }

    fn review_of(path: &str, before: &str, after: &str) -> (SessionReview, Vec<FlowStep>) {
        let edit = FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "task:A".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from(path),
            kind: EditKind::Replace,
            before: Some(before.into()),
            after: AfterState::Known(after.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("task:A", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    fn sync(repo: &Path) -> RepoSync {
        crate::task::sync(repo, "feature/A", Some("main"), false)
    }

    const BASE: &str = "function total() {\n  return 1;\n}\n";
    const FIRST: &str = "function total() {\n  const a = 1;\n  return a;\n}\n";
    const SECOND: &str = "function total() {\n  const a = 1;\n  log(a);\n  return a;\n}\n";

    #[test]
    fn only_what_changed_since_a_point_is_new_and_the_rest_stays_as_context() {
        let dir = repo("fresh");
        commit(&dir, "total.ts", BASE, "base");
        run(&dir, &["switch", "-q", "-c", "feature/A"]);
        let first = commit(&dir, "total.ts", FIRST, "primero");
        commit(&dir, "total.ts", SECOND, "segundo");
        let path = dir.join("total.ts").display().to_string();
        let (review, plan) = review_of(&path, BASE, SECOND);
        let heads = [Head { repo: dir.display().to_string(), sha: first }];
        let found = changes(&review, &plan, &heads);
        assert_eq!(found.len(), 1, "{found:?}");
        let lines = &plan[found[0].step].lines;
        let fresh: Vec<&DiffLine> = found[0].fresh.iter().map(|&index| &lines[index]).collect();
        assert!(matches!(fresh.as_slice(), [DiffLine::Added { text, .. }] if text.contains("log(a)")), "{fresh:?}");
        assert_eq!(found[0].gone, Vec::<String>::new());

        // Sin nada nuevo desde el último commit.
        let last = String::from_utf8(
            Command::new("git").arg("-C").arg(&dir).args(["rev-parse", "HEAD"]).output().unwrap().stdout,
        )
        .unwrap();
        let heads = [Head { repo: dir.display().to_string(), sha: last.trim().to_owned() }];
        assert_eq!(changes(&review, &plan, &heads), Vec::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_line_added_and_then_removed_is_told_because_no_diff_shows_it() {
        let dir = repo("gone");
        commit(&dir, "total.ts", BASE, "base");
        run(&dir, &["switch", "-q", "-c", "feature/A"]);
        let then = commit(&dir, "total.ts", SECOND, "con log");
        commit(&dir, "total.ts", FIRST, "sin log");
        let path = dir.join("total.ts").display().to_string();
        let (review, plan) = review_of(&path, BASE, FIRST);
        let found = changes(&review, &plan, &[Head { repo: dir.display().to_string(), sha: then }]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].fresh, Vec::<usize>::new());
        assert_eq!(found[0].gone, ["  log(a);"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_line_nobody_touched_outside_the_diff_is_not_told_as_removed() {
        let dir = repo("untouched");
        let far = |body: &str| format!("import a;\nimport b;\n\n\n\n\n\n\n\n\nfunction f() {{\n{body}}}\n");
        commit(&dir, "far.ts", &far("  one();\n"), "base");
        run(&dir, &["switch", "-q", "-c", "feature/A"]);
        let then = commit(&dir, "far.ts", &far("  one();\n  two();\n"), "dos");
        let after = far("  one();\n  two();\n  three();\n");
        commit(&dir, "far.ts", &after, "tres");
        let path = dir.join("far.ts").display().to_string();
        let (review, plan) = review_of(&path, &far("  one();\n"), &after);
        let found = changes(&review, &plan, &[Head { repo: dir.display().to_string(), sha: then }]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].gone, Vec::<String>::new());
        let lines = &plan[found[0].step].lines;
        let fresh: Vec<&DiffLine> = found[0].fresh.iter().map(|&index| &lines[index]).collect();
        assert!(matches!(fresh.as_slice(), [DiffLine::Added { text, .. }] if text.contains("three")), "{fresh:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_moment_is_saved_when_the_branch_advances_and_commits_are_offered() {
        let dir = repo("points");
        commit(&dir, "total.ts", BASE, "base");
        run(&dir, &["switch", "-q", "-c", "feature/A"]);
        let first = commit(&dir, "total.ts", FIRST, "primero");
        let store = Store::open_in_memory().unwrap();
        record(&store, "task:A", &[sync(&dir)]).unwrap();
        record(&store, "task:A", &[sync(&dir)]).unwrap();
        assert_eq!(store.revisions("task:A").unwrap().len(), 1, "sin avanzar no se guarda otro");
        assert_eq!(points(&store, "task:A", &[sync(&dir)]).unwrap(), Vec::new(), "desde el presente no hay nada nuevo");

        let second = commit(&dir, "total.ts", SECOND, "segundo");
        record(&store, "task:A", &[sync(&dir)]).unwrap();
        assert_eq!(store.revisions("task:A").unwrap().len(), 2);
        let found = points(&store, "task:A", &[sync(&dir)]).unwrap();
        assert_eq!(found.len(), 1, "el momento guardado y el commit son el mismo punto: {found:?}");
        assert_eq!(found[0].heads[0].sha, first);
        assert_ne!(first, second);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
