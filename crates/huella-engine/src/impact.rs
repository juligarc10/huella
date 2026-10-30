//! Impacto de una sesión: qué código existente, fuera de lo que tocó la
//! sesión, usa los símbolos que cambió. Es lo que puede romperse aunque el
//! diff parezca correcto.
//!
//! Se busca por nombre con los identificadores de tree-sitter, como el grafo
//! de llamadas entre pasos, sin modelo: puede haber falsos positivos con
//! nombres repetidos en otros módulos, nunca falsos negativos por formato.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use huella_anchoring::{Language, extract_symbols, identifiers};
use huella_core::SymbolKind;
use huella_review::SessionReview;
use huella_review::flow::{FlowStep, StepRole, StepSymbol};
use serde::Serialize;

/// Usos que se listan por paso; del resto solo se da el número.
const MAX_USES: usize = 30;
/// Ficheros que se recorren como mucho, y su tamaño máximo.
const MAX_FILES: usize = 20_000;
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Nombres demasiado comunes para que encontrarlos diga algo.
const GENERIC: [&str; 24] = [
    "new", "get", "set", "run", "main", "init", "default", "from", "into", "map", "len", "data", "value", "name", "id",
    "key", "type", "config", "index", "test", "load", "save", "update", "handler",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Use {
    /// Ruta relativa al proyecto.
    pub path: String,
    pub line: usize,
    /// Símbolo que contiene el uso, si lo hay.
    pub symbol: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StepImpact {
    /// Nombre buscado; `None` si el paso no es un símbolo del flujo.
    pub name: Option<String>,
    /// Usos fuera de la sesión (los primeros [`MAX_USES`]).
    pub uses: Vec<Use>,
    pub total: usize,
    /// El nombre es demasiado genérico: se cuenta pero no se lista.
    pub generic: bool,
}

/// Impacto de cada paso del guion, en su orden. `project` es la raíz del
/// repositorio de la sesión.
pub fn impact(project: &Path, review: &SessionReview, plan: &[FlowStep]) -> Vec<StepImpact> {
    let names: Vec<Option<String>> = plan
        .iter()
        .map(|step| {
            step.symbol
                .as_ref()
                .filter(|symbol| step.role == StepRole::Flow && usable_elsewhere(symbol))
                .map(|symbol| short_name(&symbol.qualified_name))
                .filter(|name| name.chars().count() >= 3)
        })
        .collect();
    let wanted: BTreeSet<&str> = names.iter().flatten().map(String::as_str).collect();
    // Ficheros que definen cada nombre: en el resto, un uso solo cuenta si el
    // fichero importa el nombre o el módulo que lo define.
    let mut homes: BTreeMap<&str, Vec<PathBuf>> = BTreeMap::new();
    for (step, name) in plan.iter().zip(&names) {
        if let Some(name) = name {
            homes.entry(name.as_str()).or_default().push(PathBuf::from(&review.files[step.file].path));
        }
    }
    if wanted.is_empty() {
        return plan.iter().map(|_| StepImpact::default()).collect();
    }
    // Código de la propia sesión: sus usos ya están en el grafo de llamadas.
    let own: BTreeMap<PathBuf, Vec<(usize, usize)>> = own_ranges(review, plan);

    let mut uses: BTreeMap<&str, Vec<Use>> = BTreeMap::new();
    for file in project_files(project) {
        let Some(language) = Language::from_path(&file) else { continue };
        let Ok(source) = std::fs::read_to_string(project.join(&file)) else { continue };
        let present: Vec<&str> = wanted.iter().copied().filter(|name| source.contains(name)).collect();
        if present.is_empty() {
            continue;
        }
        let absolute = project.join(&file);
        let imports: Vec<&str> = source.lines().filter(|line| is_import(line)).collect();
        let present: Vec<&str> = present
            .into_iter()
            .filter(|name| {
                let home = homes.get(name).map_or(&[][..], Vec::as_slice);
                home.contains(&absolute)
                    || imports.iter().any(|line| line.contains(name) || home.iter().any(|h| mentions_module(line, h)))
            })
            .collect();
        if present.is_empty() {
            continue;
        }
        let Ok(found) = identifiers(language, &source) else { continue };
        let symbols = extract_symbols(language, &source).map(|e| e.symbols).unwrap_or_default();
        let excluded = own.get(&project.join(&file)).map_or(&[][..], Vec::as_slice);
        let line_starts: Vec<usize> =
            std::iter::once(0).chain(source.match_indices('\n').map(|(at, _)| at + 1)).collect();
        let mut seen_lines = BTreeSet::new();
        // Símbolos homónimos ya vistos: su primera mención es su definición.
        let mut defined = BTreeSet::new();
        for (at, ident) in found {
            let Some(&name) = present.iter().find(|name| **name == ident) else { continue };
            let line = line_starts.partition_point(|&start| start <= at);
            if excluded.iter().any(|&(start, end)| start <= line && line <= end) {
                continue;
            }
            let container = symbols.iter().filter(|s| s.byte_range.contains(&at)).max_by_key(|s| s.depth);
            // Un símbolo homónimo definido aquí (otro `vote`, una reexportación) no es un uso.
            if let Some(symbol) = container.filter(|s| short_name(&s.qualified_name) == name)
                && defined.insert(symbol.byte_range.start)
            {
                continue;
            }
            if !seen_lines.insert((name, line)) {
                continue;
            }
            let container = container.map(|s| s.qualified_name.clone());
            let text = source[line_starts[line - 1]..].lines().next().unwrap_or_default().trim().to_owned();
            uses.entry(name).or_default().push(Use { path: file.display().to_string(), line, symbol: container, text });
        }
    }

    names
        .iter()
        .map(|name| {
            let Some(name) = name else { return StepImpact::default() };
            let found = uses.get(name.as_str()).cloned().unwrap_or_default();
            let generic = GENERIC.contains(&name.to_lowercase().as_str());
            StepImpact {
                name: Some(name.clone()),
                total: found.len(),
                uses: if generic { Vec::new() } else { found.into_iter().take(MAX_USES).collect() },
                generic,
            }
        })
        .collect()
}

/// Una línea que trae código de otro módulo, en los lenguajes que se anclan.
fn is_import(line: &str) -> bool {
    let line = line.trim_start();
    ["import ", "import{", "from ", "export ", "use ", "pub use ", "#include", "using ", "require "]
        .iter()
        .any(|start| line.starts_with(start))
        || line.contains("require(")
        || line.contains("import(")
}

/// La línea de importación nombra el módulo de `home`: su fichero
/// (`./billing`, `billing.service`) o, en Go, su paquete (la carpeta).
fn mentions_module(line: &str, home: &Path) -> bool {
    let stem = home.file_stem().map(|s| s.to_string_lossy()).filter(|s| s.len() >= 3 && s != "index" && s != "mod");
    let folder = home.parent().and_then(Path::file_name).map(|s| s.to_string_lossy());
    let names = stem.into_iter().chain(folder.filter(|_| home.extension().is_some_and(|e| e == "go")));
    names.into_iter().any(|name| {
        line.match_indices(name.as_ref()).any(|(at, _)| {
            // Como segmento de ruta o de módulo, no dentro de otra palabra.
            let before = line[..at].chars().next_back();
            let after = line[at + name.len()..].chars().next();
            before.is_none_or(|c| !c.is_alphanumeric() && c != '_')
                && after.is_none_or(|c| !c.is_alphanumeric() && c != '_')
        })
    })
}

/// Rangos de líneas (en el fichero final) del código de cada paso, por fichero.
fn own_ranges(review: &SessionReview, plan: &[FlowStep]) -> BTreeMap<PathBuf, Vec<(usize, usize)>> {
    let mut ranges: BTreeMap<PathBuf, Vec<(usize, usize)>> = BTreeMap::new();
    for step in plan {
        if let Some(symbol) = &step.symbol {
            ranges
                .entry(PathBuf::from(&review.files[step.file].path))
                .or_default()
                .push((symbol.start_line, symbol.end_line));
        }
    }
    ranges
}

/// Ficheros del repositorio, respetando `.gitignore`; si no es un repositorio,
/// un recorrido que salta las carpetas de dependencias y compilados.
fn project_files(project: &Path) -> Vec<PathBuf> {
    let listed = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    let files: Vec<PathBuf> = match listed {
        Some(output) => String::from_utf8_lossy(&output.stdout).lines().map(PathBuf::from).collect(),
        None => walk(project),
    };
    files
        .into_iter()
        .filter(|file| std::fs::metadata(project.join(file)).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE_BYTES))
        .take(MAX_FILES)
        .collect()
}

fn walk(root: &Path) -> Vec<PathBuf> {
    const SKIP: [&str; 8] = ["node_modules", "target", "dist", "build", ".git", ".next", "vendor", "__pycache__"];
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&dir)) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let relative = dir.join(&name);
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if !SKIP.contains(&name.to_string_lossy().as_ref()) {
                    pending.push(relative);
                }
            } else {
                found.push(relative);
            }
            if found.len() >= MAX_FILES {
                return found;
            }
        }
    }
    found
}

/// Lo que otro código puede usar: funciones, métodos y tipos, y variables
/// solo de nivel de módulo (una variable local no tiene usos fuera).
fn usable_elsewhere(symbol: &StepSymbol) -> bool {
    match symbol.kind {
        SymbolKind::Function
        | SymbolKind::Method
        | SymbolKind::Class
        | SymbolKind::Interface
        | SymbolKind::TypeAlias
        | SymbolKind::Enum
        | SymbolKind::Struct
        | SymbolKind::Trait => true,
        SymbolKind::Variable => !symbol.qualified_name.contains(['.', ':']),
        SymbolKind::Impl | SymbolKind::Module | SymbolKind::Test => false,
    }
}

/// `Clase.metodo` → `metodo`; `modulo::funcion` → `funcion`.
fn short_name(qualified: &str) -> String {
    qualified.rsplit(['.', ':']).find(|part| !part.is_empty()).unwrap_or(qualified).to_owned()
}

#[cfg(test)]
mod tests {
    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    #[test]
    fn finds_uses_outside_the_session_and_skips_its_own_code() {
        let root = std::env::temp_dir().join(format!("huella-impact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let changed = "export function chargeCustomer(id: string) {\n  return id;\n}\n\nexport function refund() {\n  return chargeCustomer('x');\n}\n";
        std::fs::write(root.join("src/billing.ts"), changed).unwrap();
        std::fs::write(
            root.join("src/checkout.ts"),
            "import { chargeCustomer } from './billing';\n\nexport function pay() {\n  return chargeCustomer('42');\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/notes.md"), "chargeCustomer en la documentación no cuenta\n").unwrap();

        let edit = FileEdit {
            origin: EditOrigin {
                agent: "test",
                session_id: "s".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: root.join("src/billing.ts"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(changed.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let impacts = impact(&root, &review, &plan);
        let charge =
            plan.iter().position(|s| s.symbol.as_ref().is_some_and(|s| s.qualified_name == "chargeCustomer")).unwrap();
        let found = &impacts[charge];
        assert_eq!(found.name.as_deref(), Some("chargeCustomer"));
        // El import y la llamada de checkout; no la llamada de refund (es de la sesión) ni el .md.
        let lines: Vec<(String, usize)> = found.uses.iter().map(|u| (u.path.clone(), u.line)).collect();
        assert_eq!(lines, vec![("src/checkout.ts".to_owned(), 1), ("src/checkout.ts".to_owned(), 4)]);
        assert_eq!(found.uses[1].symbol.as_deref(), Some("pay"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn other_files_count_only_if_they_import_the_name_or_its_module() {
        let root = std::env::temp_dir().join(format!("huella-impact-imports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let changed = "def charge(amount):\n    return amount\n";
        std::fs::write(root.join("billing.py"), changed).unwrap();
        // Importa el módulo y llama con `modulo.funcion`: es un uso.
        std::fs::write(root.join("shop.py"), "import billing\n\ndef buy():\n    return billing.charge(3)\n").unwrap();
        // Un `charge` local sin importar nada de billing: no lo es.
        std::fs::write(root.join("battery.py"), "def level():\n    charge = 80\n    return charge\n").unwrap();

        let edit = FileEdit {
            origin: EditOrigin {
                agent: "test",
                session_id: "s".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: root.join("billing.py"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(changed.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let found = impact(&root, &review, &plan).into_iter().find(|i| i.name.as_deref() == Some("charge")).unwrap();
        let files: Vec<&str> = found.uses.iter().map(|u| u.path.as_str()).collect();
        assert_eq!(files, vec!["shop.py"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn module_names_match_whole_segments() {
        assert!(mentions_module("import { x } from './billing';", Path::new("/r/src/billing.ts")));
        assert!(mentions_module("from app.billing import charge", Path::new("/r/app/billing.py")));
        assert!(!mentions_module("import { x } from './billings';", Path::new("/r/src/billing.ts")));
        assert!(mentions_module("import \"example.com/shop/billing\"", Path::new("/r/billing/charge.go")));
    }

    #[test]
    fn short_names() {
        assert_eq!(short_name("Billing.charge"), "charge");
        assert_eq!(short_name("billing::charge"), "charge");
        assert_eq!(short_name("charge"), "charge");
    }
}
