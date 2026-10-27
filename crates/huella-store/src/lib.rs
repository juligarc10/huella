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
    pub context: EditContext,
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

/// Qué decisiones listar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionFilter {
    pub entity: Option<String>,
    pub project: Option<String>,
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
        for decision in batch.decisions {
            tx.execute(
                "INSERT INTO decision (session_id, project, title, decision, rationale, alternatives, status,
                                       decided_at, extractor)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    batch.session_id,
                    batch.project,
                    decision.title,
                    decision.decision,
                    decision.rationale,
                    serde_json::to_string(&decision.alternatives).unwrap_or_else(|_| "[]".to_owned()),
                    decision.status,
                    batch.decided_at,
                    batch.extractor
                ],
            )?;
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

    /// Decisiones de la más reciente a la más antigua. Por defecto solo las
    /// vigentes (aceptadas), que son las que conviene recordar.
    ///
    /// # Errors
    /// Si falla la consulta.
    pub fn decisions(&self, filter: &DecisionFilter) -> Result<Vec<StoredDecision>> {
        let mut statement = self.conn.prepare_cached(
            "SELECT d.id, d.session_id, d.project, d.title, d.decision, d.rationale, d.alternatives, d.status,
                    d.superseded_by, d.decided_at, d.extractor,
                    (SELECT group_concat(entity, char(31)) FROM decision_entity WHERE decision_id = d.id)
             FROM decision d
             WHERE (?1 IS NULL OR EXISTS (SELECT 1 FROM decision_entity e WHERE e.decision_id = d.id AND e.entity = ?1))
               AND (?2 IS NULL OR d.project = ?2)
               AND (?3 OR d.status = 'accepted')
             ORDER BY d.decided_at DESC, d.id",
        )?;
        let entity = filter.entity.as_ref().map(|entity| entity.trim().to_lowercase());
        let rows = statement.query_map(params![entity, filter.project, filter.include_inactive], |row| {
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
            })
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

    fn new_decision(title: &str, entities: &[&str], status: &str, supersedes: &str) -> NewDecision {
        NewDecision {
            title: title.into(),
            decision: format!("se decide {title}"),
            rationale: "porque sí".into(),
            alternatives: vec!["otra: peor".into()],
            entities: entities.iter().map(|e| (*e).to_owned()).collect(),
            status: status.into(),
            supersedes: supersedes.into(),
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
