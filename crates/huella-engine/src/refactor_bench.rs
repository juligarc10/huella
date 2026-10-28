//! Banco de refactors: mide cuánta memoria sobrevive a refactors hechos fuera
//! de las sesiones (criterio de salida de la F3: más del 80 %).
//!
//! Toma los anclajes vivos (el símbolo sigue en su fichero tal como lo dejó
//! la última sesión), les aplica refactors sintéticos en memoria, sin tocar el
//! disco, y comprueba que la memoria del símbolo resultante sigue incluyendo
//! la última edición anclada. También mide falsos positivos: código ajeno en
//! el lugar de un símbolo borrado no debe heredar su memoria.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_core::{Symbol, SymbolKind};
use huella_store::{AnchorState, Store, SymbolRole};

use crate::query::memory_of;

/// Un refactor sintético.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refactor {
    /// Renombrar el símbolo y sus usos en el fichero.
    Rename,
    /// Renombrar la clase o el tipo que lo contiene.
    RenameContainer,
    /// Mover el fichero a otra carpeta.
    MoveFile,
    /// Llevar la función a un fichero nuevo.
    MoveSymbol,
    /// Renombrar y añadir una línea al cuerpo.
    RenameAndEdit,
    /// Renombrar y quitar una de cada cuatro líneas del cuerpo.
    RenameAndRewrite,
    /// Mover el fichero y renombrar el símbolo.
    MoveFileAndRename,
}

impl Refactor {
    pub const ALL: [Self; 7] = [
        Self::Rename,
        Self::RenameContainer,
        Self::MoveFile,
        Self::MoveSymbol,
        Self::RenameAndEdit,
        Self::RenameAndRewrite,
        Self::MoveFileAndRename,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Rename => "renombrar el símbolo",
            Self::RenameContainer => "renombrar su clase o tipo",
            Self::MoveFile => "mover el fichero",
            Self::MoveSymbol => "mover la función a otro fichero",
            Self::RenameAndEdit => "renombrar y retocar el cuerpo",
            Self::RenameAndRewrite => "renombrar y quitar 1/4 del cuerpo",
            Self::MoveFileAndRename => "mover el fichero y renombrar",
        }
    }
}

/// Resultado de un refactor sobre todos los anclajes a los que se aplica.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    pub applicable: usize,
    pub survived: usize,
    /// Algunos casos perdidos, para inspeccionarlos.
    pub lost_examples: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BenchReport {
    /// Anclajes vivos probados.
    pub samples: usize,
    pub by_refactor: BTreeMap<Refactor, Tally>,
    /// Código ajeno puesto en el lugar de un símbolo borrado.
    pub probes: usize,
    pub false_positives: usize,
    /// Algunos falsos positivos: dónde y con qué parecido.
    pub false_positive_examples: Vec<String>,
}

impl BenchReport {
    /// Proporción de casos, de todos los refactors, en que sobrevive la memoria.
    #[must_use]
    pub fn survival(&self) -> f64 {
        let (applicable, survived) = self
            .by_refactor
            .values()
            .fold((0, 0), |(applicable, survived), tally| (applicable + tally.applicable, survived + tally.survived));
        ratio(survived, applicable)
    }
}

#[must_use]
pub fn ratio(part: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    f64::from(u32::try_from(part).unwrap_or(u32::MAX)) / f64::from(u32::try_from(total).unwrap_or(u32::MAX))
}

const LOST_EXAMPLES: usize = 5;
/// Funciones de una línea: un retoque las cambia casi por completo.
const MIN_LINES_TO_EDIT: usize = 3;
/// Con menos líneas, quitar una de cada cuatro no deja nada que reconocer.
const REWRITE_LINES: usize = 8;

/// Un anclaje vivo con el fichero en el que está.
struct Sample<'a> {
    anchor: &'a AnchorState,
    source: &'a str,
    symbol: &'a Symbol,
    language: Language,
}

/// Aplica los refactors a, como mucho, `limit` anclajes vivos repartidos por
/// todos los ficheros.
///
/// # Errors
/// Si falla la base de datos o el parser.
pub fn refactor_benchmark(store: &Store, limit: usize) -> crate::Result<BenchReport> {
    let anchors: Vec<AnchorState> = store
        .all_anchors()?
        .into_iter()
        .filter(|anchor| anchor.role == SymbolRole::Touched && !anchor.fingerprint.shape_hash.is_empty())
        .collect();
    let mut files: HashMap<String, Option<(String, Vec<Symbol>)>> = HashMap::new();
    for anchor in &anchors {
        files.entry(anchor.path.clone()).or_insert_with(|| {
            let source = std::fs::read_to_string(&anchor.path).ok()?;
            let language = Language::from_path(Path::new(&anchor.path))?;
            let symbols = extract_symbols(language, &source).ok()?.symbols;
            Some((source, symbols))
        });
    }
    let mut live: Vec<Sample> = anchors
        .iter()
        .filter_map(|anchor| {
            let (source, symbols) = files.get(&anchor.path)?.as_ref()?;
            let symbol = symbols.iter().find(|s| s.qualified_name == anchor.qualified_name)?;
            let language = Language::from_path(Path::new(&anchor.path))?;
            (symbol.fingerprint.shape_hash == anchor.fingerprint.shape_hash).then_some(Sample {
                anchor,
                source,
                symbol,
                language,
            })
        })
        .filter(|sample| is_renamable(own_name(&sample.symbol.qualified_name)))
        .collect();
    live.sort_by(|a, b| (&a.anchor.path, &a.anchor.qualified_name).cmp(&(&b.anchor.path, &b.anchor.qualified_name)));
    // Muestra repartida por todo el conjunto, determinista.
    let step = live.len().div_ceil(limit.max(1)).max(1);
    let samples: Vec<&Sample> = live.iter().step_by(step).collect();

    let mut report = BenchReport { samples: samples.len(), ..BenchReport::default() };
    for sample in &samples {
        for refactor in Refactor::ALL {
            let Some(case) = apply(refactor, sample) else { continue };
            let tally = report.by_refactor.entry(refactor).or_default();
            tally.applicable += 1;
            if survives(store, sample, &case)? {
                tally.survived += 1;
            } else if tally.lost_examples.len() < LOST_EXAMPLES {
                tally.lost_examples.push(format!("{} · {}", sample.anchor.path, sample.anchor.qualified_name));
            }
        }
    }
    for (index, sample) in samples.iter().enumerate() {
        let Some(case) = probe(sample, &samples, index) else { continue };
        report.probes += 1;
        let memory = memory_of(store, Path::new(&case.path), &case.source, &case.name, &case.reader())?;
        let Some(memory) = memory else { continue };
        if memory.entries.iter().any(|entry| entry.edit_id == sample.anchor.edit_id) {
            report.false_positives += 1;
            if report.false_positive_examples.len() < LOST_EXAMPLES * 2 {
                let origin = memory.origins.first();
                report.false_positive_examples.push(format!(
                    "{} · {} ← {} ({:.0} %)",
                    sample.anchor.path,
                    sample.anchor.qualified_name,
                    origin.map_or("?", |o| o.qualified_name.as_str()),
                    origin.map_or(0.0, |o| o.similarity * 100.0)
                ));
            }
        }
    }
    Ok(report)
}

/// El fichero tras el refactor y dónde buscar el símbolo.
struct Case {
    path: String,
    source: String,
    name: String,
    /// Otros ficheros cambiados: contenido nuevo, o `None` si ya no existen.
    overlay: Vec<(String, Option<String>)>,
}

impl Case {
    fn reader(&self) -> impl Fn(&str) -> Option<String> + '_ {
        move |path| {
            if path == self.path {
                return Some(self.source.clone());
            }
            match self.overlay.iter().find(|(changed, _)| changed == path) {
                Some((_, content)) => content.clone(),
                None => std::fs::read_to_string(path).ok(),
            }
        }
    }
}

fn survives(store: &Store, sample: &Sample, case: &Case) -> crate::Result<bool> {
    let memory = memory_of(store, Path::new(&case.path), &case.source, &case.name, &case.reader())?;
    Ok(memory.is_some_and(|memory| memory.entries.iter().any(|entry| entry.edit_id == sample.anchor.edit_id)))
}

fn apply(refactor: Refactor, sample: &Sample) -> Option<Case> {
    let path = &sample.anchor.path;
    let name = &sample.symbol.qualified_name;
    match refactor {
        Refactor::Rename => {
            let (source, renamed) = rename_symbol(sample.source, name);
            Some(Case { path: path.clone(), source, name: renamed, overlay: Vec::new() })
        }
        Refactor::RenameContainer => {
            let container = container_name(name)?;
            let new_container = format!("{container}Huella");
            let source = replace_identifier(sample.source, container, &new_container);
            let renamed = format!("{new_container}{}", &name[container.len()..]);
            Some(Case { path: path.clone(), source, name: renamed, overlay: Vec::new() })
        }
        Refactor::MoveFile => {
            let moved = moved_path(path)?;
            let overlay = vec![(path.clone(), None)];
            Some(Case { path: moved, source: sample.source.to_owned(), name: name.clone(), overlay })
        }
        Refactor::MoveSymbol => {
            if sample.symbol.depth > 0 || !is_function(sample.symbol.kind) {
                return None;
            }
            let range = sample.symbol.byte_range.clone();
            let remaining = format!("{}{}", &sample.source[..range.start], &sample.source[range.end..]);
            let extension = Path::new(path).extension()?.to_str()?;
            let new_path = format!("{}/huella_moved_{}.{extension}", parent(path)?, own_name(name));
            let source = format!("{}\n", &sample.source[range]);
            Some(Case { path: new_path, source, name: name.clone(), overlay: vec![(path.clone(), Some(remaining))] })
        }
        Refactor::RenameAndEdit => {
            if !is_function(sample.symbol.kind)
                || sample.symbol.end_line - sample.symbol.start_line + 1 < MIN_LINES_TO_EDIT
            {
                return None;
            }
            let edited = insert_statement(sample.source, sample.symbol, sample.language)?;
            let (source, renamed) = rename_symbol(&edited, name);
            Some(Case { path: path.clone(), source, name: renamed, overlay: Vec::new() })
        }
        Refactor::RenameAndRewrite => {
            if !is_function(sample.symbol.kind) || sample.symbol.end_line - sample.symbol.start_line + 1 < REWRITE_LINES
            {
                return None;
            }
            let rewritten = drop_every_fourth_line(sample.source, sample.symbol);
            let (source, renamed) = rename_symbol(&rewritten, name);
            Some(Case { path: path.clone(), source, name: renamed, overlay: Vec::new() })
        }
        Refactor::MoveFileAndRename => {
            let (source, renamed) = rename_symbol(sample.source, name);
            let overlay = vec![(path.clone(), None)];
            Some(Case { path: moved_path(path)?, source, name: renamed, overlay })
        }
    }
}

/// Código de otro fichero, renombrado, en el lugar de la función borrada.
fn probe(sample: &Sample, samples: &[&Sample], index: usize) -> Option<Case> {
    if sample.symbol.depth > 0 || !is_function(sample.symbol.kind) {
        return None;
    }
    let donor = samples[index + 1..].iter().chain(&samples[..index]).find(|other| {
        other.anchor.path != sample.anchor.path
                && other.symbol.depth == 0
                && is_function(other.symbol.kind)
                && other.language == sample.language
                // El mismo código copiado no es ajeno: heredar su memoria es correcto.
                && other.symbol.fingerprint.shape_hash != sample.symbol.fingerprint.shape_hash
    })?;
    let donor_text = &donor.source[donor.symbol.byte_range.clone()];
    let donor_name = own_name(&donor.symbol.qualified_name);
    let probe_text = replace_identifier(donor_text, donor_name, "huellaProbe");
    let range = sample.symbol.byte_range.clone();
    let source = format!("{}{probe_text}{}", &sample.source[..range.start], &sample.source[range.end..]);
    Some(Case { path: sample.anchor.path.clone(), source, name: "huellaProbe".into(), overlay: Vec::new() })
}

/// Renombra el nombre propio del símbolo y sus usos en el fichero.
fn rename_symbol(source: &str, qualified_name: &str) -> (String, String) {
    let name = own_name(qualified_name);
    let new_name = format!("{name}Huella");
    let renamed = format!("{}{new_name}", &qualified_name[..qualified_name.len() - name.len()]);
    (replace_identifier(source, name, &new_name), renamed)
}

/// Sustituye `from` por `to` donde aparezca como identificador completo.
fn replace_identifier(source: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    let mut previous: Option<char> = None;
    while let Some(found) = rest.find(from) {
        let before = rest[..found].chars().next_back().or(previous);
        let after = rest[found + from.len()..].chars().next();
        out.push_str(&rest[..found]);
        if before.is_some_and(is_identifier_char) || after.is_some_and(is_identifier_char) {
            out.push_str(from);
        } else {
            out.push_str(to);
        }
        previous = from.chars().next_back();
        rest = &rest[found + from.len()..];
    }
    out.push_str(rest);
    out
}

/// Añade una sentencia antes de la última línea del cuerpo.
fn insert_statement(source: &str, symbol: &Symbol, language: Language) -> Option<String> {
    let mut lines: Vec<&str> = source.split('\n').collect();
    let last = symbol.end_line.checked_sub(1)?;
    let indent: String = lines.get(last)?.chars().take_while(|c| c.is_whitespace()).collect();
    let statement = match language {
        Language::Python => format!("{indent}print(\"huella\")"),
        Language::Rust => format!("{indent}eprintln!(\"huella\");"),
        _ => format!("{indent}console.debug(\"huella\");"),
    };
    lines.insert(last, &statement);
    Some(lines.join("\n"))
}

/// Quita una de cada cuatro líneas del interior del cuerpo (ni la firma ni el
/// cierre), un cambio grande pero que deja el símbolo con su nombre.
fn drop_every_fourth_line(source: &str, symbol: &Symbol) -> String {
    let interior = symbol.start_line + 1..symbol.end_line.saturating_sub(1);
    source
        .split('\n')
        .enumerate()
        .filter(|(index, _)| {
            let line = index + 1;
            !(interior.contains(&line) && (line - interior.start) % 4 == 3)
        })
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn moved_path(path: &str) -> Option<String> {
    let file_name = Path::new(path).file_name()?.to_str()?;
    Some(format!("{}/huella_moved/{file_name}", parent(path)?))
}

fn parent(path: &str) -> Option<&str> {
    Path::new(path).parent()?.to_str()
}

fn container_name(qualified_name: &str) -> Option<&str> {
    let end = qualified_name.find(['.', ':'])?;
    let container = &qualified_name[..end];
    is_identifier(container).then_some(container)
}

fn own_name(qualified_name: &str) -> &str {
    qualified_name.rsplit(['.', ':']).next().unwrap_or(qualified_name)
}

/// Nombres que se pueden renombrar: no `export default` ni constructores.
fn is_renamable(name: &str) -> bool {
    is_identifier(name) && !matches!(name, "default" | "constructor" | "__init__")
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(is_identifier_char) && !name.starts_with(|c: char| c.is_ascii_digit())
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

const fn is_function(kind: SymbolKind) -> bool {
    matches!(kind, SymbolKind::Function | SymbolKind::Method)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_replaced_only_when_whole() {
        let source = "function save(x) { return saveAll(save, x.save, _save); }";
        assert_eq!(
            replace_identifier(source, "save", "store"),
            "function store(x) { return saveAll(store, x.store, _save); }"
        );
    }

    #[test]
    fn renaming_keeps_the_container_prefix() {
        let (source, renamed) = rename_symbol("class A { run() { this.run(); } }", "A.run");
        assert_eq!(renamed, "A.runHuella");
        assert_eq!(source, "class A { runHuella() { this.runHuella(); } }");
    }
}
