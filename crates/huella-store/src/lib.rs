//! Memoria persistente de Huella en SQLite.
//!
//! Un único fichero guarda, por transcripción, hasta dónde se ha leído; por
//! edición, su contexto y los símbolos que tocó o borró. Las ediciones se
//! identifican por el id del evento del agente, así que volver a indexar la
//! misma transcripción no duplica nada.

use std::collections::HashMap;
use std::path::Path;

use huella_core::{AnchorOutcome, Confidence, EditContext, EditKind, FileEdit, SymbolKind, TranscriptCursor};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("error de SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("no se pudo crear el directorio de la base de datos {path}: {source}")]
    CreateDir { path: String, source: std::io::Error },
    #[error(
        "la base de datos tiene la versión de esquema {found}, más nueva que la que entiende este binario ({supported})"
    )]
    NewerSchema { found: i64, supported: i64 },
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Migraciones en orden; la posición + 1 es la versión de esquema.
const MIGRATIONS: &[&str] = &[r"
    CREATE TABLE transcript (
        path        TEXT PRIMARY KEY,
        offset      INTEGER NOT NULL,
        prompt      TEXT,
        rationale   TEXT
    );
    CREATE TABLE session (
        id          TEXT PRIMARY KEY,
        agent       TEXT NOT NULL,
        cwd         TEXT,
        first_at    TEXT,
        last_at     TEXT
    );
    CREATE TABLE edit (
        id                TEXT PRIMARY KEY,
        session_id        TEXT NOT NULL REFERENCES session(id),
        at                TEXT,
        path              TEXT NOT NULL,
        kind              TEXT NOT NULL,
        status            TEXT NOT NULL,
        confidence        TEXT,
        module_level      INTEGER NOT NULL DEFAULT 0,
        unresolved_reason TEXT,
        prompt            TEXT,
        rationale         TEXT,
        changed_lines     TEXT NOT NULL
    );
    CREATE INDEX edit_by_path ON edit(path, at);
    CREATE TABLE edit_symbol (
        edit_id         TEXT NOT NULL REFERENCES edit(id) ON DELETE CASCADE,
        role            TEXT NOT NULL CHECK (role IN ('touched', 'removed')),
        qualified_name  TEXT NOT NULL,
        kind            TEXT NOT NULL,
        start_line      INTEGER NOT NULL,
        end_line        INTEGER NOT NULL,
        body_hash       TEXT NOT NULL,
        PRIMARY KEY (edit_id, role, qualified_name)
    );
    CREATE INDEX edit_symbol_by_name ON edit_symbol(qualified_name);
"];

/// Edición ya anclada, lista para guardar.
pub struct IndexedEdit<'a> {
    pub edit: &'a FileEdit,
    /// Ruta absoluta y normalizada con la que se consultará después.
    pub path: &'a str,
    pub outcome: &'a AnchorOutcome,
    pub changed_lines: &'a [(usize, usize)],
}

/// Qué papel tuvo un símbolo en una edición.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolRole {
    Touched,
    Removed,
}

/// Una edición que tocó un símbolo, con su contexto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub edit_id: String,
    pub session_id: String,
    pub at: Option<String>,
    pub role: SymbolRole,
    pub kind: String,
    pub confidence: Option<String>,
    /// Hash del cuerpo del símbolo justo después de esa edición.
    pub body_hash: String,
    pub context: EditContext,
}

/// Resumen de la historia de un símbolo para mostrarlo sobre él.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolActivity {
    pub edits: usize,
    pub sessions: usize,
    pub last_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreStats {
    pub transcripts: usize,
    pub sessions: usize,
    pub edits: usize,
    pub anchored_symbols: usize,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Abre (o crea) la base de datos y aplica las migraciones pendientes.
    ///
    /// # Errors
    /// Si el fichero no se puede crear o abrir, o su esquema es más nuevo que
    /// el de este binario.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| StoreError::CreateDir { path: parent.display().to_string(), source })?;
        }
        Self::init(Connection::open(path)?)
    }

    /// Base de datos en memoria, para tests.
    ///
    /// # Errors
    /// Si SQLite no puede inicializarla.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // WAL: el servidor del editor puede leer mientras otro proceso indexa.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let supported = i64::try_from(MIGRATIONS.len()).expect("pocas migraciones");
        let current: i64 = self.conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if current > supported {
            return Err(StoreError::NewerSchema { found: current, supported });
        }
        let tx = self.conn.transaction()?;
        for (version, sql) in (1..).zip(MIGRATIONS).skip(usize::try_from(current).unwrap_or_default()) {
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", version)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Hasta dónde se leyó una transcripción.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn cursor(&self, transcript: &str) -> Result<Option<TranscriptCursor>> {
        let cursor = self
            .conn
            .query_row("SELECT offset, prompt, rationale FROM transcript WHERE path = ?1", [transcript], |row| {
                Ok(TranscriptCursor {
                    offset: row.get::<_, i64>(0)?.try_into().unwrap_or_default(),
                    context: EditContext { prompt: row.get(1)?, rationale: row.get(2)? },
                })
            })
            .optional()?;
        Ok(cursor)
    }

    /// Guarda las ediciones nuevas de una transcripción y su cursor en una sola
    /// transacción: o se guarda todo o nada, y el cursor nunca avanza sobre
    /// ediciones perdidas.
    ///
    /// # Errors
    /// Si falla alguna escritura; en ese caso no se guarda nada.
    pub fn save(&mut self, transcript: &str, cursor: &TranscriptCursor, edits: &[IndexedEdit]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for indexed in edits {
            insert_edit(&tx, indexed)?;
        }
        tx.execute(
            "INSERT INTO transcript (path, offset, prompt, rationale) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET offset = excluded.offset, prompt = excluded.prompt,
                                             rationale = excluded.rationale",
            params![
                transcript,
                i64::try_from(cursor.offset).unwrap_or(i64::MAX),
                cursor.context.prompt,
                cursor.context.rationale
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Ediciones que tocaron o borraron un símbolo, de la más reciente a la más antigua.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn history(&self, path: &str, qualified_name: &str) -> Result<Vec<HistoryEntry>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT e.id, e.session_id, e.at, s.role, e.kind, e.confidence, s.body_hash, e.prompt, e.rationale
             FROM edit_symbol s JOIN edit e ON e.id = s.edit_id
             WHERE e.path = ?1 AND s.qualified_name = ?2
             ORDER BY e.at DESC, e.id DESC",
        )?;
        let rows = statement.query_map(params![path, qualified_name], |row| {
            Ok(HistoryEntry {
                edit_id: row.get(0)?,
                session_id: row.get(1)?,
                at: row.get(2)?,
                role: if row.get::<_, String>(3)? == "removed" { SymbolRole::Removed } else { SymbolRole::Touched },
                kind: row.get(4)?,
                confidence: row.get(5)?,
                body_hash: row.get(6)?,
                context: EditContext { prompt: row.get(7)?, rationale: row.get(8)? },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Actividad de cada símbolo de un fichero, por nombre cualificado.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn activity(&self, path: &str) -> Result<HashMap<String, SymbolActivity>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT s.qualified_name, COUNT(DISTINCT e.id), COUNT(DISTINCT e.session_id), MAX(e.at)
             FROM edit_symbol s JOIN edit e ON e.id = s.edit_id
             WHERE e.path = ?1 AND s.role = 'touched'
             GROUP BY s.qualified_name",
        )?;
        let rows = statement.query_map([path], |row| {
            let count = |index| row.get::<_, i64>(index).map(|n| usize::try_from(n).unwrap_or_default());
            Ok((row.get(0)?, SymbolActivity { edits: count(1)?, sessions: count(2)?, last_at: row.get(3)? }))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// # Errors
    /// Si falla la consulta.
    pub fn stats(&self) -> Result<StoreStats> {
        let count = |sql: &str| -> Result<usize> {
            let n: i64 = self.conn.query_row(sql, [], |row| row.get(0))?;
            Ok(usize::try_from(n).unwrap_or_default())
        };
        Ok(StoreStats {
            transcripts: count("SELECT COUNT(*) FROM transcript")?,
            sessions: count("SELECT COUNT(*) FROM session")?,
            edits: count("SELECT COUNT(*) FROM edit")?,
            anchored_symbols: count("SELECT COUNT(DISTINCT qualified_name) FROM edit_symbol")?,
        })
    }
}

fn insert_edit(tx: &Transaction, indexed: &IndexedEdit) -> Result<()> {
    let edit = indexed.edit;
    let origin = &edit.origin;
    tx.execute(
        "INSERT INTO session (id, agent, cwd, first_at, last_at) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(id) DO UPDATE SET
             first_at = MIN(COALESCE(first_at, excluded.first_at), excluded.first_at),
             last_at = MAX(COALESCE(last_at, excluded.last_at), excluded.last_at)",
        params![
            origin.session_id,
            origin.agent,
            origin.cwd.as_ref().map(|cwd| cwd.display().to_string()),
            origin.timestamp
        ],
    )?;

    let (status, anchor, reason) = match indexed.outcome {
        AnchorOutcome::Anchored(anchor) => ("anchored", Some(anchor), None),
        AnchorOutcome::FileOnly => ("file_only", None, None),
        AnchorOutcome::NoChange => ("no_change", None, None),
        AnchorOutcome::Unresolved { reason } => ("unresolved", None, Some(reason.as_str())),
    };
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO edit (id, session_id, at, path, kind, status, confidence, module_level,
                                     unresolved_reason, prompt, rationale, changed_lines)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            origin.event_id,
            origin.session_id,
            origin.timestamp,
            indexed.path,
            kind_name(edit.kind),
            status,
            anchor.map(|a| confidence_name(a.confidence)),
            anchor.is_some_and(|a| a.module_level),
            reason,
            edit.context.prompt,
            edit.context.rationale,
            serde_json::to_string(indexed.changed_lines).unwrap_or_else(|_| "[]".to_owned()),
        ],
    )?;
    // Edición ya indexada antes: sus símbolos también lo están.
    if inserted == 0 {
        return Ok(());
    }
    let Some(anchor) = anchor else { return Ok(()) };
    let mut insert_symbol = tx.prepare_cached(
        "INSERT OR IGNORE INTO edit_symbol (edit_id, role, qualified_name, kind, start_line, end_line, body_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let roles = anchor.symbols.iter().map(|s| ("touched", s)).chain(anchor.removed.iter().map(|s| ("removed", s)));
    for (role, symbol) in roles {
        insert_symbol.execute(params![
            origin.event_id,
            role,
            symbol.qualified_name,
            symbol_kind_name(symbol.kind),
            i64::try_from(symbol.start_line).unwrap_or(i64::MAX),
            i64::try_from(symbol.end_line).unwrap_or(i64::MAX),
            symbol.body_hash
        ])?;
    }
    Ok(())
}

fn kind_name(kind: EditKind) -> &'static str {
    match kind {
        EditKind::Create => "create",
        EditKind::Rewrite => "rewrite",
        EditKind::Replace => "replace",
    }
}

fn confidence_name(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Exact => "exact",
        Confidence::Approximate => "approximate",
    }
}

fn symbol_kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Class => "class",
        SymbolKind::Interface => "interface",
        SymbolKind::TypeAlias => "type_alias",
        SymbolKind::Enum => "enum",
        SymbolKind::Struct => "struct",
        SymbolKind::Trait => "trait",
        SymbolKind::Impl => "impl",
        SymbolKind::Module => "module",
        SymbolKind::Variable => "variable",
        SymbolKind::Test => "test",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{AfterState, Anchor, EditOrigin, Symbol};

    use super::*;

    fn symbol(name: &str, hash: &str) -> Symbol {
        Symbol {
            qualified_name: name.into(),
            kind: SymbolKind::Method,
            start_line: 1,
            end_line: 3,
            body_hash: hash.into(),
            byte_range: 0..10,
            depth: 1,
        }
    }

    fn edit(id: &str, session: &str, at: &str, prompt: &str) -> FileEdit {
        FileEdit {
            origin: EditOrigin {
                agent: "claude-code",
                session_id: session.into(),
                event_id: id.into(),
                timestamp: Some(at.into()),
                cwd: Some(PathBuf::from("/repo")),
            },
            path: PathBuf::from("/repo/src/user.ts"),
            kind: EditKind::Replace,
            before: None,
            after: AfterState::Known(String::new()),
            user_modified: false,
            context: EditContext { prompt: Some(prompt.into()), rationale: Some(format!("porque {prompt}")) },
        }
    }

    fn anchored(touched: &[(&str, &str)], removed: &[(&str, &str)]) -> AnchorOutcome {
        AnchorOutcome::Anchored(Anchor {
            symbols: touched.iter().map(|(name, hash)| symbol(name, hash)).collect(),
            removed: removed.iter().map(|(name, hash)| symbol(name, hash)).collect(),
            module_level: false,
            confidence: Confidence::Exact,
        })
    }

    fn save(store: &mut Store, offset: u64, items: &[(&FileEdit, &AnchorOutcome)]) {
        let indexed: Vec<IndexedEdit> = items
            .iter()
            .map(|(edit, outcome)| IndexedEdit { edit, path: "/repo/src/user.ts", outcome, changed_lines: &[(2, 2)] })
            .collect();
        let cursor = TranscriptCursor { offset, context: EditContext::default() };
        store.save("/t/s1.jsonl", &cursor, &indexed).unwrap();
    }

    #[test]
    fn history_lists_edits_of_a_symbol_newest_first_with_context() {
        let mut store = Store::open_in_memory().unwrap();
        let first = edit("e1", "s1", "2026-10-01T10:00:00Z", "valida el email");
        let second = edit("e2", "s2", "2026-10-02T10:00:00Z", "normaliza el email");
        let other = edit("e3", "s2", "2026-10-03T10:00:00Z", "otra cosa");
        let (a, b, c) = (
            anchored(&[("UserService.register", "h1")], &[]),
            anchored(&[("UserService.register", "h2")], &[]),
            anchored(&[("UserService.remove", "h9")], &[]),
        );
        save(&mut store, 100, &[(&first, &a), (&second, &b), (&other, &c)]);

        let history = store.history("/repo/src/user.ts", "UserService.register").unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].edit_id, "e2");
        assert_eq!(history[0].body_hash, "h2");
        assert_eq!(history[0].context.prompt.as_deref(), Some("normaliza el email"));
        assert_eq!(history[1].context.rationale.as_deref(), Some("porque valida el email"));
        assert_eq!(store.history("/otro/fichero.ts", "UserService.register").unwrap(), Vec::new());
    }

    #[test]
    fn removed_symbols_keep_their_history() {
        let mut store = Store::open_in_memory().unwrap();
        let deletion = edit("e1", "s1", "2026-10-01T10:00:00Z", "borra remove");
        let outcome = anchored(&[("UserService", "h0")], &[("UserService.remove", "h1")]);
        save(&mut store, 10, &[(&deletion, &outcome)]);
        let history = store.history("/repo/src/user.ts", "UserService.remove").unwrap();
        assert_eq!(history[0].role, SymbolRole::Removed);
    }

    #[test]
    fn activity_counts_edits_and_sessions_per_symbol() {
        let mut store = Store::open_in_memory().unwrap();
        let edits = [
            edit("e1", "s1", "2026-10-01T10:00:00Z", "a"),
            edit("e2", "s1", "2026-10-02T10:00:00Z", "b"),
            edit("e3", "s2", "2026-10-05T10:00:00Z", "c"),
        ];
        let outcome = anchored(&[("UserService.register", "h")], &[]);
        save(&mut store, 10, &[(&edits[0], &outcome), (&edits[1], &outcome), (&edits[2], &outcome)]);
        let activity = store.activity("/repo/src/user.ts").unwrap();
        assert_eq!(
            activity["UserService.register"],
            SymbolActivity { edits: 3, sessions: 2, last_at: Some("2026-10-05T10:00:00Z".into()) }
        );
    }

    #[test]
    fn saving_twice_is_idempotent_and_moves_the_cursor() {
        let mut store = Store::open_in_memory().unwrap();
        let first = edit("e1", "s1", "2026-10-01T10:00:00Z", "a");
        let outcome = anchored(&[("UserService.register", "h")], &[]);
        save(&mut store, 10, &[(&first, &outcome)]);
        save(&mut store, 20, &[(&first, &outcome)]);
        assert_eq!(store.stats().unwrap(), StoreStats { transcripts: 1, sessions: 1, edits: 1, anchored_symbols: 1 });
        assert_eq!(store.cursor("/t/s1.jsonl").unwrap().unwrap().offset, 20);
        assert_eq!(store.cursor("/t/otra.jsonl").unwrap(), None);
    }

    #[test]
    fn unanchored_edits_are_stored_without_symbols() {
        let mut store = Store::open_in_memory().unwrap();
        let readme = edit("e1", "s1", "2026-10-01T10:00:00Z", "docs");
        save(&mut store, 10, &[(&readme, &AnchorOutcome::FileOnly)]);
        assert_eq!(store.stats().unwrap().edits, 1);
        assert_eq!(store.stats().unwrap().anchored_symbols, 0);
    }

    #[test]
    fn reopening_a_file_keeps_data_and_rejects_newer_schemas() {
        let dir = std::env::temp_dir().join(format!("huella-store-{}", std::process::id()));
        let path = dir.join("nested/huella.db");
        {
            let mut store = Store::open(&path).unwrap();
            let first = edit("e1", "s1", "2026-10-01T10:00:00Z", "a");
            save(&mut store, 10, &[(&first, &AnchorOutcome::NoChange)]);
        }
        assert_eq!(Store::open(&path).unwrap().stats().unwrap().edits, 1);
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(Store::open(&path), Err(StoreError::NewerSchema { found: 99, .. })));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
