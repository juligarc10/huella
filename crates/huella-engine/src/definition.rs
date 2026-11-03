//! Dónde está definido un nombre, para verlo sin salir de la revisión.
//!
//! Se busca por nombre con los símbolos de tree-sitter, sin resolver tipos: si
//! hay varios candidatos se devuelven todos, primero los más cercanos. Se mira
//! lo que se va a mergear (el contenido de la rama), no la copia de trabajo.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use huella_anchoring::{Language, extract_symbols};
use huella_core::{Symbol, SymbolKind};
use huella_review::SessionReview;
use serde::Serialize;

use crate::triage::clearly_generated;

/// Candidatos que se devuelven como mucho.
const MAX_RESULTS: usize = 10;
/// Ficheros del repositorio que se abren por búsqueda.
const MAX_FILES: usize = 200;
const MAX_FILE_BYTES: usize = 512 * 1024;
/// Líneas de código que se devuelven por definición.
const MAX_LINES: usize = 300;

/// Dónde buscar: un repositorio y, si no es la copia de trabajo, en qué rama.
#[derive(Debug, Clone)]
pub struct SearchRoot {
    pub root: PathBuf,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Definition {
    /// Ruta completa, con la que se resalta y se busca desde ella.
    pub path: String,
    /// Ruta para mostrar, relativa a su repositorio.
    pub display_path: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub lines: Vec<String>,
    /// La definición es más larga que lo que se devuelve.
    pub truncated: bool,
    /// Está en un fichero que cambia esta revisión.
    pub in_change: bool,
}

/// ¿Es un nombre que se puede buscar? Evita pasar a git otra cosa que un identificador.
#[must_use]
pub fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn short_name(qualified: &str) -> &str {
    qualified.rsplit(['.', ':']).next().unwrap_or(qualified)
}

fn extension(path: &str) -> Option<&str> {
    Path::new(path).extension().and_then(|extension| extension.to_str())
}

fn is_test(path: &str) -> bool {
    let path = path.to_lowercase();
    path.contains("/test/") || path.contains("/tests/") || path.contains("test.") || path.contains(".spec.")
}

/// Los símbolos del fichero que se llaman así. Si uno está dentro de otro con el
/// mismo nombre (el constructor de Java en su clase), basta con el de fuera.
fn named(path: &str, source: &str, name: &str) -> Vec<Symbol> {
    let Some(language) = Language::from_path(Path::new(path)) else { return Vec::new() };
    let Ok(extraction) = extract_symbols(language, source) else { return Vec::new() };
    let found: Vec<Symbol> =
        extraction.symbols.into_iter().filter(|symbol| short_name(&symbol.qualified_name) == name).collect();
    found
        .iter()
        .filter(|symbol| {
            !found.iter().any(|outer| {
                !std::ptr::eq(*symbol, outer)
                    && outer.start_line <= symbol.start_line
                    && symbol.end_line <= outer.end_line
                    && (outer.end_line - outer.start_line) > (symbol.end_line - symbol.start_line)
            })
        })
        .cloned()
        .collect()
}

fn definition(path: &str, display: &str, source: &str, symbol: &Symbol, in_change: bool) -> Definition {
    let total = symbol.end_line + 1 - symbol.start_line;
    let lines: Vec<String> =
        source.lines().skip(symbol.start_line - 1).take(total.min(MAX_LINES)).map(str::to_owned).collect();
    Definition {
        path: path.to_owned(),
        display_path: display.to_owned(),
        qualified_name: symbol.qualified_name.clone(),
        kind: symbol.kind,
        start_line: symbol.start_line,
        lines,
        truncated: total > MAX_LINES,
        in_change,
    }
}

/// Ficheros del repositorio que contienen la palabra, sin los generados y con los
/// del mismo lenguaje primero, porque hay un tope.
fn candidates(search: &SearchRoot, name: &str, near: Option<&str>) -> Vec<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(&search.root).args(["grep", "-l", "-I", "-w", "-F", "-e", name]);
    if let Some(reference) = &search.reference {
        command.arg(reference);
    }
    command.arg("--");
    let Ok(output) = command.output() else { return Vec::new() };
    let listed = String::from_utf8_lossy(&output.stdout);
    let prefix = search.reference.as_ref().map(|reference| format!("{reference}:"));
    let mut files: Vec<String> = listed
        .lines()
        .map(|line| prefix.as_deref().and_then(|prefix| line.strip_prefix(prefix)).unwrap_or(line).to_owned())
        .filter(|path| !clearly_generated(path) && Language::from_path(Path::new(path)).is_some())
        .collect();
    files.sort_by_key(|path| extension(path) != near);
    files.truncate(MAX_FILES);
    files
}

fn read(search: &SearchRoot, inside: &str) -> Option<String> {
    let bytes = match &search.reference {
        Some(reference) => {
            let output = Command::new("git")
                .arg("-C")
                .arg(&search.root)
                .args(["show", &format!("{reference}:{inside}")])
                .output()
                .ok()?;
            output.status.success().then_some(output.stdout)?
        }
        None => std::fs::read(search.root.join(inside)).ok()?,
    };
    (bytes.len() <= MAX_FILE_BYTES).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// Definiciones de `name`, de la más cercana a `from` a la más lejana: primero lo
/// que cambia la revisión y después cada repositorio.
#[must_use]
pub fn find(review: &SessionReview, roots: &[SearchRoot], name: &str, from: &str) -> Vec<Definition> {
    if !is_identifier(name) {
        return Vec::new();
    }
    let display_of = |path: &str| {
        roots
            .iter()
            .find_map(|search| path.strip_prefix(&format!("{}/", search.root.display())).map(str::to_owned))
            .unwrap_or_else(|| path.to_owned())
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut found: Vec<Definition> = Vec::new();
    for file in &review.files {
        seen.insert(file.path.clone());
        if clearly_generated(&file.path) {
            continue;
        }
        for symbol in named(&file.path, &file.after, name) {
            found.push(definition(&file.path, &display_of(&file.path), &file.after, &symbol, true));
        }
    }
    for search in roots {
        for inside in candidates(search, name, extension(from)) {
            let path = format!("{}/{inside}", search.root.display());
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(source) = read(search, &inside) else { continue };
            for symbol in named(&path, &source, name) {
                found.push(definition(&path, &inside, &source, &symbol, false));
            }
        }
    }
    found.sort_by_key(|item| {
        (
            item.path != from,
            extension(&item.path) != extension(from),
            is_test(&item.path),
            !item.in_change,
            item.path.clone(),
            item.start_line,
        )
    });
    found.truncate(MAX_RESULTS);
    found
}

#[cfg(test)]
mod tests {
    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-definition-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        dir
    }

    fn commit(repo: &Path, path: &str, content: &str) {
        let file = repo.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
        git(repo, &["add", path]);
        git(repo, &["commit", "-q", "-m", "x"]);
    }

    fn review_of(files: &[(&str, &str)]) -> SessionReview {
        let edits: Vec<FileEdit> = files
            .iter()
            .enumerate()
            .map(|(n, (path, content))| FileEdit {
                origin: EditOrigin {
                    agent: "t",
                    session_id: "s".into(),
                    event_id: format!("e{n}"),
                    timestamp: None,
                    cwd: None,
                },
                path: PathBuf::from(path),
                kind: EditKind::Create,
                before: None,
                after: AfterState::Known((*content).into()),
                user_modified: false,
                context: EditContext::default(),
            })
            .collect();
        huella_review::build("s", &edits, |e| e.path.display().to_string(), |_| None)
    }

    const SERVICE: &str = "package a;\npublic class UserService {\n  public UserService() {}\n  public void register(String email) {\n    save(email);\n  }\n  private void save(String email) {\n    audit(email);\n  }\n}\n";

    #[test]
    fn a_java_method_and_a_class_are_found_in_the_committed_branch() {
        let dir = repo("branch");
        commit(&dir, "src/UserService.java", SERVICE);
        // La copia de trabajo no es lo que se va a mergear.
        std::fs::write(dir.join("src/UserService.java"), "class Otra {}\n").unwrap();
        let roots = [SearchRoot { root: dir.clone(), reference: Some("main".into()) }];
        let from = format!("{}/src/Caller.java", dir.display());
        let review = review_of(&[(&from, "class Caller {}\n")]);

        let found = find(&review, &roots, "save", &from);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].qualified_name, "UserService.save");
        assert_eq!(found[0].display_path, "src/UserService.java");
        assert_eq!(found[0].start_line, 7);
        assert_eq!(found[0].lines[1].trim(), "audit(email);");
        assert!(!found[0].in_change);

        // La clase, sin su constructor, que se llama igual.
        let found = find(&review, &roots, "UserService", &from);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].qualified_name, "UserService");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_the_review_changes_wins_and_the_nearest_goes_first() {
        let dir = repo("near");
        commit(&dir, "lib/other.ts", "export function total() {\n  return 'viejo';\n}\n");
        commit(&dir, "lib/total.py", "def total():\n    return 1\n");
        let path = format!("{}/lib/other.ts", dir.display());
        let review = review_of(&[(&path, "export function total() {\n  return 'de la rama';\n}\n")]);
        let roots = [SearchRoot { root: dir.clone(), reference: None }];
        let from = format!("{}/lib/caller.ts", dir.display());
        let found = find(&review, &roots, "total", &from);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].in_change && found[0].lines[1].contains("de la rama"), "{found:?}");
        assert_eq!(found[1].display_path, "lib/total.py", "otro lenguaje, después");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_identifiers_reach_git_and_generated_files_do_not_count() {
        assert!(is_identifier("user_name$2"));
        for bad in ["", "--output=x", "a b", "a;b", "../a"] {
            assert!(!is_identifier(bad), "{bad}");
        }
        let dir = repo("generated");
        commit(&dir, "dist/app.js", "function hidden() {}\n");
        let roots = [SearchRoot { root: dir.clone(), reference: None }];
        let review = review_of(&[]);
        assert!(find(&review, &roots, "hidden", "/x.js").is_empty());
        assert!(find(&review, &roots, "--output=x", "/x.js").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_long_definition_is_cut_and_says_so() {
        let body = "  x();\n".repeat(400);
        let source = format!("function long() {{\n{body}}}\n");
        let path = "/w/long.ts";
        let review = review_of(&[(path, source.as_str())]);
        let found = find(&review, &[], "long", path);
        assert_eq!(found[0].lines.len(), MAX_LINES);
        assert!(found[0].truncated);
    }
}
