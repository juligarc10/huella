//! Consultas sobre la memoria de un fichero abierto en el editor.

use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_core::Symbol;
use huella_store::{HistoryEntry, Store, SymbolActivity, SymbolRole};

use crate::paths::normalize_path;

/// Lo que Huella recuerda de un símbolo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolMemory {
    /// El símbolo tal como está ahora en el fichero.
    pub symbol: Symbol,
    /// Ediciones que lo tocaron, de la más reciente a la más antigua.
    pub entries: Vec<HistoryEntry>,
    /// El código actual no es el que dejó la última sesión que lo tocó.
    pub changed_since_last_session: bool,
}

/// Resumen de un símbolo para mostrarlo encima de él (`CodeLens`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolLens {
    pub symbol: Symbol,
    pub activity: SymbolActivity,
}

/// Memoria del símbolo en la línea `line` (1-based) de un fichero cuyo
/// contenido actual es `source`. Si el símbolo más interno no tiene historia,
/// se sube a sus contenedores (una variable local → su función → su clase).
///
/// # Errors
/// Si falla la base de datos o el parser.
pub fn memory_at(store: &Store, path: &Path, source: &str, line: usize) -> crate::Result<Option<SymbolMemory>> {
    let Some(language) = Language::from_path(path) else { return Ok(None) };
    let key = normalize_path(path, None);
    let mut containing: Vec<Symbol> = extract_symbols(language, source)?
        .symbols
        .into_iter()
        .filter(|symbol| symbol.start_line <= line && line <= symbol.end_line)
        .collect();
    containing.sort_by_key(|symbol| std::cmp::Reverse(symbol.depth));
    for symbol in containing {
        let entries = store.history(&key, &symbol.qualified_name)?;
        let last_touch = entries.iter().find(|entry| entry.role == SymbolRole::Touched);
        if let Some(last) = last_touch {
            let changed_since_last_session = last.body_hash != symbol.body_hash;
            return Ok(Some(SymbolMemory { symbol, entries, changed_since_last_session }));
        }
    }
    Ok(None)
}

/// Símbolos del fichero con historia, en orden de aparición.
///
/// # Errors
/// Si falla la base de datos o el parser.
pub fn lenses(store: &Store, path: &Path, source: &str) -> crate::Result<Vec<SymbolLens>> {
    let Some(language) = Language::from_path(path) else { return Ok(Vec::new()) };
    let mut activity = store.activity(&normalize_path(path, None))?;
    if activity.is_empty() {
        return Ok(Vec::new());
    }
    let lenses = extract_symbols(language, source)?
        .symbols
        .into_iter()
        .filter_map(|symbol| activity.remove(&symbol.qualified_name).map(|activity| SymbolLens { symbol, activity }))
        .collect();
    Ok(lenses)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_anchoring::anchor_edit;
    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit, TranscriptCursor};
    use huella_store::IndexedEdit;

    use super::*;

    const V1: &str = "export class UserService {\n  register(email: string) {\n    return save(email);\n  }\n}\n";
    const V2: &str =
        "export class UserService {\n  register(email: string) {\n    return save(email.trim());\n  }\n}\n";

    fn store_with(edits: &[(&str, Option<&str>, &str, &str)]) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        for (index, (id, before, after, prompt)) in edits.iter().enumerate() {
            let edit = FileEdit {
                origin: EditOrigin {
                    agent: "test",
                    session_id: format!("s{index}"),
                    event_id: (*id).into(),
                    timestamp: Some(format!("2026-10-2{index}T10:00:00Z")),
                    cwd: None,
                },
                path: PathBuf::from("/repo/user.ts"),
                kind: EditKind::Replace,
                before: before.map(str::to_owned),
                after: AfterState::Known((*after).into()),
                user_modified: false,
                context: EditContext { prompt: Some((*prompt).into()), rationale: None },
            };
            let report = anchor_edit(&edit, |_| None);
            let indexed = [IndexedEdit {
                edit: &edit,
                path: "/repo/user.ts",
                outcome: &report.outcome,
                changed_lines: &report.changed_lines,
            }];
            store.save(&format!("/t/{id}"), &TranscriptCursor::default(), &indexed).unwrap();
        }
        store
    }

    #[test]
    fn memory_of_the_symbol_under_the_cursor_with_drift() {
        let store = store_with(&[("e1", None, V1, "crea el servicio"), ("e2", Some(V1), V2, "recorta el email")]);
        let memory = memory_at(&store, Path::new("/repo/user.ts"), V2, 3).unwrap().unwrap();
        assert_eq!(memory.symbol.qualified_name, "UserService.register");
        let prompts: Vec<_> = memory.entries.iter().map(|e| e.context.prompt.clone().unwrap()).collect();
        assert_eq!(prompts, vec!["recorta el email", "crea el servicio"]);
        assert!(!memory.changed_since_last_session);

        let edited_by_hand = V2.replace("trim()", "toLowerCase()");
        let memory = memory_at(&store, Path::new("/repo/user.ts"), &edited_by_hand, 3).unwrap().unwrap();
        assert!(memory.changed_since_last_session);
    }

    #[test]
    fn climbs_to_the_closest_container_with_history() {
        let store = store_with(&[("e1", None, V1, "crea el servicio")]);
        let with_helper = V1.replace("return save(email);", "const clean = () => email;\n    return save(clean());");
        let memory = memory_at(&store, Path::new("/repo/user.ts"), &with_helper, 3).unwrap().unwrap();
        assert_eq!(memory.symbol.qualified_name, "UserService.register");
    }

    #[test]
    fn no_memory_outside_symbols_or_for_unknown_files() {
        let store = store_with(&[("e1", None, V1, "crea el servicio")]);
        assert_eq!(memory_at(&store, Path::new("/repo/otro.ts"), V1, 2).unwrap(), None);
        assert_eq!(memory_at(&store, Path::new("/repo/README.md"), "# hola", 1).unwrap(), None);
    }

    #[test]
    fn lenses_list_symbols_with_activity() {
        let store = store_with(&[("e1", None, V1, "crea"), ("e2", Some(V1), V2, "recorta")]);
        let lenses = lenses(&store, Path::new("/repo/user.ts"), V2).unwrap();
        let summary: Vec<_> = lenses.iter().map(|l| (l.symbol.qualified_name.as_str(), l.activity.edits)).collect();
        assert_eq!(summary, vec![("UserService", 1), ("UserService.register", 2)]);
    }
}
