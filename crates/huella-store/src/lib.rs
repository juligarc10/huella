//! Memoria persistente de Huella en SQLite.
//!
//! Un único fichero guarda, por transcripción, hasta dónde se ha leído; por
//! edición, su contexto y los símbolos que tocó o borró. Las ediciones se
//! identifican por el id del evento del agente, así que volver a indexar la
//! misma transcripción no duplica nada.

use std::collections::HashMap;
use std::path::Path;

use huella_core::{
    AnchorOutcome, Confidence, EditContext, EditKind, FileEdit, Fingerprint, SymbolKind, TranscriptCursor,
};
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
const MIGRATIONS: &[&str] = &[
    r"
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
",
    r"
    CREATE TABLE decision (
        id              INTEGER PRIMARY KEY,
        session_id      TEXT NOT NULL,
        project         TEXT,
        title           TEXT NOT NULL,
        decision        TEXT NOT NULL,
        rationale       TEXT NOT NULL,
        alternatives    TEXT NOT NULL,
        status          TEXT NOT NULL,
        superseded_by   TEXT,
        decided_at      TEXT,
        extractor       TEXT NOT NULL
    );
    CREATE INDEX decision_by_session ON decision(session_id);
    CREATE TABLE decision_entity (
        decision_id     INTEGER NOT NULL REFERENCES decision(id) ON DELETE CASCADE,
        entity          TEXT NOT NULL,
        PRIMARY KEY (decision_id, entity)
    );
    CREATE INDEX decision_entity_by_entity ON decision_entity(entity);
    CREATE TABLE decision_extraction (
        session_id      TEXT PRIMARY KEY,
        turns           INTEGER NOT NULL,
        extractor       TEXT NOT NULL
    );
",
    r"
    CREATE TABLE review_mark (
        session_id      TEXT NOT NULL,
        step_key        TEXT NOT NULL,
        content_hash    TEXT NOT NULL,
        reviewed_at     TEXT NOT NULL,
        PRIMARY KEY (session_id, step_key)
    );
",
    r"
    CREATE TABLE review_narration (
        session_id      TEXT NOT NULL,
        turn            INTEGER NOT NULL,
        input_hash      TEXT NOT NULL,
        model           TEXT NOT NULL,
        narration       TEXT NOT NULL,
        created_at      TEXT NOT NULL,
        PRIMARY KEY (session_id, turn)
    );
",
    r"
    CREATE TABLE review_active (
        id              INTEGER PRIMARY KEY CHECK (id = 1),
        session_id      TEXT NOT NULL,
        started_at      TEXT NOT NULL
    );
",
    r"
    CREATE TABLE review_workspace (
        session_id      TEXT PRIMARY KEY,
        repo            TEXT NOT NULL,
        path            TEXT NOT NULL,
        branch          TEXT NOT NULL,
        created_at      TEXT NOT NULL
    );
",
    // Huella del cuerpo independiente del nombre (renombrados y movimientos).
    // Se vuelven a leer las transcripciones desde el principio para rellenarla
    // en las ediciones ya indexadas que aún tienen transcripción.
    r"
    ALTER TABLE edit_symbol ADD COLUMN shape_hash TEXT;
    ALTER TABLE edit_symbol ADD COLUMN sketch BLOB;
    ALTER TABLE edit_symbol ADD COLUMN tokens INTEGER;
    ALTER TABLE edit_symbol ADD COLUMN own_name TEXT;
    CREATE INDEX edit_symbol_by_shape ON edit_symbol(shape_hash);
    CREATE INDEX edit_symbol_by_own_name ON edit_symbol(own_name);
    UPDATE transcript SET offset = 0, prompt = NULL, rationale = NULL;
",
    // Búsqueda de texto. Las decisiones se indexan con disparadores sobre su
    // tabla; las ediciones, desde Rust al guardarlas (su texto junta varias
    // tablas y parte los identificadores), y las ya guardadas al abrir.
    r"
    CREATE VIRTUAL TABLE edit_fts USING fts5(body, symbols, tokenize = 'unicode61 remove_diacritics 2');
    CREATE VIRTUAL TABLE decision_fts USING fts5(
        title, decision, rationale, alternatives,
        content = 'decision', content_rowid = 'id', tokenize = 'unicode61 remove_diacritics 2'
    );
    INSERT INTO decision_fts(decision_fts) VALUES ('rebuild');
    CREATE TRIGGER decision_fts_insert AFTER INSERT ON decision BEGIN
        INSERT INTO decision_fts(rowid, title, decision, rationale, alternatives)
        VALUES (new.id, new.title, new.decision, new.rationale, new.alternatives);
    END;
    CREATE TRIGGER decision_fts_delete AFTER DELETE ON decision BEGIN
        INSERT INTO decision_fts(decision_fts, rowid, title, decision, rationale, alternatives)
        VALUES ('delete', old.id, old.title, old.decision, old.rationale, old.alternatives);
    END;
    CREATE TRIGGER decision_fts_update AFTER UPDATE ON decision BEGIN
        INSERT INTO decision_fts(decision_fts, rowid, title, decision, rationale, alternatives)
        VALUES ('delete', old.id, old.title, old.decision, old.rationale, old.alternatives);
        INSERT INTO decision_fts(rowid, title, decision, rationale, alternatives)
        VALUES (new.id, new.title, new.decision, new.rationale, new.alternatives);
    END;
",
    // Ciclo de vida entre sesiones. Cada decisión tiene un id estable (sesión
    // y título) que sobrevive a volver a extraer; los reemplazos entre
    // sesiones pertenecen a la sesión nueva y las correcciones a mano se
    // guardan aparte, así que volver a extraer una sesión no pisa ninguno.
    r"
    ALTER TABLE decision ADD COLUMN uid TEXT;
    ALTER TABLE decision ADD COLUMN scope TEXT NOT NULL DEFAULT 'project';
    ALTER TABLE decision ADD COLUMN scope_path TEXT;
    CREATE INDEX decision_by_uid ON decision(uid);
    CREATE TABLE decision_supersession (
        old_uid         TEXT NOT NULL,
        new_session_id  TEXT NOT NULL,
        new_title       TEXT NOT NULL,
        PRIMARY KEY (old_uid, new_session_id)
    );
    CREATE TABLE decision_override (
        uid             TEXT PRIMARY KEY,
        status          TEXT NOT NULL,
        at              TEXT NOT NULL
    );
    CREATE VIEW decision_view AS
    SELECT d.*,
           COALESCE(o.status, CASE WHEN x.new_title IS NOT NULL THEN 'superseded' ELSE d.status END)
               AS effective_status,
           CASE WHEN COALESCE(o.status, 'superseded') = 'superseded'
                THEN COALESCE(d.superseded_by, x.new_title) END AS effective_superseded_by,
           o.status IS NOT NULL AS overridden
    FROM decision d
    LEFT JOIN decision_override o ON o.uid = d.uid
    LEFT JOIN (SELECT old_uid, MIN(new_title) AS new_title FROM decision_supersession GROUP BY old_uid) x
           ON x.old_uid = d.uid;
",
];

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
    /// Hash de su forma sin el nombre, si se conoce.
    pub shape_hash: Option<String>,
    pub context: EditContext,
}

/// Último estado conocido de un símbolo anclado en una ruta: lo que dejó la
/// última edición que lo tocó o lo borró.
#[derive(Debug, Clone, PartialEq)]
pub struct AnchorState {
    pub path: String,
    pub qualified_name: String,
    pub role: SymbolRole,
    pub edit_id: String,
    pub at: Option<String>,
    /// Vacía en ediciones indexadas antes de existir y sin transcripción.
    pub fingerprint: Fingerprint,
}

/// Resumen de la historia de un símbolo para mostrarlo sobre él.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolActivity {
    pub edits: usize,
    pub sessions: usize,
    pub last_at: Option<String>,
}

/// Decisión extraída de una sesión, lista para guardar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDecision {
    pub title: String,
    pub decision: String,
    pub rationale: String,
    pub alternatives: Vec<String>,
    pub entities: Vec<String>,
    /// `accepted`, `proposed`, `rejected` o `superseded`.
    pub status: String,
    /// Título de la decisión de la misma sesión que esta reemplaza, o vacío.
    pub supersedes: String,
    /// `user`, `project` o `path`.
    pub scope: String,
    /// Carpeta o fichero si `scope` es `path`.
    pub scope_path: Option<String>,
    /// Ids estables de decisiones de otras sesiones que esta reemplaza.
    pub replaces: Vec<String>,
}

/// Lote de decisiones de una sesión con el contexto de dónde salen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDecisions<'a> {
    pub session_id: &'a str,
    /// Proyecto (directorio de la sesión normalizado).
    pub project: Option<&'a str>,
    pub extractor: &'a str,
    /// Mensajes del diálogo cuando se extrajo: si crece, hay que volver a extraer.
    pub turns: usize,
    pub decided_at: Option<&'a str>,
    pub decisions: &'a [NewDecision],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDecision {
    pub id: i64,
    /// Id estable: sobrevive a volver a extraer la sesión.
    pub uid: String,
    pub scope: String,
    pub scope_path: Option<String>,
    /// El estado lo fijó la persona a mano.
    pub overridden: bool,
    pub session_id: String,
    pub project: Option<String>,
    pub title: String,
    pub decision: String,
    pub rationale: String,
    pub alternatives: Vec<String>,
    pub entities: Vec<String>,
    pub status: String,
    pub superseded_by: Option<String>,
    pub decided_at: Option<String>,
    pub extractor: String,
}

/// Worktree de git con el estado previo de la sesión en un commit y su
/// resultado como cambios sin commitear, para revisarla con las herramientas
/// de git del editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewWorkspace {
    pub session_id: String,
    pub repo: String,
    pub path: String,
    pub branch: String,
}

/// Narración de un turno generada por un modelo, con el hash de lo que se le
/// envió para saber si sigue vigente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNarration {
    pub input_hash: String,
    pub model: String,
    /// JSON tal como lo devolvió el modelo.
    pub narration: String,
}

/// Resumen de una sesión para elegir qué revisar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub first_at: Option<String>,
    pub last_at: Option<String>,
    pub edits: usize,
    pub files: usize,
}

/// Qué buscar en la memoria.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    /// Solo lo de este proyecto (ruta absoluta).
    pub project: Option<String>,
    pub limit: usize,
    /// Incluir decisiones reemplazadas, rechazadas o solo propuestas.
    pub include_inactive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SearchHit {
    /// Una petición de una sesión, con lo que tocó.
    Request {
        session_id: String,
        at: Option<String>,
        prompt: Option<String>,
        /// Fragmento con las palabras buscadas marcadas con `[` `]`.
        snippet: String,
        paths: Vec<String>,
        symbols: Vec<String>,
        edits: usize,
        score: f64,
    },
    Decision {
        decision: Box<StoredDecision>,
        snippet: String,
        score: f64,
    },
}

impl SearchHit {
    #[must_use]
    pub fn score(&self) -> f64 {
        match self {
            Self::Request { score, .. } | Self::Decision { score, .. } => *score,
        }
    }
}

/// Qué decisiones listar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionFilter {
    pub entity: Option<String>,
    /// Las del proyecto y las de la persona (alcance `user`), que valen en todos.
    pub project: Option<String>,
    /// Solo las de este alcance.
    pub scope: Option<String>,
    /// Incluir las reemplazadas, rechazadas y solo propuestas.
    pub include_inactive: bool,
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
        self.backfill_own_names()?;
        self.backfill_edit_search()?;
        self.backfill_decision_uids()
    }

    /// Id estable de las decisiones guardadas antes de existir.
    fn backfill_decision_uids(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut select = tx.prepare("SELECT id, session_id, title FROM decision WHERE uid IS NULL")?;
            let pending: Vec<(i64, String, String)> = select
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut update = tx.prepare("UPDATE decision SET uid = ?2 WHERE id = ?1")?;
            for (id, session_id, title) in &pending {
                update.execute(params![id, decision_uid(session_id, title)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Indexa para la búsqueda las ediciones guardadas antes de existir.
    fn backfill_edit_search(&mut self) -> Result<()> {
        let missing: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM edit WHERE rowid NOT IN (SELECT rowid FROM edit_fts)",
            [],
            |row| row.get(0),
        )?;
        if missing == 0 {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut select = tx.prepare(
                "SELECT e.rowid, e.path, e.prompt, e.rationale,
                        (SELECT group_concat(s.qualified_name, ' ') FROM edit_symbol s WHERE s.edit_id = e.id)
                 FROM edit e WHERE e.rowid NOT IN (SELECT rowid FROM edit_fts)",
            )?;
            let rows: Vec<PendingSearchRow> = select
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut insert = tx.prepare("INSERT INTO edit_fts (rowid, body, symbols) VALUES (?1, ?2, ?3)")?;
            for (rowid, path, prompt, rationale, symbols) in rows {
                let symbols: Vec<&str> = symbols.as_deref().unwrap_or_default().split(' ').collect();
                let (body, symbols) = search_body(&path, prompt.as_deref(), rationale.as_deref(), &symbols);
                insert.execute(params![rowid, body, symbols])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Nombre propio de los símbolos anclados antes de existir la columna,
    /// también los de transcripciones que ya no están.
    fn backfill_own_names(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut select = tx.prepare("SELECT rowid, qualified_name FROM edit_symbol WHERE own_name IS NULL")?;
            let pending: Vec<(i64, String)> =
                select.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            let mut update = tx.prepare("UPDATE edit_symbol SET own_name = ?2 WHERE rowid = ?1")?;
            for (rowid, qualified_name) in &pending {
                update.execute(params![rowid, own_name(qualified_name)])?;
            }
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
            "SELECT e.id, e.session_id, e.at, s.role, e.kind, e.confidence, s.body_hash, e.prompt, e.rationale,
                    s.shape_hash
             FROM edit_symbol s JOIN edit e ON e.id = s.edit_id
             WHERE e.path = ?1 AND s.qualified_name = ?2
             ORDER BY e.at DESC, e.id DESC",
        )?;
        let rows = statement.query_map(params![path, qualified_name], |row| {
            Ok(HistoryEntry {
                edit_id: row.get(0)?,
                session_id: row.get(1)?,
                at: row.get(2)?,
                role: role_from(&row.get::<_, String>(3)?),
                kind: row.get(4)?,
                confidence: row.get(5)?,
                body_hash: row.get(6)?,
                shape_hash: row.get(9)?,
                context: EditContext { prompt: row.get(7)?, rationale: row.get(8)? },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Último estado de cada símbolo anclado en un fichero.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn anchors_in(&self, path: &str) -> Result<Vec<AnchorState>> {
        latest_anchors(&self.conn, "e.path = ?1", &[&path])
    }

    /// Último estado de todos los símbolos anclados.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn all_anchors(&self) -> Result<Vec<AnchorState>> {
        latest_anchors(&self.conn, "1 = 1", &[])
    }

    /// Último estado de los símbolos que alguna vez tuvieron esta forma, en
    /// cualquier fichero.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn anchors_with_shape(&self, shape_hash: &str) -> Result<Vec<AnchorState>> {
        let mut found = latest_anchors(&self.conn, "s.shape_hash = ?1", &[&shape_hash])?;
        // Se filtró por la forma de alguna edición, no necesariamente la última.
        found.retain(|state| state.fingerprint.shape_hash == shape_hash);
        Ok(found)
    }

    /// Último estado de los símbolos cuyo nombre propio (el último tramo del
    /// cualificado) es `name`, en cualquier fichero.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn anchors_named(&self, name: &str) -> Result<Vec<AnchorState>> {
        latest_anchors(&self.conn, "s.own_name = ?1", &[&name])
    }

    /// Último estado de los símbolos de ficheros con este nombre (`user.ts`)
    /// en cualquier carpeta.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn anchors_in_files_named(&self, file_name: &str) -> Result<Vec<AnchorState>> {
        latest_anchors(&self.conn, "(e.path = ?1 OR e.path LIKE '%/' || ?1)", &[&file_name])
    }

    /// Símbolos que borró una edición, con su huella de antes de borrarlos.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn removed_by(&self, edit_id: &str) -> Result<Vec<AnchorState>> {
        latest_anchors(&self.conn, "s.edit_id = ?1 AND s.role = 'removed'", &[&edit_id])
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

    /// Pasos revisados de una sesión: clave del paso → hash del contenido
    /// que se revisó.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn review_marks(&self, session_id: &str) -> Result<HashMap<String, String>> {
        let mut statement =
            self.conn.prepare_cached("SELECT step_key, content_hash FROM review_mark WHERE session_id = ?1")?;
        let rows = statement.query_map([session_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Marca (o desmarca) un paso como revisado con el contenido que se vio.
    ///
    /// # Errors
    /// Si falla la escritura.
    pub fn set_review_mark(&self, session_id: &str, step_key: &str, content_hash: &str, reviewed: bool) -> Result<()> {
        if reviewed {
            self.conn.execute(
                "INSERT INTO review_mark (session_id, step_key, content_hash, reviewed_at)
                 VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(session_id, step_key) DO UPDATE SET content_hash = excluded.content_hash,
                                                                 reviewed_at = excluded.reviewed_at",
                params![session_id, step_key, content_hash],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM review_mark WHERE session_id = ?1 AND step_key = ?2",
                params![session_id, step_key],
            )?;
        }
        Ok(())
    }

    /// Worktree de git preparado para revisar una sesión en el editor.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn review_workspace(&self, session_id: &str) -> Result<Option<ReviewWorkspace>> {
        let found = self
            .conn
            .query_row(
                "SELECT session_id, repo, path, branch FROM review_workspace WHERE session_id = ?1",
                [session_id],
                |row| {
                    Ok(ReviewWorkspace {
                        session_id: row.get(0)?,
                        repo: row.get(1)?,
                        path: row.get(2)?,
                        branch: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(found)
    }

    /// # Errors
    /// Si falla la escritura.
    pub fn save_review_workspace(&self, workspace: &ReviewWorkspace) -> Result<()> {
        self.conn.execute(
            "INSERT INTO review_workspace (session_id, repo, path, branch, created_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(session_id) DO UPDATE SET repo = excluded.repo, path = excluded.path, branch = excluded.branch",
            params![workspace.session_id, workspace.repo, workspace.path, workspace.branch],
        )?;
        Ok(())
    }

    /// # Errors
    /// Si falla la escritura.
    pub fn delete_review_workspace(&self, session_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM review_workspace WHERE session_id = ?1", [session_id])?;
        Ok(())
    }

    /// Sesión que se está revisando: el editor muestra sus pasos pendientes.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn active_review(&self) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT session_id FROM review_active WHERE id = 1", [], |row| row.get(0)).optional()?)
    }

    /// Fija (o quita, con `None`) la sesión que se está revisando.
    ///
    /// # Errors
    /// Si falla la escritura.
    pub fn set_active_review(&self, session_id: Option<&str>) -> Result<()> {
        match session_id {
            Some(session) => self.conn.execute(
                "INSERT INTO review_active (id, session_id, started_at)
                 VALUES (1, ?1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(id) DO UPDATE SET session_id = excluded.session_id, started_at = excluded.started_at",
                [session],
            )?,
            None => self.conn.execute("DELETE FROM review_active", [])?,
        };
        Ok(())
    }

    /// Narraciones guardadas de una sesión, por turno.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn narrations(&self, session_id: &str) -> Result<HashMap<usize, StoredNarration>> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT turn, input_hash, model, narration FROM review_narration WHERE session_id = ?1")?;
        let rows = statement.query_map([session_id], |row| {
            let turn: i64 = row.get(0)?;
            Ok((
                usize::try_from(turn).unwrap_or_default(),
                StoredNarration { input_hash: row.get(1)?, model: row.get(2)?, narration: row.get(3)? },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Guarda (o sustituye) la narración de un turno.
    ///
    /// # Errors
    /// Si falla la escritura.
    pub fn save_narration(&self, session_id: &str, turn: usize, stored: &StoredNarration) -> Result<()> {
        self.conn.execute(
            "INSERT INTO review_narration (session_id, turn, input_hash, model, narration, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(session_id, turn) DO UPDATE SET input_hash = excluded.input_hash, model = excluded.model,
                 narration = excluded.narration, created_at = excluded.created_at",
            params![
                session_id,
                i64::try_from(turn).unwrap_or(i64::MAX),
                stored.input_hash,
                stored.model,
                stored.narration
            ],
        )?;
        Ok(())
    }

    /// Sesiones con ediciones, de la más reciente a la más antigua.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT s.id, s.first_at, s.last_at, COUNT(e.id), COUNT(DISTINCT e.path)
             FROM session s JOIN edit e ON e.session_id = s.id
             GROUP BY s.id ORDER BY s.last_at DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            let count = |index| row.get::<_, i64>(index).map(|n| usize::try_from(n).unwrap_or_default());
            Ok(SessionSummary {
                id: row.get(0)?,
                first_at: row.get(1)?,
                last_at: row.get(2)?,
                edits: count(3)?,
                files: count(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Ficheros (rutas normalizadas) que editó una sesión.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn session_paths(&self, session_id: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare_cached("SELECT DISTINCT path FROM edit WHERE session_id = ?1")?;
        let rows = statement.query_map([session_id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Reasigna el proyecto de las decisiones de una sesión sin volver a extraerlas.
    ///
    /// # Errors
    /// Si falla la escritura.
    pub fn set_decisions_project(&self, session_id: &str, project: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE decision SET project = ?2 WHERE session_id = ?1 AND project IS NOT ?2",
            params![session_id, project],
        )?;
        Ok(())
    }

    /// Mensajes de diálogo que tenía una sesión cuando se extrajeron sus
    /// decisiones; `None` si nunca se extrajeron.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn extracted_turns(&self, session_id: &str) -> Result<Option<usize>> {
        let turns: Option<i64> = self
            .conn
            .query_row("SELECT turns FROM decision_extraction WHERE session_id = ?1", [session_id], |row| row.get(0))
            .optional()?;
        Ok(turns.map(|n| usize::try_from(n).unwrap_or_default()))
    }

    /// Sustituye las decisiones de una sesión por las de una extracción nueva y
    /// marca como reemplazadas las que una decisión posterior dice reemplazar.
    ///
    /// # Errors
    /// Si falla alguna escritura; en ese caso no cambia nada.
    pub fn replace_decisions(&mut self, batch: &SessionDecisions) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM decision WHERE session_id = ?1", [batch.session_id])?;
        tx.execute("DELETE FROM decision_supersession WHERE new_session_id = ?1", [batch.session_id])?;
        for decision in batch.decisions {
            tx.execute(
                "INSERT INTO decision (session_id, project, title, decision, rationale, alternatives, status,
                                       decided_at, extractor, uid, scope, scope_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    batch.session_id,
                    batch.project,
                    decision.title,
                    decision.decision,
                    decision.rationale,
                    serde_json::to_string(&decision.alternatives).unwrap_or_else(|_| "[]".to_owned()),
                    decision.status,
                    batch.decided_at,
                    batch.extractor,
                    decision_uid(batch.session_id, &decision.title),
                    decision.scope,
                    decision.scope_path
                ],
            )?;
            // Solo una decisión tomada reemplaza a otra; una propuesta, no.
            if decision.status == "accepted" {
                for old_uid in &decision.replaces {
                    tx.execute(
                        "INSERT OR IGNORE INTO decision_supersession (old_uid, new_session_id, new_title)
                         VALUES (?1, ?2, ?3)",
                        params![old_uid, batch.session_id, decision.title],
                    )?;
                }
            }
            let id = tx.last_insert_rowid();
            for entity in &decision.entities {
                tx.execute(
                    "INSERT OR IGNORE INTO decision_entity (decision_id, entity) VALUES (?1, ?2)",
                    params![id, entity],
                )?;
            }
        }
        for decision in batch.decisions.iter().filter(|d| !d.supersedes.is_empty()) {
            tx.execute(
                "UPDATE decision SET status = 'superseded', superseded_by = ?1
                 WHERE session_id = ?2 AND title = ?3 COLLATE NOCASE AND title <> ?1",
                params![decision.title, batch.session_id, decision.supersedes],
            )?;
        }
        tx.execute(
            "INSERT INTO decision_extraction (session_id, turns, extractor) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET turns = excluded.turns, extractor = excluded.extractor",
            params![batch.session_id, i64::try_from(batch.turns).unwrap_or(i64::MAX), batch.extractor],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Busca en las peticiones, explicaciones, rutas y símbolos de las
    /// ediciones y en las decisiones. Las ediciones de una misma petición se
    /// agrupan; los dos tipos se mezclan por su puesto en su propia lista.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>> {
        let Some(fts) = fts_query(&query.text) else { return Ok(Vec::new()) };
        let project_prefix = query.project.as_ref().map(|project| format!("{}/", project.trim_end_matches('/')));
        let requests = self.search_requests(&fts, project_prefix.as_deref())?;
        let decisions = self.search_decisions(&fts, query)?;
        let keys = [
            requests.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(),
            decisions.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(),
        ];
        let mut by_key: HashMap<String, SearchHit> = requests.into_iter().chain(decisions).collect();
        let mut hits = Vec::new();
        for (key, score) in fuse_ranks(&keys).into_iter().take(query.limit.max(1)) {
            if let Some(mut hit) = by_key.remove(&key) {
                match &mut hit {
                    SearchHit::Request { score: s, .. } | SearchHit::Decision { score: s, .. } => *s = score,
                }
                hits.push(hit);
            }
        }
        Ok(hits)
    }

    fn search_requests(&self, fts: &str, project_prefix: Option<&str>) -> Result<Vec<(String, SearchHit)>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT e.session_id, e.at, e.prompt, e.path, snippet(edit_fts, 0, '[', ']', '…', 12),
                    (SELECT group_concat(s.qualified_name, char(31)) FROM edit_symbol s
                     WHERE s.edit_id = e.id AND s.role = 'touched')
             FROM edit_fts JOIN edit e ON e.rowid = edit_fts.rowid
             WHERE edit_fts MATCH ?1 AND (?2 IS NULL OR e.path LIKE ?2 || '%')
             ORDER BY bm25(edit_fts) LIMIT 400",
        )?;
        let rows = statement.query_map(params![fts, project_prefix], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        // Agrupa por petición conservando el orden de la mejor edición de cada una.
        let mut order: Vec<String> = Vec::new();
        let mut grouped: HashMap<String, SearchHit> = HashMap::new();
        for row in rows {
            let (session_id, at, prompt, path, snippet, symbols) = row?;
            let key = format!("request\u{1f}{session_id}\u{1f}{}", prompt.as_deref().unwrap_or(&path));
            let hit = grouped.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                SearchHit::Request {
                    session_id,
                    at: at.clone(),
                    prompt,
                    snippet,
                    paths: Vec::new(),
                    symbols: Vec::new(),
                    edits: 0,
                    score: 0.0,
                }
            });
            if let SearchHit::Request { at: latest, paths, symbols: all, edits, .. } = hit {
                *edits += 1;
                if at > *latest {
                    latest.clone_from(&at);
                }
                if !paths.contains(&path) {
                    paths.push(path);
                }
                for symbol in symbols.as_deref().unwrap_or_default().split('\u{1f}').filter(|s| !s.is_empty()) {
                    if !all.iter().any(|known| known == symbol) {
                        all.push(symbol.to_owned());
                    }
                }
            }
        }
        Ok(order.into_iter().filter_map(|key| grouped.remove(&key).map(|hit| (key, hit))).collect())
    }

    fn search_decisions(&self, fts: &str, query: &SearchQuery) -> Result<Vec<(String, SearchHit)>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT d.id, snippet(decision_fts, -1, '[', ']', '…', 12)
             FROM decision_fts JOIN decision_view d ON d.id = decision_fts.rowid
             WHERE decision_fts MATCH ?1 AND (?2 IS NULL OR d.project = ?2 OR d.scope = 'user')
               AND (?3 OR d.effective_status = 'accepted')
             ORDER BY bm25(decision_fts) LIMIT 100",
        )?;
        let ids: Vec<(i64, String)> = statement
            .query_map(params![fts, query.project, query.include_inactive], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut hits = Vec::new();
        for (id, snippet) in ids {
            if let Some(decision) = self.decision(id)? {
                hits.push((
                    format!("decision\u{1f}{id}"),
                    SearchHit::Decision { decision: Box::new(decision), snippet, score: 0.0 },
                ));
            }
        }
        Ok(hits)
    }

    /// Una decisión por su id.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn decision(&self, id: i64) -> Result<Option<StoredDecision>> {
        let filter = DecisionFilter { include_inactive: true, ..DecisionFilter::default() };
        Ok(self.decisions_where(&filter, Some(id), None)?.into_iter().next())
    }

    /// Decisiones de la más reciente a la más antigua. Por defecto solo las
    /// vigentes (aceptadas), que son las que conviene recordar.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn decisions(&self, filter: &DecisionFilter) -> Result<Vec<StoredDecision>> {
        self.decisions_where(filter, None, None)
    }

    /// Decisiones (también inactivas) cuyo id estable empieza por `prefix`.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn decisions_by_uid(&self, prefix: &str) -> Result<Vec<StoredDecision>> {
        let filter = DecisionFilter { include_inactive: true, ..DecisionFilter::default() };
        self.decisions_where(&filter, None, Some(prefix))
    }

    /// Fija a mano el estado de una decisión (`None` lo devuelve al extraído).
    /// Sobrevive a volver a extraer su sesión.
    ///
    /// # Errors
    /// Si falla la escritura.
    pub fn set_decision_status(&self, uid: &str, status: Option<&str>) -> Result<()> {
        match status {
            Some(status) => self.conn.execute(
                "INSERT INTO decision_override (uid, status, at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(uid) DO UPDATE SET status = excluded.status, at = excluded.at",
                params![uid, status],
            )?,
            None => self.conn.execute("DELETE FROM decision_override WHERE uid = ?1", [uid])?,
        };
        Ok(())
    }

    fn decisions_where(
        &self,
        filter: &DecisionFilter,
        id: Option<i64>,
        uid: Option<&str>,
    ) -> Result<Vec<StoredDecision>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT d.id, d.session_id, d.project, d.title, d.decision, d.rationale, d.alternatives,
                    d.effective_status, d.effective_superseded_by, d.decided_at, d.extractor,
                    (SELECT group_concat(entity, char(31)) FROM decision_entity WHERE decision_id = d.id),
                    d.uid, d.scope, d.scope_path, d.overridden
             FROM decision_view d
             WHERE (?1 IS NULL OR EXISTS (SELECT 1 FROM decision_entity e WHERE e.decision_id = d.id AND e.entity = ?1))
               AND (?2 IS NULL OR d.project = ?2 OR d.scope = 'user')
               AND (?3 OR d.effective_status = 'accepted')
               AND (?4 IS NULL OR d.id = ?4)
               AND (?5 IS NULL OR d.scope = ?5)
               AND (?6 IS NULL OR d.uid LIKE ?6 || '%')
             ORDER BY d.decided_at DESC, d.id",
        )?;
        let entity = filter.entity.as_ref().map(|entity| entity.trim().to_lowercase());
        let rows = statement.query_map(
            params![entity, filter.project, filter.include_inactive, id, filter.scope, uid],
            |row| {
                let alternatives: String = row.get(6)?;
                let entities: Option<String> = row.get(11)?;
                let mut entities: Vec<String> =
                    entities.map(|list| list.split('\u{1f}').map(str::to_owned).collect()).unwrap_or_default();
                entities.sort();
                Ok(StoredDecision {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    project: row.get(2)?,
                    title: row.get(3)?,
                    decision: row.get(4)?,
                    rationale: row.get(5)?,
                    alternatives: serde_json::from_str(&alternatives).unwrap_or_default(),
                    entities,
                    status: row.get(7)?,
                    superseded_by: row.get(8)?,
                    decided_at: row.get(9)?,
                    extractor: row.get(10)?,
                    uid: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
                    scope: row.get(13)?,
                    scope_path: row.get(14)?,
                    overridden: row.get(15)?,
                })
            },
        )?;
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
    if inserted == 1 {
        let symbols: Vec<&str> = anchor
            .map(|a| a.symbols.iter().chain(&a.removed).map(|s| s.qualified_name.as_str()).collect())
            .unwrap_or_default();
        let (body, symbols) =
            search_body(indexed.path, edit.context.prompt.as_deref(), edit.context.rationale.as_deref(), &symbols);
        tx.execute(
            "INSERT INTO edit_fts (rowid, body, symbols) VALUES (last_insert_rowid(), ?1, ?2)",
            [body, symbols],
        )?;
    }
    let Some(anchor) = anchor else { return Ok(()) };
    let roles = anchor.symbols.iter().map(|s| ("touched", s)).chain(anchor.removed.iter().map(|s| ("removed", s)));
    // Edición ya indexada antes: solo se completa la huella si le falta, sin
    // añadir símbolos que el anclaje actual nombre de otra forma.
    if inserted == 0 {
        let mut backfill = tx.prepare_cached(
            "UPDATE edit_symbol SET shape_hash = ?4, sketch = ?5, tokens = ?6
             WHERE edit_id = ?1 AND role = ?2 AND qualified_name = ?3 AND shape_hash IS NULL",
        )?;
        for (role, symbol) in roles {
            let fingerprint = &symbol.fingerprint;
            backfill.execute(params![
                origin.event_id,
                role,
                symbol.qualified_name,
                fingerprint.shape_hash,
                fingerprint.sketch_bytes(),
                i64::try_from(fingerprint.tokens).unwrap_or(i64::MAX)
            ])?;
        }
        return Ok(());
    }
    let mut insert_symbol = tx.prepare_cached(
        "INSERT OR IGNORE INTO edit_symbol (edit_id, role, qualified_name, kind, start_line, end_line, body_hash,
                                            shape_hash, sketch, tokens, own_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for (role, symbol) in roles {
        let fingerprint = &symbol.fingerprint;
        insert_symbol.execute(params![
            origin.event_id,
            role,
            symbol.qualified_name,
            symbol_kind_name(symbol.kind),
            i64::try_from(symbol.start_line).unwrap_or(i64::MAX),
            i64::try_from(symbol.end_line).unwrap_or(i64::MAX),
            symbol.body_hash,
            fingerprint.shape_hash,
            fingerprint.sketch_bytes(),
            i64::try_from(fingerprint.tokens).unwrap_or(i64::MAX),
            own_name(&symbol.qualified_name)
        ])?;
    }
    Ok(())
}

/// Edición aún sin indexar: rowid, ruta, petición, explicación y símbolos.
type PendingSearchRow = (i64, String, Option<String>, Option<String>, Option<String>);

/// Texto por el que se encuentra una edición: la prosa (petición,
/// explicación y ruta), de la que salen los fragmentos, y los símbolos,
/// también partidos en palabras (`hashPassword` → `hash Password`) para que
/// «password» los encuentre.
fn search_body(path: &str, prompt: Option<&str>, rationale: Option<&str>, symbols: &[&str]) -> (String, String) {
    let body = [prompt, rationale, Some(path)].into_iter().flatten().collect::<Vec<_>>().join("\n");
    let symbols = symbols
        .iter()
        .filter(|s| !s.is_empty())
        .map(|symbol| format!("{symbol} {}", split_identifier(symbol)))
        .collect::<Vec<_>>()
        .join("\n");
    (body, symbols)
}

/// `UserService.hashPassword` → `User Service hash Password`.
fn split_identifier(identifier: &str) -> String {
    let mut words = String::new();
    let mut previous: Option<char> = None;
    for c in identifier.chars() {
        let boundary = previous.is_some_and(|p| (p.is_lowercase() || p.is_ascii_digit()) && c.is_uppercase());
        if boundary || (!c.is_alphanumeric() && !words.ends_with(' ')) {
            words.push(' ');
        }
        if c.is_alphanumeric() {
            words.push(c);
        }
        previous = Some(c);
    }
    words.trim().to_owned()
}

/// Consulta FTS5 con las palabras de la persona: todas deben aparecer, la
/// última también como prefijo (se busca mientras se escribe).
fn fts_query(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let (last, rest) = words.split_last()?;
    let mut query: Vec<String> = rest.iter().map(|word| format!("\"{word}\"")).collect();
    query.push(format!("\"{last}\"*"));
    Some(query.join(" "))
}

/// Une listas ordenadas por relevancia sumando 1/(k + puesto) de cada una
/// (reciprocal rank fusion): no hace falta que sus puntuaciones sean comparables.
fn fuse_ranks(lists: &[Vec<String>]) -> Vec<(String, f64)> {
    const K: f64 = 60.0;
    let mut scores: HashMap<String, f64> = HashMap::new();
    for list in lists {
        for (rank, key) in (1u32..).zip(list) {
            *scores.entry(key.clone()).or_default() += 1.0 / (K + f64::from(rank));
        }
    }
    let mut fused: Vec<(String, f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    fused
}

/// Id estable de una decisión: su sesión y su título sin mayúsculas.
#[must_use]
pub fn decision_uid(session_id: &str, title: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(session_id.as_bytes());
    hasher.update(&[0x1f]);
    hasher.update(title.trim().to_lowercase().as_bytes());
    hasher.finalize().to_hex()[..12].to_owned()
}

/// Último tramo del nombre cualificado: `UserService.register` → `register`.
fn own_name(qualified_name: &str) -> &str {
    qualified_name.rsplit(['.', ':']).next().unwrap_or(qualified_name)
}

/// Última aparición de cada símbolo entre las filas de `edit_symbol` que
/// cumplen `filter` (sobre `s` y `e`), con `params`.
fn latest_anchors(conn: &Connection, filter: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<AnchorState>> {
    let sql = format!(
        "SELECT e.path, s.qualified_name, s.role, e.id, e.at, s.shape_hash, s.sketch, s.tokens
         FROM edit_symbol s JOIN edit e ON e.id = s.edit_id
         WHERE {filter}
         ORDER BY e.at DESC, e.id DESC"
    );
    let mut statement = conn.prepare_cached(&sql)?;
    let rows = statement.query_map(params, |row| {
        Ok(AnchorState {
            path: row.get(0)?,
            qualified_name: row.get(1)?,
            role: role_from(&row.get::<_, String>(2)?),
            edit_id: row.get(3)?,
            at: row.get(4)?,
            fingerprint: fingerprint_from(row.get(5)?, row.get::<_, Option<Vec<u8>>>(6)?.as_deref(), row.get(7)?),
        })
    })?;
    let mut seen = std::collections::HashSet::new();
    let mut latest = Vec::new();
    for state in rows {
        let state = state?;
        if seen.insert((state.path.clone(), state.qualified_name.clone())) {
            latest.push(state);
        }
    }
    Ok(latest)
}

fn role_from(role: &str) -> SymbolRole {
    if role == "removed" { SymbolRole::Removed } else { SymbolRole::Touched }
}

fn fingerprint_from(shape_hash: Option<String>, sketch: Option<&[u8]>, tokens: Option<i64>) -> Fingerprint {
    Fingerprint {
        shape_hash: shape_hash.unwrap_or_default(),
        sketch: sketch.map(Fingerprint::sketch_from_bytes).unwrap_or_default(),
        tokens: tokens.and_then(|n| usize::try_from(n).ok()).unwrap_or_default(),
    }
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
            fingerprint: huella_core::Fingerprint::default(),
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

    fn new_decision(title: &str, entities: &[&str], status: &str, supersedes: &str) -> NewDecision {
        NewDecision {
            title: title.into(),
            decision: format!("se decide {title}"),
            rationale: "porque sí".into(),
            alternatives: vec!["otra: peor".into()],
            entities: entities.iter().map(|e| (*e).to_owned()).collect(),
            status: status.into(),
            supersedes: supersedes.into(),
            scope: "project".into(),
            scope_path: None,
            replaces: Vec::new(),
        }
    }

    fn batch<'a>(session: &'a str, turns: usize, decisions: &'a [NewDecision]) -> SessionDecisions<'a> {
        SessionDecisions {
            session_id: session,
            project: Some("/repo"),
            extractor: "test:modelo",
            turns,
            decided_at: Some("2026-10-25T10:00:00Z"),
            decisions,
        }
    }

    fn search(store: &Store, text: &str) -> Vec<SearchHit> {
        store.search(&SearchQuery { text: text.into(), limit: 10, ..SearchQuery::default() }).unwrap()
    }

    #[test]
    fn search_finds_requests_by_words_accents_and_split_identifiers() {
        let mut store = Store::open_in_memory().unwrap();
        let first = edit("e1", "s1", "2026-10-01T10:00:00Z", "añade la migración de usuarios");
        let second = edit("e2", "s1", "2026-10-01T10:01:00Z", "añade la migración de usuarios");
        let other = edit("e3", "s2", "2026-10-02T10:00:00Z", "cambia el color del botón");
        let hashed = anchored(&[("UserService.hashPassword", "h1")], &[]);
        save(&mut store, 10, &[(&first, &hashed), (&second, &hashed), (&other, &anchored(&[("Button", "h2")], &[]))]);

        let hits = search(&store, "migracion");
        assert_eq!(hits.len(), 1, "las dos ediciones de la petición se agrupan: {hits:?}");
        let SearchHit::Request { session_id, edits, symbols, snippet, .. } = &hits[0] else { panic!("{hits:?}") };
        assert_eq!((session_id.as_str(), *edits), ("s1", 2));
        assert_eq!(symbols, &vec!["UserService.hashPassword".to_owned()]);
        assert!(snippet.contains("[migración]"), "{snippet}");

        assert_eq!(search(&store, "password").len(), 1, "por las palabras del identificador");
        assert_eq!(search(&store, "bot").len(), 1, "la última palabra vale como prefijo");
        assert_eq!(search(&store, "migración botón"), Vec::new(), "todas las palabras deben aparecer");
        assert_eq!(search(&store, "  "), Vec::new());
    }

    #[test]
    fn search_finds_active_decisions_and_follows_their_changes() {
        let mut store = Store::open_in_memory().unwrap();
        let decisions = [
            new_decision("SQLite en local", &["sqlite"], "accepted", ""),
            new_decision("Postgres en local", &["postgres"], "rejected", ""),
        ];
        store.replace_decisions(&batch("s1", 10, &decisions)).unwrap();
        let titles = |hits: Vec<SearchHit>| -> Vec<String> {
            hits.into_iter()
                .filter_map(|hit| match hit {
                    SearchHit::Decision { decision, .. } => Some(decision.title),
                    SearchHit::Request { .. } => None,
                })
                .collect()
        };
        assert_eq!(titles(search(&store, "local")), vec!["SQLite en local"]);
        let all = store
            .search(&SearchQuery { text: "local".into(), limit: 10, include_inactive: true, ..SearchQuery::default() })
            .unwrap();
        assert_eq!(titles(all).len(), 2);

        // Volver a extraer la sesión sustituye sus decisiones también en el índice.
        store.replace_decisions(&batch("s1", 12, &[new_decision("DuckDB en local", &[], "accepted", "")])).unwrap();
        assert_eq!(titles(search(&store, "local")), vec!["DuckDB en local"]);
        assert_eq!(search(&store, "sqlite"), Vec::new());
    }

    #[test]
    fn edits_saved_before_the_search_index_existed_are_indexed_on_open() {
        let dir = std::env::temp_dir().join(format!("huella-fts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("huella.db");
        let _ = std::fs::remove_file(&path);
        {
            let mut store = Store::open(&path).unwrap();
            let first = edit("e1", "s1", "2026-10-01T10:00:00Z", "valida el email");
            save(&mut store, 10, &[(&first, &anchored(&[("validate", "h1")], &[]))]);
            store.conn.execute("DELETE FROM edit_fts", []).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(search(&store, "email").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn identifiers_split_into_words() {
        assert_eq!(split_identifier("UserService.hashPassword"), "User Service hash Password");
        assert_eq!(split_identifier("parse_v2Config"), "parse v2 Config");
    }

    fn status_of(store: &Store, title: &str) -> (String, Option<String>) {
        let all = store.decisions(&DecisionFilter { include_inactive: true, ..DecisionFilter::default() }).unwrap();
        let found = all.into_iter().find(|d| d.title == title).unwrap();
        (found.status, found.superseded_by)
    }

    #[test]
    fn a_later_session_supersedes_earlier_decisions_and_survives_reextraction() {
        let mut store = Store::open_in_memory().unwrap();
        let sqlite = [new_decision("SQLite en local", &["sqlite"], "accepted", "")];
        store.replace_decisions(&batch("s1", 10, &sqlite)).unwrap();
        let old_uid = decision_uid("s1", "SQLite en local");

        let mut duckdb = new_decision("DuckDB en local", &["duckdb"], "accepted", "");
        duckdb.replaces = vec![old_uid.clone()];
        store.replace_decisions(&batch("s2", 10, std::slice::from_ref(&duckdb))).unwrap();
        assert_eq!(status_of(&store, "SQLite en local"), ("superseded".into(), Some("DuckDB en local".into())));

        // Volver a extraer la sesión antigua no la resucita: el reemplazo es de la nueva.
        store.replace_decisions(&batch("s1", 12, &sqlite)).unwrap();
        assert_eq!(status_of(&store, "SQLite en local").0, "superseded");

        // Si la nueva deja de reemplazarla, vuelve a estar vigente.
        duckdb.replaces.clear();
        store.replace_decisions(&batch("s2", 12, &[duckdb])).unwrap();
        assert_eq!(status_of(&store, "SQLite en local"), ("accepted".into(), None));
    }

    #[test]
    fn only_a_taken_decision_supersedes_another_session() {
        let mut store = Store::open_in_memory().unwrap();
        store.replace_decisions(&batch("s1", 10, &[new_decision("SQLite en local", &[], "accepted", "")])).unwrap();
        let mut proposal = new_decision("Quizá DuckDB", &[], "proposed", "");
        proposal.replaces = vec![decision_uid("s1", "sqlite en local")];
        store.replace_decisions(&batch("s2", 10, &[proposal])).unwrap();
        assert_eq!(status_of(&store, "SQLite en local").0, "accepted");
    }

    #[test]
    fn a_status_set_by_hand_wins_and_survives_reextraction() {
        let mut store = Store::open_in_memory().unwrap();
        let decisions = [new_decision("SQLite en local", &[], "accepted", "")];
        store.replace_decisions(&batch("s1", 10, &decisions)).unwrap();
        let uid = &store.decisions_by_uid(&decision_uid("s1", "SQLite en local")[..6]).unwrap()[0].uid;
        store.set_decision_status(uid, Some("rejected")).unwrap();
        store.replace_decisions(&batch("s1", 12, &decisions)).unwrap();
        let found = &store.decisions_by_uid(uid).unwrap()[0];
        assert_eq!((found.status.as_str(), found.overridden), ("rejected", true));
        assert_eq!(store.decisions(&DecisionFilter::default()).unwrap(), Vec::new());
        store.set_decision_status(uid, None).unwrap();
        assert_eq!(status_of(&store, "SQLite en local").0, "accepted");
    }

    #[test]
    fn decisions_of_the_person_apply_to_every_project() {
        let mut store = Store::open_in_memory().unwrap();
        let mut spanish = new_decision("Commits en español", &[], "accepted", "");
        spanish.scope = "user".into();
        store
            .replace_decisions(&batch("s1", 10, &[spanish, new_decision("SQLite en local", &[], "accepted", "")]))
            .unwrap();
        let elsewhere = DecisionFilter { project: Some("/otro".into()), ..DecisionFilter::default() };
        let titles: Vec<String> = store.decisions(&elsewhere).unwrap().into_iter().map(|d| d.title).collect();
        assert_eq!(titles, vec!["Commits en español"]);
        let only_user = DecisionFilter { scope: Some("user".into()), ..DecisionFilter::default() };
        assert_eq!(store.decisions(&only_user).unwrap().len(), 1);
    }

    #[test]
    fn later_decisions_supersede_earlier_ones_in_the_same_session() {
        let mut store = Store::open_in_memory().unwrap();
        let decisions = [
            new_decision("TypeScript para el MVP", &["typescript"], "accepted", ""),
            new_decision("Rust para todo el producto", &["rust", "python"], "accepted", "typescript para el mvp"),
        ];
        store.replace_decisions(&batch("s1", 10, &decisions)).unwrap();

        let active = store.decisions(&DecisionFilter::default()).unwrap();
        assert_eq!(active.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(), vec!["Rust para todo el producto"]);
        assert_eq!(active[0].entities, vec!["python", "rust"]);
        assert_eq!(active[0].alternatives, vec!["otra: peor"]);

        let all = store.decisions(&DecisionFilter { include_inactive: true, ..DecisionFilter::default() }).unwrap();
        let replaced = all.iter().find(|d| d.title == "TypeScript para el MVP").unwrap();
        assert_eq!(
            (replaced.status.as_str(), replaced.superseded_by.as_deref()),
            ("superseded", Some("Rust para todo el producto"))
        );
    }

    #[test]
    fn decisions_filter_by_entity_and_project() {
        let mut store = Store::open_in_memory().unwrap();
        let decisions = [
            new_decision("SQLite local", &["sqlite"], "accepted", ""),
            new_decision("LSP síncrono", &["lsp"], "accepted", ""),
        ];
        store.replace_decisions(&batch("s1", 4, &decisions)).unwrap();
        let by_entity = DecisionFilter { entity: Some(" SQLite ".into()), ..DecisionFilter::default() };
        assert_eq!(store.decisions(&by_entity).unwrap().len(), 1);
        let other_project = DecisionFilter { project: Some("/otro".into()), ..DecisionFilter::default() };
        assert_eq!(store.decisions(&other_project).unwrap(), Vec::new());
    }

    #[test]
    fn re_extraction_replaces_a_session_and_records_its_size() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.extracted_turns("s1").unwrap(), None);
        store.replace_decisions(&batch("s1", 4, &[new_decision("A", &[], "accepted", "")])).unwrap();
        store.replace_decisions(&batch("s1", 9, &[new_decision("B", &[], "accepted", "")])).unwrap();
        assert_eq!(store.extracted_turns("s1").unwrap(), Some(9));
        let titles: Vec<String> =
            store.decisions(&DecisionFilter::default()).unwrap().into_iter().map(|d| d.title).collect();
        assert_eq!(titles, vec!["B"]);
    }

    #[test]
    fn review_marks_remember_the_reviewed_content() {
        let store = Store::open_in_memory().unwrap();
        store.set_review_mark("s1", "a.ts#f#Flow", "h1", true).unwrap();
        store.set_review_mark("s1", "a.ts#g#Flow", "h2", true).unwrap();
        store.set_review_mark("s1", "a.ts#f#Flow", "h3", true).unwrap();
        store.set_review_mark("s1", "a.ts#g#Flow", "h2", false).unwrap();
        let marks = store.review_marks("s1").unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks["a.ts#f#Flow"], "h3");
        assert!(store.review_marks("s2").unwrap().is_empty());
    }

    #[test]
    fn review_workspaces_are_remembered_per_session() {
        let store = Store::open_in_memory().unwrap();
        let workspace = ReviewWorkspace {
            session_id: "s1".into(),
            repo: "/repo".into(),
            path: "/repo/.claude/worktrees/huella-review-s1".into(),
            branch: "huella/review-s1".into(),
        };
        store.save_review_workspace(&workspace).unwrap();
        assert_eq!(store.review_workspace("s1").unwrap(), Some(workspace));
        store.delete_review_workspace("s1").unwrap();
        assert_eq!(store.review_workspace("s1").unwrap(), None);
    }

    #[test]
    fn there_is_at_most_one_active_review() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.active_review().unwrap(), None);
        store.set_active_review(Some("s1")).unwrap();
        store.set_active_review(Some("s2")).unwrap();
        assert_eq!(store.active_review().unwrap().as_deref(), Some("s2"));
        store.set_active_review(None).unwrap();
        assert_eq!(store.active_review().unwrap(), None);
    }

    #[test]
    fn narrations_are_stored_per_turn_and_replaced() {
        let store = Store::open_in_memory().unwrap();
        let first = StoredNarration { input_hash: "h1".into(), model: "m".into(), narration: "{}".into() };
        store.save_narration("s1", 0, &first).unwrap();
        let second = StoredNarration { input_hash: "h2".into(), ..first.clone() };
        store.save_narration("s1", 0, &second).unwrap();
        store.save_narration("s1", 3, &first).unwrap();
        let stored = store.narrations("s1").unwrap();
        assert_eq!((stored.len(), stored[&0].input_hash.as_str()), (2, "h2"));
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
