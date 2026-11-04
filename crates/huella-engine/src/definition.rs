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
/// Usos que se listan; del resto solo se da el número.
const MAX_USES: usize = 30;

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

/// Lo que se busca: un nombre, visto desde un fichero.
#[derive(Debug, Clone, Copy)]
pub struct Lookup<'a> {
    pub name: &'a str,
    /// Fichero desde el que se busca.
    pub from: &'a str,
    /// Lo que hay antes del nombre en su línea: de ahí sale de qué tipo es.
    pub before: Option<&'a str>,
}

/// Un uso del nombre en el código.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Use {
    pub display_path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Found {
    pub definitions: Vec<Definition>,
    /// Los primeros usos en el mismo servicio, y cuántos hay.
    pub uses: Vec<Use>,
    pub total_uses: usize,
}

/// A qué se le pide el nombre, según lo que hay antes en la línea.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Receiver<'a> {
    /// `save(x)`, o `this.save(x)`: de este mismo fichero, si está.
    Own,
    /// `users.save(x)` o `Audit.log(x)`.
    Named(&'a str),
    /// `a().save(x)`, o no se sabe: sin pistas.
    Unknown,
}

fn receiver(before: Option<&str>) -> Receiver<'_> {
    let Some(before) = before else { return Receiver::Unknown };
    let before = before.trim_end();
    let Some(rest) =
        before.strip_suffix('.').or_else(|| before.strip_suffix("::")).or_else(|| before.strip_suffix("->"))
    else {
        return Receiver::Own;
    };
    let rest = rest.trim_end();
    let start = rest
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .map_or(0, |at| at + rest[at..].chars().next().map_or(1, char::len_utf8));
    match &rest[start..] {
        "" => Receiver::Unknown,
        "this" | "self" | "Self" | "super" => Receiver::Own,
        name => Receiver::Named(name),
    }
}

fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).filter(|word| !word.is_empty())
}

fn is_type_name(word: &str) -> bool {
    word.chars().next().is_some_and(char::is_uppercase)
}

/// El tipo con que se declara una variable o un campo en el fichero: `UserRepository
/// users` (Java, C#, Go al revés) o `users: UserRepository` (TypeScript, Kotlin, Rust).
fn declared_type<'a>(source: &'a str, variable: &str) -> Option<&'a str> {
    source.lines().find_map(|line| {
        let tokens: Vec<&str> = words(line).collect();
        let at = tokens.iter().position(|token| *token == variable)?;
        let after = line.split(variable).nth(1).unwrap_or_default().trim_start();
        // Después del nombre, `: Tipo` o, en Go, `Tipo`.
        if let Some(next) = tokens.get(at + 1).filter(|next| is_type_name(next))
            && (after.starts_with(':') || after.starts_with(*next))
        {
            return Some(*next);
        }
        // Antes del nombre, `Tipo nombre` o `List<Tipo> nombre`: el tipo más cercano.
        let declares = after.is_empty() || after.starts_with([';', '=', ',', ')']);
        declares.then(|| tokens[..at].iter().rev().find(|token| is_type_name(token)).copied()).flatten()
    })
}

fn container(qualified: &str) -> Option<&str> {
    let plain = qualified.rsplit_once(['.', ':'])?.0;
    Some(short_name(plain.trim_end_matches(':')))
}

/// ¿La clase de esta definición implementa o extiende `wanted`?
fn extends(source: &str, class: &str, wanted: &str) -> bool {
    source.lines().any(|line| {
        let tokens: Vec<&str> = words(line).collect();
        tokens.iter().any(|token| matches!(*token, "class" | "impl" | "struct" | "interface" | "object"))
            && tokens.contains(&class)
            && tokens.contains(&wanted)
    })
}

/// Una definición candidata, con el fichero en que está.
struct Candidate {
    definition: Definition,
    source: String,
}

fn candidates_in_review(review: &SessionReview, name: &str, display_of: impl Fn(&str) -> String) -> Vec<Candidate> {
    review
        .files
        .iter()
        .filter(|file| !clearly_generated(&file.path))
        .flat_map(|file| {
            named(&file.path, &file.after, name)
                .into_iter()
                .map(|symbol| Candidate {
                    definition: definition(&file.path, &display_of(&file.path), &file.after, &symbol, true),
                    source: file.after.clone(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn root_of<'a>(roots: &'a [SearchRoot], path: &str) -> Option<&'a SearchRoot> {
    roots.iter().find(|search| path.starts_with(&format!("{}/", search.root.display())))
}

/// Los usos de `name` en el servicio. Con un tipo conocido, solo en ficheros que lo
/// nombran o que definen el método: otro `execute` de otra clase no es un uso de este.
fn uses_of(
    search: &SearchRoot,
    name: &str,
    wanted: Option<&str>,
    near: Option<&str>,
    found: &[Definition],
) -> (Vec<Use>, usize) {
    let grep = |word: &str, list: bool| {
        let mut command = Command::new("git");
        command.arg("-C").arg(&search.root).args(["grep", "-I", "-w", "-F"]);
        command.arg(if list { "-l" } else { "-n" }).args(["-e", word]);
        if let Some(reference) = &search.reference {
            command.arg(reference);
        }
        command.arg("--");
        command.output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default()
    };
    let prefix = search.reference.as_ref().map_or(String::new(), |reference| format!("{reference}:"));
    let naming: Option<BTreeSet<String>> = wanted.map(|word| {
        grep(word, true).lines().map(|line| line.strip_prefix(prefix.as_str()).unwrap_or(line).to_owned()).collect()
    });
    let mut uses = Vec::new();
    let mut total = 0;
    for line in grep(name, false).lines() {
        let line = line.strip_prefix(prefix.as_str()).unwrap_or(line);
        let mut parts = line.splitn(3, ':');
        let (Some(path), Some(number), Some(text)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let Ok(number) = number.parse::<usize>() else { continue };
        let inside_definition = found.iter().any(|definition| {
            definition.display_path == path
                && number >= definition.start_line
                && number < definition.start_line + definition.lines.len()
        });
        let defines_it = found.iter().any(|definition| definition.display_path == path);
        if inside_definition
            || clearly_generated(path)
            || extension(path) != near
            || naming.as_ref().is_some_and(|files| !files.contains(path) && !defines_it)
        {
            continue;
        }
        total += 1;
        if uses.len() < MAX_USES {
            uses.push(Use { display_path: path.to_owned(), line: number, text: text.trim().to_owned() });
        }
    }
    (uses, total)
}

/// Definiciones de un nombre, de la más cercana a la más lejana, y sus usos. Se
/// queda en el servicio desde el que se busca, salvo que allí no esté; si se sabe de
/// qué tipo se llama, solo ese tipo y lo que lo implementa.
#[must_use]
pub fn find(review: &SessionReview, roots: &[SearchRoot], lookup: Lookup<'_>) -> Found {
    let Lookup { name, from, before } = lookup;
    if !is_identifier(name) {
        return Found::default();
    }
    let display_of = |path: &str| {
        root_of(roots, path)
            .and_then(|search| path.strip_prefix(&format!("{}/", search.root.display())))
            .unwrap_or(path)
            .to_owned()
    };
    let mut seen: BTreeSet<String> = review.files.iter().map(|file| file.path.clone()).collect();
    let mut all = candidates_in_review(review, name, display_of);
    for search in roots {
        for inside in candidates(search, name, extension(from)) {
            let path = format!("{}/{inside}", search.root.display());
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(source) = read(search, &inside) else { continue };
            for symbol in named(&path, &source, name) {
                all.push(Candidate {
                    definition: definition(&path, &inside, &source, &symbol, false),
                    source: source.clone(),
                });
            }
        }
    }

    // Del mismo servicio, si lo hay.
    let home = root_of(roots, from);
    if let Some(home) = home {
        let prefix = format!("{}/", home.root.display());
        if all.iter().any(|candidate| candidate.definition.path.starts_with(&prefix)) {
            all.retain(|candidate| candidate.definition.path.starts_with(&prefix));
        }
    }

    let from_source = review
        .files
        .iter()
        .find(|file| file.path == from)
        .map(|file| file.after.clone())
        .or_else(|| {
            let search = home?;
            read(search, from.strip_prefix(&format!("{}/", search.root.display()))?)
        })
        .unwrap_or_default();
    let called_on = receiver(before);
    let wanted = match called_on {
        Receiver::Named(word) if is_type_name(word) => Some(word.to_owned()),
        Receiver::Named(word) => declared_type(&from_source, word).map(str::to_owned),
        Receiver::Own | Receiver::Unknown => None,
    };
    if let Some(wanted) = &wanted {
        let matches = |candidate: &Candidate| {
            container(&candidate.definition.qualified_name)
                .is_some_and(|class| class == wanted || extends(&candidate.source, class, wanted))
        };
        if all.iter().any(matches) {
            all.retain(matches);
        }
    }
    if called_on == Receiver::Own && all.iter().any(|candidate| candidate.definition.path == from) {
        all.retain(|candidate| candidate.definition.path == from);
    }

    let mut definitions: Vec<Definition> = all.into_iter().map(|candidate| candidate.definition).collect();
    definitions.sort_by_key(|item| {
        (
            item.path != from,
            extension(&item.path) != extension(from),
            is_test(&item.path),
            !item.in_change,
            item.path.clone(),
            item.start_line,
        )
    });
    definitions.truncate(MAX_RESULTS);
    let (uses, total_uses) =
        home.map(|search| uses_of(search, name, wanted.as_deref(), extension(from), &definitions)).unwrap_or_default();
    Found { definitions, uses, total_uses }
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

    fn defs(review: &SessionReview, roots: &[SearchRoot], name: &str, from: &str) -> Vec<Definition> {
        find(review, roots, Lookup { name, from, before: None }).definitions
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

        let found = defs(&review, &roots, "save", &from);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].qualified_name, "UserService.save");
        assert_eq!(found[0].display_path, "src/UserService.java");
        assert_eq!(found[0].start_line, 7);
        assert_eq!(found[0].lines[1].trim(), "audit(email);");
        assert!(!found[0].in_change);

        // La clase, sin su constructor, que se llama igual.
        let found = defs(&review, &roots, "UserService", &from);
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
        let found = defs(&review, &roots, "total", &from);
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
        assert_eq!(defs(&review, &roots, "hidden", "/x.js").len(), 0);
        assert_eq!(defs(&review, &roots, "--output=x", "/x.js").len(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const CALLER: &str = "class OrderService {\n  private final PaymentGateway gateway;\n  void pay() {\n    gateway.execute(1);\n    execute(2);\n  }\n  void execute(int x) {}\n}\n";
    const GATEWAY: &str = "interface PaymentGateway {\n  void execute(int amount);\n}\n";
    const STRIPE: &str = "class StripeGateway implements PaymentGateway {\n  public void execute(int amount) {}\n}\n";
    const OTHER_TASK: &str = "class ImportTask {\n  void execute() {}\n}\n";

    fn services(name: &str) -> (PathBuf, PathBuf) {
        let billing = repo(&format!("{name}-billing"));
        commit(&billing, "src/OrderService.java", CALLER);
        commit(&billing, "src/PaymentGateway.java", GATEWAY);
        commit(&billing, "src/StripeGateway.java", STRIPE);
        commit(&billing, "src/ImportTask.java", OTHER_TASK);
        let other = repo(&format!("{name}-other"));
        commit(&other, "src/PaymentGateway.java", "class PaymentGateway {\n  void execute() {}\n}\n");
        commit(&other, "src/Elsewhere.java", "class Elsewhere {\n  void execute() {}\n}\n");
        (billing, other)
    }

    fn names(found: &Found) -> Vec<&str> {
        let mut names: Vec<&str> =
            found.definitions.iter().map(|definition| definition.qualified_name.as_str()).collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn a_method_is_looked_up_in_its_own_service_and_on_the_type_it_is_called_on() {
        let (billing, other) = services("scope");
        let roots = [
            SearchRoot { root: billing.clone(), reference: Some("main".into()) },
            SearchRoot { root: other.clone(), reference: Some("main".into()) },
        ];
        let from = format!("{}/src/OrderService.java", billing.display());
        let review = review_of(&[]);

        // `gateway` es un PaymentGateway: su interfaz y quien la implementa, no ImportTask ni el otro servicio.
        let found = find(&review, &roots, Lookup { name: "execute", from: &from, before: Some("    gateway.") });
        assert_eq!(names(&found), ["PaymentGateway.execute", "StripeGateway.execute"], "{found:?}");
        assert!(found.definitions.iter().all(|definition| definition.path.starts_with(&billing.display().to_string())));

        // Sin receptor: el de este mismo fichero.
        let found = find(&review, &roots, Lookup { name: "execute", from: &from, before: Some("    ") });
        assert_eq!(names(&found), ["OrderService.execute"]);

        // Sin pistas: todos los de este servicio, ninguno del otro.
        let found = find(&review, &roots, Lookup { name: "execute", from: &from, before: None });
        assert_eq!(names(&found).len(), 4, "{found:?}");
        assert!(found.definitions.iter().all(|definition| definition.path.starts_with(&billing.display().to_string())));

        // Si en este servicio no está, se busca en los demás.
        let found = find(&review, &roots, Lookup { name: "Elsewhere", from: &from, before: None });
        assert_eq!(names(&found), ["Elsewhere"]);
        std::fs::remove_dir_all(&billing).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn the_uses_are_those_of_the_type_and_not_any_method_with_that_name() {
        let (billing, other) = services("uses");
        let roots = [SearchRoot { root: billing.clone(), reference: Some("main".into()) }];
        let from = format!("{}/src/OrderService.java", billing.display());
        let found = find(&review_of(&[]), &roots, Lookup { name: "execute", from: &from, before: Some("gateway.") });
        let uses: Vec<(&str, usize)> = found.uses.iter().map(|one| (one.display_path.as_str(), one.line)).collect();
        assert!(uses.contains(&("src/OrderService.java", 4)), "{uses:?}");
        assert!(!uses.iter().any(|(path, _)| *path == "src/ImportTask.java"), "{uses:?}");
        assert_eq!(found.total_uses, found.uses.len());
        std::fs::remove_dir_all(&billing).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn the_type_of_a_variable_comes_from_how_it_is_declared() {
        assert_eq!(declared_type("  private final UserRepository users;", "users"), Some("UserRepository"));
        assert_eq!(declared_type("  List<User> users = x();", "users"), Some("User"));
        assert_eq!(declared_type("  private users: UserRepository,", "users"), Some("UserRepository"));
        assert_eq!(declared_type("users.save(x);", "users"), None);
        assert_eq!(receiver(Some("  users.")), Receiver::Named("users"));
        assert_eq!(receiver(Some("this.")), Receiver::Own);
        assert_eq!(receiver(Some("return ")), Receiver::Own);
        assert_eq!(receiver(Some("a().")), Receiver::Unknown);
        assert_eq!(receiver(None), Receiver::Unknown);
    }

    #[test]
    fn a_long_definition_is_cut_and_says_so() {
        let body = "  x();\n".repeat(400);
        let source = format!("function long() {{\n{body}}}\n");
        let path = "/w/long.ts";
        let review = review_of(&[(path, source.as_str())]);
        let found = defs(&review, &[], "long", path);
        assert_eq!(found[0].lines.len(), MAX_LINES);
        assert!(found[0].truncated);
    }
}
