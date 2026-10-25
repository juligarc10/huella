//! Anclaje de una edición a los símbolos que tocó.
//!
//! Se compara el fichero antes y después línea a línea; cada línea cambiada se
//! asigna al símbolo más interno que la contiene, y cada borrado al símbolo que
//! rodea el punto donde estaba el texto. Lo que no cae en ningún símbolo es
//! código de nivel de módulo (imports, sentencias sueltas).

use std::path::Path;

use huella_core::{AfterState, AnchorOutcome, Confidence, FileEdit, Symbol};
use similar::{DiffTag, TextDiff};

use crate::language::Language;
use crate::symbols::extract_symbols;

/// Resultado de anclar una edición, con el contexto necesario para informar.
#[derive(Debug, Clone)]
pub struct AnchorReport {
    pub language: Option<Language>,
    pub outcome: AnchorOutcome,
    /// Rangos de líneas cambiadas en el fichero final (1-based, inclusivos).
    pub changed_lines: Vec<(usize, usize)>,
    pub has_syntax_errors: bool,
}

impl AnchorReport {
    fn new(language: Option<Language>, outcome: AnchorOutcome) -> Self {
        Self { language, outcome, changed_lines: Vec::new(), has_syntax_errors: false }
    }
}

/// Ancla una edición. `read_current` lee el fichero actual del disco; solo se
/// usa si la transcripción no permite reconstruir el fichero final.
pub fn anchor_edit(edit: &FileEdit, read_current: impl Fn(&Path) -> Option<String>) -> AnchorReport {
    let Some(language) = Language::from_path(&edit.path) else {
        return AnchorReport::new(None, AnchorOutcome::FileOnly);
    };
    let unresolved =
        |reason: &str| AnchorReport::new(Some(language), AnchorOutcome::Unresolved { reason: reason.into() });

    let (after, changes, confidence) = match &edit.after {
        AfterState::Known(after) => {
            let changes = diff_changes(edit.before.as_deref().unwrap_or_default(), after);
            (after.clone(), changes, Confidence::Exact)
        }
        AfterState::FragmentOnly(fragment) => {
            if fragment.trim().is_empty() {
                return unresolved("borrado sin fichero original en la transcripción");
            }
            let Some(current) = read_current(&edit.path) else {
                return unresolved("sin fichero original y el fichero ya no existe en disco");
            };
            let Some(changes) = locate_fragment(&current, fragment) else {
                return unresolved("sin fichero original y el fragmento ya no está en disco");
            };
            (current, changes, Confidence::Approximate)
        }
    };
    if changes.is_empty() {
        return AnchorReport::new(Some(language), AnchorOutcome::NoChange);
    }

    let extraction = match extract_symbols(language, &after) {
        Ok(extraction) => extraction,
        Err(error) => return unresolved(&error.to_string()),
    };
    let lines = Lines::new(&after);
    let (symbols, also_module) = assign(&changes, &lines, &extraction.symbols);
    let outcome = if symbols.is_empty() {
        AnchorOutcome::ModuleOnly { confidence }
    } else {
        AnchorOutcome::Symbols { symbols, also_module, confidence }
    };
    AnchorReport {
        language: Some(language),
        outcome,
        changed_lines: changed_line_ranges(&changes, lines.count()),
        has_syntax_errors: extraction.has_syntax_errors,
    }
}

/// Cambio en el fichero final, en índices de línea 0-based.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    /// Líneas insertadas o modificadas.
    Lines(std::ops::Range<usize>),
    /// Se borró texto justo antes de esta línea.
    DeletionBefore(usize),
}

fn diff_changes(before: &str, after: &str) -> Vec<Change> {
    TextDiff::from_lines(before, after)
        .ops()
        .iter()
        .filter_map(|op| {
            let (tag, _, new_range) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => None,
                DiffTag::Delete => Some(Change::DeletionBefore(new_range.start)),
                DiffTag::Insert | DiffTag::Replace => Some(Change::Lines(new_range)),
            }
        })
        .collect()
}

/// Localiza el texto insertado en el fichero actual. Si aparece varias veces
/// no se puede saber cuál fue: se descarta.
fn locate_fragment(current: &str, fragment: &str) -> Option<Vec<Change>> {
    let start = current.find(fragment)?;
    if current[start + 1..].contains(fragment) {
        return None;
    }
    let first_line = current[..start].matches('\n').count();
    let last_line = first_line + fragment.trim_end_matches('\n').matches('\n').count();
    Some(vec![Change::Lines(first_line..last_line + 1)])
}

/// Posiciones de cada línea del texto, compatibles con el diff por líneas.
struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1).filter(|&i| i < text.len()));
        Self { text, starts }
    }

    fn count(&self) -> usize {
        if self.text.is_empty() { 0 } else { self.starts.len() }
    }

    /// Byte donde empieza la línea `index`, o el final del texto.
    fn start_of(&self, index: usize) -> usize {
        self.starts.get(index).copied().unwrap_or(self.text.len())
    }

    /// Byte del primer carácter que no es espacio de la línea `index`.
    /// `None` si la línea está en blanco.
    fn first_content_byte(&self, index: usize) -> Option<usize> {
        let start = self.start_of(index);
        let end = self.starts.get(index + 1).copied().unwrap_or(self.text.len());
        let line = &self.text[start..end];
        let leading = line.len() - line.trim_start().len();
        (leading < line.len()).then_some(start + leading)
    }
}

/// Asigna cada cambio a su símbolo más interno. Devuelve los símbolos tocados
/// (sin repetir, en orden) y si algún cambio quedó fuera de todo símbolo.
fn assign(changes: &[Change], lines: &Lines, symbols: &[Symbol]) -> (Vec<Symbol>, bool) {
    let mut touched: Vec<usize> = Vec::new();
    let mut also_module = false;
    let mut record = |found: Option<usize>| match found {
        Some(index) if !touched.contains(&index) => touched.push(index),
        Some(_) => {}
        None => also_module = true,
    };
    for change in changes {
        match change {
            Change::Lines(range) => {
                for index in range.clone() {
                    // La línea pertenece al símbolo donde empieza su contenido: así un
                    // comentario al final (`const x = 1; // nota`) no la saca del símbolo.
                    if let Some(first) = lines.first_content_byte(index) {
                        record(innermost(symbols, |s| s.byte_range.contains(&first)));
                    }
                }
            }
            Change::DeletionBefore(index) => {
                let point = lines.start_of(*index);
                record(innermost(symbols, |s| s.byte_range.start < point && point < s.byte_range.end));
            }
        }
    }
    touched.sort_unstable();
    (touched.into_iter().map(|index| symbols[index].clone()).collect(), also_module)
}

fn innermost(symbols: &[Symbol], contains: impl Fn(&Symbol) -> bool) -> Option<usize> {
    symbols
        .iter()
        .enumerate()
        .filter(|(_, symbol)| contains(symbol))
        .max_by_key(|(_, symbol)| (symbol.depth, std::cmp::Reverse(symbol.byte_range.len())))
        .map(|(index, _)| index)
}

/// Une los cambios en rangos de líneas 1-based para mostrarlos.
fn changed_line_ranges(changes: &[Change], line_count: usize) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = changes
        .iter()
        .map(|change| match change {
            Change::Lines(range) => (range.start + 1, range.end.max(range.start + 1)),
            Change::DeletionBefore(index) => {
                let line = (index + 1).min(line_count.max(1));
                (line, line)
            }
        })
        .collect();
    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{EditKind, EditOrigin};

    use super::*;

    const SERVICE: &str = "import { db } from './db';\n\nexport class UserService {\n  register(email: string) {\n    return db.insert(email);\n  }\n\n  remove(id: string) {\n    return db.delete(id);\n  }\n}\n";

    fn edit(path: &str, before: Option<&str>, after: AfterState) -> FileEdit {
        FileEdit {
            origin: EditOrigin {
                agent: "test",
                session_id: "s".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from(path),
            kind: EditKind::Replace,
            before: before.map(str::to_owned),
            after,
            user_modified: false,
        }
    }

    fn anchor_known(before: Option<&str>, after: &str) -> AnchorReport {
        anchor_edit(&edit("src/user.ts", before, AfterState::Known(after.into())), |_| None)
    }

    fn anchored_names(report: &AnchorReport) -> (Vec<String>, bool) {
        match &report.outcome {
            AnchorOutcome::Symbols { symbols, also_module, .. } => {
                (symbols.iter().map(|s| s.qualified_name.clone()).collect(), *also_module)
            }
            other => panic!("se esperaban símbolos, llegó {other:?}"),
        }
    }

    #[test]
    fn change_inside_a_method_anchors_to_that_method() {
        let after = SERVICE.replace("db.insert(email)", "db.insert(email.trim())");
        let report = anchor_known(Some(SERVICE), &after);
        assert_eq!(anchored_names(&report), (vec!["UserService.register".into()], false));
        assert_eq!(report.changed_lines, vec![(5, 5)]);
        assert_eq!(report.outcome.confidence(), Some(Confidence::Exact));
    }

    #[test]
    fn import_change_is_module_level() {
        let after = SERVICE.replace("'./db'", "'./database'");
        let report = anchor_known(Some(SERVICE), &after);
        assert!(matches!(report.outcome, AnchorOutcome::ModuleOnly { confidence: Confidence::Exact }));
    }

    #[test]
    fn change_spanning_two_methods_and_an_import_anchors_all() {
        let after = SERVICE
            .replace("'./db'", "'./database'")
            .replace("db.insert(email)", "db.add(email)")
            .replace("db.delete(id)", "db.drop(id)");
        let report = anchor_known(Some(SERVICE), &after);
        assert_eq!(anchored_names(&report), (vec!["UserService.register".into(), "UserService.remove".into()], true));
    }

    #[test]
    fn deleting_a_method_anchors_to_its_class() {
        let after = SERVICE.replace("\n  remove(id: string) {\n    return db.delete(id);\n  }\n", "");
        let report = anchor_known(Some(SERVICE), &after);
        assert_eq!(anchored_names(&report), (vec!["UserService".into()], false));
    }

    #[test]
    fn new_file_anchors_every_top_level_symbol_and_the_module() {
        let report = anchor_known(None, SERVICE);
        let (names, also_module) = anchored_names(&report);
        assert_eq!(names, vec!["UserService", "UserService.register", "UserService.remove"]);
        assert!(also_module, "el import es código de módulo");
    }

    #[test]
    fn trailing_comment_does_not_push_a_line_out_of_its_symbol() {
        let before = "const DURATION = 1000;\n";
        let after = "const DURATION = 18000; // ms por punto\n";
        assert_eq!(anchored_names(&anchor_known(Some(before), after)), (vec!["DURATION".into()], false));
    }

    #[test]
    fn identical_content_is_no_change() {
        assert!(matches!(anchor_known(Some(SERVICE), SERVICE).outcome, AnchorOutcome::NoChange));
    }

    #[test]
    fn unsupported_language_anchors_to_the_file() {
        let report = anchor_edit(&edit("README.md", None, AfterState::Known("# Hola".into())), |_| None);
        assert!(matches!(report.outcome, AnchorOutcome::FileOnly));
        assert_eq!(report.language, None);
    }

    #[test]
    fn fragment_is_located_on_disk_with_approximate_confidence() {
        let fragment = AfterState::FragmentOnly("return db.delete(id);".into());
        let report = anchor_edit(&edit("src/user.ts", None, fragment), |_| Some(SERVICE.into()));
        assert_eq!(anchored_names(&report), (vec!["UserService.remove".into()], false));
        assert_eq!(report.outcome.confidence(), Some(Confidence::Approximate));
        assert_eq!(report.changed_lines, vec![(9, 9)]);
    }

    #[test]
    fn fragment_missing_or_ambiguous_on_disk_is_unresolved() {
        let missing = edit("src/user.ts", None, AfterState::FragmentOnly("nope()".into()));
        assert!(matches!(anchor_edit(&missing, |_| Some(SERVICE.into())).outcome, AnchorOutcome::Unresolved { .. }));
        let ambiguous = edit("src/user.ts", None, AfterState::FragmentOnly("return db".into()));
        assert!(matches!(anchor_edit(&ambiguous, |_| Some(SERVICE.into())).outcome, AnchorOutcome::Unresolved { .. }));
        let gone = edit("src/user.ts", None, AfterState::FragmentOnly("x".into()));
        assert!(matches!(anchor_edit(&gone, |_| None).outcome, AnchorOutcome::Unresolved { .. }));
    }

    #[test]
    fn lines_handle_trailing_newline_and_blank_lines() {
        let lines = Lines::new("a\n\n  b  \n");
        assert_eq!(lines.count(), 3);
        assert_eq!(lines.first_content_byte(0), Some(0));
        assert_eq!(lines.first_content_byte(1), None);
        assert_eq!(lines.first_content_byte(2), Some(5));
        assert_eq!(Lines::new("").count(), 0);
    }
}
