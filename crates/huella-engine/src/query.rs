//! Consultas sobre la memoria de un fichero abierto en el editor.

use std::collections::HashMap;
use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_core::Symbol;
use huella_store::{HistoryEntry, Store, SymbolActivity, SymbolRole};

use crate::lineage::{Origin, ReadFile, Resolver, read_from_disk};
use crate::paths::normalize_path;

/// Lo que Huella recuerda de un símbolo.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolMemory {
    /// El símbolo tal como está ahora en el fichero.
    pub symbol: Symbol,
    /// Ediciones que lo tocaron, también con sus nombres o rutas anteriores,
    /// de la más reciente a la más antigua.
    pub entries: Vec<HistoryEntry>,
    /// El código actual no es el que dejó la última sesión que lo tocó.
    pub changed_since_last_session: bool,
    /// De dónde viene si se renombró o se movió.
    pub origins: Vec<Origin>,
}

/// Resumen de un símbolo para mostrarlo encima de él (`CodeLens`).
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolLens {
    pub symbol: Symbol,
    pub activity: SymbolActivity,
    pub origins: Vec<Origin>,
}

/// Memoria del símbolo en la línea `line` (1-based) de un fichero cuyo
/// contenido actual es `source`. Si el símbolo más interno no tiene historia,
/// se sube a sus contenedores (una variable local → su función → su clase).
/// Los demás ficheros se leen del disco.
///
/// # Errors
/// Si falla la base de datos o el parser.
pub fn memory_at(store: &Store, path: &Path, source: &str, line: usize) -> crate::Result<Option<SymbolMemory>> {
    memory_at_with(store, path, source, line, &read_from_disk)
}

/// Como [`memory_at`], leyendo los demás ficheros con `read`.
///
/// # Errors
/// Si falla la base de datos o el parser.
pub fn memory_at_with(
    store: &Store,
    path: &Path,
    source: &str,
    line: usize,
    read: ReadFile,
) -> crate::Result<Option<SymbolMemory>> {
    let Some(language) = Language::from_path(path) else { return Ok(None) };
    let key = normalize_path(path, None);
    let symbols = extract_symbols(language, source)?.symbols;
    let mut containing: Vec<&Symbol> =
        symbols.iter().filter(|symbol| symbol.start_line <= line && line <= symbol.end_line).collect();
    containing.sort_by_key(|symbol| std::cmp::Reverse(symbol.depth));
    let mut resolver = Resolver::new(store, read, &key, &symbols)?;
    for symbol in containing {
        if let Some(memory) = memory_of_symbol(store, &key, symbol, &mut resolver)? {
            return Ok(Some(memory));
        }
    }
    Ok(None)
}

/// Memoria del símbolo llamado `qualified_name` en un fichero con contenido
/// `source`, sin subir a sus contenedores.
///
/// # Errors
/// Si falla la base de datos o el parser.
pub fn memory_of(
    store: &Store,
    path: &Path,
    source: &str,
    qualified_name: &str,
    read: ReadFile,
) -> crate::Result<Option<SymbolMemory>> {
    let Some(language) = Language::from_path(path) else { return Ok(None) };
    let key = normalize_path(path, None);
    let symbols = extract_symbols(language, source)?.symbols;
    let Some(symbol) = symbols.iter().find(|symbol| symbol.qualified_name == qualified_name) else {
        return Ok(None);
    };
    let mut resolver = Resolver::new(store, read, &key, &symbols)?;
    memory_of_symbol(store, &key, symbol, &mut resolver)
}

/// Historia del símbolo con la de sus predecesores, si tiene alguna edición
/// que lo tocara.
fn memory_of_symbol(
    store: &Store,
    key: &str,
    symbol: &Symbol,
    resolver: &mut Resolver,
) -> crate::Result<Option<SymbolMemory>> {
    let mut entries = store.history(key, &symbol.qualified_name)?;
    let origins = resolver.origins(symbol, !entries.is_empty())?;
    for origin in &origins {
        entries.extend(store.history(&origin.path, &origin.qualified_name)?);
    }
    entries.sort_by(|a, b| b.at.cmp(&a.at));
    let Some(last) = entries.iter().find(|entry| entry.role == SymbolRole::Touched) else { return Ok(None) };
    // La forma sin el nombre también vale: renombrar no es cambiar el código.
    let unchanged = last.body_hash == symbol.body_hash
        || last.shape_hash.as_deref() == Some(symbol.fingerprint.shape_hash.as_str());
    Ok(Some(SymbolMemory { symbol: symbol.clone(), entries, changed_since_last_session: !unchanged, origins }))
}

/// Símbolos del fichero con historia (también la de sus nombres o rutas
/// anteriores), en orden de aparición.
///
/// # Errors
/// Si falla la base de datos o el parser.
pub fn lenses(store: &Store, path: &Path, source: &str) -> crate::Result<Vec<SymbolLens>> {
    lenses_with(store, path, source, &read_from_disk)
}

/// Como [`lenses`], leyendo los demás ficheros con `read`.
///
/// # Errors
/// Si falla la base de datos o el parser.
pub fn lenses_with(store: &Store, path: &Path, source: &str, read: ReadFile) -> crate::Result<Vec<SymbolLens>> {
    let Some(language) = Language::from_path(path) else { return Ok(Vec::new()) };
    let key = normalize_path(path, None);
    let mut activity = store.activity(&key)?;
    let symbols = extract_symbols(language, source)?.symbols;
    let mut resolver = Resolver::new(store, read, &key, &symbols)?;
    let mut elsewhere: HashMap<String, HashMap<String, SymbolActivity>> = HashMap::new();
    let mut lenses = Vec::new();
    for symbol in &symbols {
        let direct = activity.remove(&symbol.qualified_name);
        let origins = resolver.origins(symbol, direct.is_some())?;
        let mut total = direct.clone();
        for origin in &origins {
            if !elsewhere.contains_key(&origin.path) {
                elsewhere.insert(origin.path.clone(), store.activity(&origin.path)?);
            }
            if let Some(previous) = elsewhere.get(&origin.path).and_then(|by_name| by_name.get(&origin.qualified_name))
            {
                total = Some(merge(total, previous));
            }
        }
        if let Some(activity) = total {
            lenses.push(SymbolLens { symbol: symbol.clone(), activity, origins });
        }
    }
    Ok(lenses)
}

/// Suma la actividad de un predecesor (las sesiones pueden repetirse: es una
/// cota superior, suficiente para el resumen).
fn merge(total: Option<SymbolActivity>, previous: &SymbolActivity) -> SymbolActivity {
    let Some(total) = total else { return previous.clone() };
    SymbolActivity {
        edits: total.edits + previous.edits,
        sessions: total.sessions + previous.sessions,
        last_at: total.last_at.max(previous.last_at.clone()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_anchoring::anchor_edit;
    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit, TranscriptCursor};
    use huella_store::IndexedEdit;

    use super::*;
    use crate::lineage::Link;

    const V1: &str = "export class UserService {\n  register(email: string) {\n    return save(email);\n  }\n}\n";
    const V2: &str =
        "export class UserService {\n  register(email: string) {\n    return save(email.trim());\n  }\n}\n";

    fn store_with(edits: &[(&str, Option<&str>, &str, &str)]) -> Store {
        let at: Vec<_> =
            edits.iter().map(|(id, before, after, prompt)| (*id, "/repo/user.ts", *before, *after, *prompt)).collect();
        store_at(&at)
    }

    fn store_at(edits: &[(&str, &str, Option<&str>, &str, &str)]) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        for (index, (id, path, before, after, prompt)) in edits.iter().enumerate() {
            let edit = FileEdit {
                origin: EditOrigin {
                    agent: "test",
                    session_id: format!("s{index}"),
                    event_id: (*id).into(),
                    timestamp: Some(format!("2026-10-2{index}T10:00:00Z")),
                    cwd: None,
                },
                path: PathBuf::from(path),
                kind: EditKind::Replace,
                before: before.map(str::to_owned),
                after: AfterState::Known((*after).into()),
                user_modified: false,
                context: EditContext { prompt: Some((*prompt).into()), rationale: None },
            };
            let report = anchor_edit(&edit, |_| None);
            let indexed =
                [IndexedEdit { edit: &edit, path, outcome: &report.outcome, changed_lines: &report.changed_lines }];
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
    fn no_memory_outside_symbols_or_for_copies() {
        let store = store_with(&[("e1", None, V1, "crea el servicio")]);
        // El original sigue en su sitio: es una copia, no un movimiento.
        let read = files(&[("/repo/user.ts", V1), ("/repo/otro.ts", V1)]);
        assert_eq!(memory_at_with(&store, Path::new("/repo/otro.ts"), V1, 2, &read).unwrap(), None);
        assert_eq!(memory_at(&store, Path::new("/repo/README.md"), "# hola", 1).unwrap(), None);
    }

    #[test]
    fn lenses_list_symbols_with_activity() {
        let store = store_with(&[("e1", None, V1, "crea"), ("e2", Some(V1), V2, "recorta")]);
        let lenses = lenses(&store, Path::new("/repo/user.ts"), V2).unwrap();
        let summary: Vec<_> = lenses.iter().map(|l| (l.symbol.qualified_name.as_str(), l.activity.edits)).collect();
        assert_eq!(summary, vec![("UserService", 1), ("UserService.register", 2)]);
    }

    /// Lector de ficheros en memoria: lo que no está, no existe.
    fn files(contents: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let contents: HashMap<String, String> =
            contents.iter().map(|(path, content)| ((*path).to_owned(), (*content).to_owned())).collect();
        move |path| contents.get(path).cloned()
    }

    fn origin_of(memory: &SymbolMemory) -> (Link, &str, &str) {
        let origin = &memory.origins[0];
        (origin.link, origin.path.as_str(), origin.qualified_name.as_str())
    }

    #[test]
    fn a_symbol_renamed_by_hand_keeps_its_memory() {
        let store = store_with(&[("e1", None, V1, "crea el servicio")]);
        let renamed = V1.replace("register", "signUp");
        let read = files(&[("/repo/user.ts", &renamed)]);
        let memory = memory_at_with(&store, Path::new("/repo/user.ts"), &renamed, 3, &read).unwrap().unwrap();
        assert_eq!(memory.symbol.qualified_name, "UserService.signUp");
        assert_eq!(origin_of(&memory), (Link::Renamed, "/repo/user.ts", "UserService.register"));
        assert_eq!(memory.entries[0].context.prompt.as_deref(), Some("crea el servicio"));
        assert!(!memory.changed_since_last_session, "renombrar no es cambiar el código");
    }

    #[test]
    fn a_moved_file_keeps_its_memory() {
        let store = store_with(&[("e1", None, V1, "crea el servicio")]);
        let read = files(&[("/repo/src/users/user.ts", V1)]);
        let memory = memory_at_with(&store, Path::new("/repo/src/users/user.ts"), V1, 3, &read).unwrap().unwrap();
        assert_eq!(origin_of(&memory), (Link::Moved, "/repo/user.ts", "UserService.register"));
        let lenses = lenses_with(&store, Path::new("/repo/src/users/user.ts"), V1, &read).unwrap();
        assert_eq!(lenses.len(), 2, "la clase y el método");
    }

    #[test]
    fn a_symbol_moved_to_another_file_keeps_its_memory() {
        const HELPERS: &str = "export function normalize(email: string) {\n  return email.trim().toLowerCase();\n}\n";
        let with_helper = format!("{V1}\n{HELPERS}");
        let store = store_with(&[("e1", None, &with_helper, "crea el servicio y el helper")]);
        let read = files(&[("/repo/user.ts", V1), ("/repo/email.ts", HELPERS)]);
        let memory = memory_at_with(&store, Path::new("/repo/email.ts"), HELPERS, 2, &read).unwrap().unwrap();
        assert_eq!(origin_of(&memory), (Link::Moved, "/repo/user.ts", "normalize"));
    }

    #[test]
    fn a_rename_by_the_agent_chains_the_old_history() {
        let renamed = V1.replace("register", "signUp");
        let store = store_with(&[("e1", None, V1, "crea el servicio"), ("e2", Some(V1), &renamed, "renómbralo")]);
        let read = files(&[("/repo/user.ts", &renamed)]);
        let memory = memory_at_with(&store, Path::new("/repo/user.ts"), &renamed, 3, &read).unwrap().unwrap();
        assert_eq!(origin_of(&memory), (Link::Renamed, "/repo/user.ts", "UserService.register"));
        let prompts: Vec<_> = memory.entries.iter().filter_map(|e| e.context.prompt.as_deref()).collect();
        assert!(prompts.contains(&"crea el servicio"), "{prompts:?}");
    }

    #[test]
    fn two_renames_at_once_do_not_cross() {
        const TWO: &str = "export function parseUser(raw: string) {\n  const [name, mail] = raw.split(',');\n  return { name, mail };\n}\n\nexport function priceOf(items: number[]) {\n  return items.reduce((a, b) => a + b, 0) * 1.21;\n}\n";
        let store = store_with(&[("e1", None, TWO, "crea")]);
        let renamed = TWO.replace("parseUser", "readUser").replace("priceOf", "totalWithTax");
        let read = files(&[("/repo/user.ts", &renamed)]);
        let lenses = lenses_with(&store, Path::new("/repo/user.ts"), &renamed, &read).unwrap();
        let pairs: Vec<_> =
            lenses.iter().map(|l| (l.symbol.qualified_name.as_str(), l.origins[0].qualified_name.as_str())).collect();
        assert_eq!(pairs, vec![("readUser", "parseUser"), ("totalWithTax", "priceOf")]);
    }

    #[test]
    fn new_unrelated_code_does_not_inherit_memory() {
        let store = store_with(&[("e1", None, V1, "crea el servicio")]);
        let replaced = "export class UserService {\n  audit(events: string[]) {\n    for (const e of events) log.write(e, Date.now());\n  }\n}\n";
        let read = files(&[("/repo/user.ts", replaced)]);
        let memory = memory_at_with(&store, Path::new("/repo/user.ts"), replaced, 3, &read).unwrap().unwrap();
        assert_eq!(memory.symbol.qualified_name, "UserService", "sube a la clase, que sí tiene memoria");
        assert_eq!(memory.origins, Vec::new());
    }
}
