//! Lo que el agente declara mientras escribe: por qué decidió algo, qué dio por
//! hecho, qué no comprobó y cómo funciona lo que no es obvio. Huella lo deduce
//! de las transcripciones cuando puede; esto es lo que el agente dice a
//! propósito, con estructura, y vale para cualquier agente con MCP.
//!
//! Cada declaración se ancla al símbolo (con tree-sitter) y guarda su texto de
//! entonces: si el código cambia, se sabe que puede estar desactualizada. Es
//! una pista para quien revisa, no una verdad: el agente puede justificarse.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use huella_review::SessionReview;
use huella_review::flow::FlowStep;
use huella_store::{Alternative, Declaration, Store};
use serde::Serialize;

use crate::{EngineError, Result};

pub const KINDS: [&str; 4] = ["decision", "assumption", "unverified", "risk"];
pub const CONFIDENCES: [&str; 3] = ["high", "medium", "low"];
/// Declaraciones por paso que se muestran y se pasan al modelo.
const PER_STEP: usize = 5;

/// Lo que llega del agente.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewDeclaration {
    pub file: PathBuf,
    pub symbol: Option<String>,
    /// Líneas 1-based e inclusivas.
    pub lines: Option<(usize, usize)>,
    pub kind: String,
    pub summary: String,
    pub how: Option<String>,
    pub alternatives: Vec<Alternative>,
    pub assumptions: Vec<String>,
    pub unverified: Vec<String>,
    pub confidence: Option<String>,
    pub review_hint: Option<String>,
}

/// Dónde quedó anclada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub id: i64,
    pub symbol: Option<String>,
    pub lines: (usize, usize),
}

/// Guarda una declaración anclada al código tal como está ahora en disco.
///
/// # Errors
/// Si el fichero no existe, faltan campos o los valores no son válidos.
pub fn record(store: &Store, new: &NewDeclaration) -> Result<Recorded> {
    let invalid = |message: String| EngineError::Invalid(message);
    if !KINDS.contains(&new.kind.as_str()) {
        return Err(invalid(format!("kind debe ser uno de {}", KINDS.join(", "))));
    }
    if let Some(confidence) = &new.confidence
        && !CONFIDENCES.contains(&confidence.as_str())
    {
        return Err(invalid(format!("confidence debe ser uno de {}", CONFIDENCES.join(", "))));
    }
    if new.summary.trim().is_empty() {
        return Err(invalid("falta summary: qué se hizo y por qué".into()));
    }
    let file = std::fs::canonicalize(&new.file)
        .map_err(|_| invalid(format!("{} no existe: registra después de escribir el código", new.file.display())))?;
    let source = std::fs::read_to_string(&file)?;
    let lines: Vec<&str> = source.lines().collect();
    let (symbol, (start, end)) = anchor(&file, &source, new.symbol.as_deref(), new.lines);
    let (start, end) = (start.clamp(1, lines.len().max(1)), end.clamp(1, lines.len().max(1)));
    let snapshot = lines.get(start - 1..end.max(start)).map(|slice| slice.join("\n")).unwrap_or_default();
    let repo = file.parent().and_then(crate::task::repo_root);
    let branch = repo.as_deref().and_then(crate::task::checked_out);
    let id = store.add_declaration(&Declaration {
        id: 0,
        recorded_at: String::new(),
        path: crate::normalize_path(&file, None),
        symbol: symbol.clone(),
        start_line: start,
        end_line: end,
        snapshot,
        repo: repo.map(|repo| repo.display().to_string()),
        branch,
        kind: new.kind.clone(),
        summary: new.summary.trim().to_owned(),
        how: new.how.clone().filter(|how| !how.trim().is_empty()),
        alternatives: new.alternatives.clone(),
        assumptions: new.assumptions.clone(),
        unverified: new.unverified.clone(),
        confidence: new.confidence.clone(),
        review_hint: new.review_hint.clone().filter(|hint| !hint.trim().is_empty()),
    })?;
    Ok(Recorded { id, symbol, lines: (start, end) })
}

/// El símbolo al que se refiere: el que se nombra (el más interno que tenga
/// esas líneas, si hay varios) o el más interno que contiene las líneas. Sin
/// símbolo, quedan las líneas; sin nada, el fichero entero.
/// Un símbolo sin firma y con `.` como separador: `Clase#metodo(int)` es `Clase.metodo`.
pub(crate) fn plain_symbol(name: &str) -> String {
    name.split('(').next().unwrap_or(name).trim().replace('#', ".").replace("::", ".")
}

pub(crate) fn anchor(
    file: &Path,
    source: &str,
    name: Option<&str>,
    lines: Option<(usize, usize)>,
) -> (Option<String>, (usize, usize)) {
    let total = source.lines().count().max(1);
    let symbols = huella_anchoring::Language::from_path(file)
        .and_then(|language| huella_anchoring::extract_symbols(language, source).ok())
        .map(|extraction| extraction.symbols)
        .unwrap_or_default();
    let name = name.map(plain_symbol);
    let name = name.as_deref();
    let exact = |symbol: &&huella_core::Symbol| name.is_some_and(|name| plain_symbol(&symbol.qualified_name) == name);
    let named = |symbol: &&huella_core::Symbol| {
        name.is_some_and(|name| {
            let short = symbol.qualified_name.rsplit(['.', ':']).next().unwrap_or(&symbol.qualified_name);
            let wanted = name.rsplit('.').next().unwrap_or(name);
            short == wanted
        })
    };
    let contains = |symbol: &&huella_core::Symbol| {
        lines.is_some_and(|(start, end)| symbol.start_line <= start && end.max(start) <= symbol.end_line)
    };
    let innermost = |symbol: &&huella_core::Symbol| symbol.end_line - symbol.start_line;
    // Primero el nombre exacto: el constructor de Java se llama como su clase.
    let found = symbols
        .iter()
        .filter(exact)
        .min_by_key(|symbol| (!contains(symbol), innermost(symbol)))
        .or_else(|| symbols.iter().filter(named).min_by_key(|symbol| (!contains(symbol), innermost(symbol))))
        .or_else(|| symbols.iter().filter(contains).min_by_key(innermost));
    match (found, lines) {
        (Some(symbol), _) => (Some(symbol.qualified_name.clone()), (symbol.start_line, symbol.end_line)),
        (None, Some((start, end))) => (None, (start, end.max(start))),
        (None, None) => (None, (1, total)),
    }
}

/// Una declaración en la revisión, con si el código cambió desde entonces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredView {
    #[serde(flatten)]
    pub declaration: Declaration,
    pub stale: bool,
}

/// Declaraciones de cada paso, por índice en el plan: las del mismo símbolo (o,
/// en pasos sin símbolo, las del fichero sin símbolo), la más reciente primero.
///
/// # Errors
/// Si falla la base de datos.
pub fn for_steps(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
) -> Result<BTreeMap<usize, Vec<DeclaredView>>> {
    let mut by_path: BTreeMap<usize, Vec<Declaration>> = BTreeMap::new();
    for (index, file) in review.files.iter().enumerate() {
        let mut found = store.declarations(&file.path)?;
        // La revisión de una tarea usa rutas reales; la de una sesión, las del agente.
        if let Ok(real) = std::fs::canonicalize(&file.path) {
            let real = crate::normalize_path(&real, None);
            if real != file.path {
                found.extend(store.declarations(&real)?);
            }
        }
        by_path.insert(index, found);
    }
    let mut result = BTreeMap::new();
    for (index, step) in plan.iter().enumerate() {
        let Some(candidates) = by_path.get(&step.file) else { continue };
        let current: Vec<&str> = review.files[step.file].after.lines().collect();
        let found: Vec<DeclaredView> = candidates
            .iter()
            .filter(|declaration| match (&step.symbol, &declaration.symbol) {
                (Some(symbol), Some(name)) => &symbol.qualified_name == name,
                (None, None) => true,
                _ => false,
            })
            .take(PER_STEP)
            .map(|declaration| {
                let now = match &step.symbol {
                    Some(symbol) => {
                        current.get(symbol.start_line.saturating_sub(1)..symbol.end_line.min(current.len()))
                    }
                    None => {
                        current.get(declaration.start_line.saturating_sub(1)..declaration.end_line.min(current.len()))
                    }
                }
                .map(|slice| slice.join("\n"))
                .unwrap_or_default();
                view(declaration.clone(), &now)
            })
            .collect();
        if !found.is_empty() {
            result.insert(index, found);
        }
    }
    Ok(result)
}

/// Una declaración junto al texto que tiene ahora su código: desactualizada si
/// cambió algo más que espacios.
#[must_use]
pub fn view(declaration: Declaration, current: &str) -> DeclaredView {
    DeclaredView { stale: squash(current) != squash(&declaration.snapshot), declaration }
}

/// El texto sin espacios: reformatear no desactualiza una declaración.
fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Las declaraciones en texto, para el contexto que se envía a un modelo.
#[must_use]
pub fn render(entries: &[DeclaredView]) -> String {
    let mut out = String::new();
    for entry in entries {
        let declaration = &entry.declaration;
        let stale = if entry.stale { " (el código cambió después)" } else { "" };
        let _ = writeln!(out, "- [{}{stale}] {}", kind_label(&declaration.kind), declaration.summary);
        if let Some(how) = &declaration.how {
            let _ = writeln!(out, "  Cómo funciona: {how}");
        }
        for alternative in &declaration.alternatives {
            let _ = writeln!(out, "  Descartó {}: {}", alternative.option, alternative.why_not);
        }
        for assumption in &declaration.assumptions {
            let _ = writeln!(out, "  Supone: {assumption}");
        }
        for item in &declaration.unverified {
            let _ = writeln!(out, "  Sin comprobar: {item}");
        }
        if let Some(confidence) = &declaration.confidence {
            let _ = writeln!(out, "  Confianza: {}", confidence_label(confidence));
        }
        if let Some(hint) = &declaration.review_hint {
            let _ = writeln!(out, "  Pide que se mire: {hint}");
        }
    }
    out
}

#[must_use]
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "assumption" => "suposición",
        "unverified" => "sin verificar",
        "risk" => "riesgo",
        _ => "decisión",
    }
}

#[must_use]
pub fn confidence_label(confidence: &str) -> &'static str {
    match confidence {
        "high" => "alta",
        "low" => "baja",
        _ => "media",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    const CODE: &str =
        "export class Auth {\n  login(password: string) {\n    return bcrypt.compare(password, hash);\n  }\n}\n";

    fn file(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-declared-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.ts");
        std::fs::write(&path, content).unwrap();
        std::fs::canonicalize(path).unwrap()
    }

    fn declaration(file: &Path) -> NewDeclaration {
        NewDeclaration {
            file: file.to_owned(),
            symbol: Some("login".into()),
            kind: "decision".into(),
            summary: "Comparo con bcrypt para no guardar la contraseña".into(),
            unverified: vec!["contraseñas vacías".into()],
            confidence: Some("medium".into()),
            ..NewDeclaration::default()
        }
    }

    #[test]
    fn declarations_anchor_to_the_named_or_containing_symbol() {
        let store = Store::open_in_memory().unwrap();
        let path = file("anchor", CODE);
        let named = record(&store, &declaration(&path)).unwrap();
        assert_eq!((named.symbol.as_deref(), named.lines), (Some("Auth.login"), (2, 4)));
        let by_lines =
            record(&store, &NewDeclaration { symbol: None, lines: Some((3, 3)), ..declaration(&path) }).unwrap();
        assert_eq!(by_lines.symbol.as_deref(), Some("Auth.login"));
        let bad = record(&store, &NewDeclaration { kind: "idea".into(), ..declaration(&path) });
        assert!(bad.is_err());
        let missing = record(&store, &NewDeclaration { file: path.with_file_name("no.ts"), ..declaration(&path) });
        assert!(missing.unwrap_err().to_string().contains("después de escribir"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    fn review_of(path: &Path, content: &str) -> (SessionReview, Vec<FlowStep>) {
        let edit = FileEdit {
            origin: EditOrigin { agent: "t", session_id: "s".into(), event_id: "e".into(), timestamp: None, cwd: None },
            path: path.to_owned(),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(content.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    #[test]
    fn steps_show_their_declarations_and_whether_the_code_changed() {
        let store = Store::open_in_memory().unwrap();
        let path = file("steps", CODE);
        record(&store, &declaration(&path)).unwrap();
        let (review, plan) = review_of(&path, CODE);
        let login =
            plan.iter().position(|s| s.symbol.as_ref().is_some_and(|s| s.qualified_name == "Auth.login")).unwrap();
        let found = for_steps(&store, &review, &plan).unwrap();
        assert_eq!(found[&login].len(), 1);
        assert!(!found[&login][0].stale);
        assert!(render(&found[&login]).contains("Sin comprobar: contraseñas vacías"));

        let changed = CODE.replace("compare(password, hash)", "compare(password.trim(), hash)");
        let (review, plan) = review_of(&path, &changed);
        assert!(for_steps(&store, &review, &plan).unwrap()[&login][0].stale);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
