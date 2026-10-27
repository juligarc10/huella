//! Casos de uso de Huella: indexar transcripciones y consultar la memoria de
//! un símbolo. Los usan tanto la CLI como el servidor del editor.

mod decisions;
mod index;
pub mod lineage;
pub mod narration;
mod paths;
mod query;
pub mod refactor_bench;
mod render;
mod review;
pub mod workspace;

use std::path::PathBuf;

pub use decisions::{ExtractionOptions, ExtractionReport, extract_decisions};
pub use index::{IndexReport, index};
pub use paths::normalize_path;
pub use query::{SymbolLens, SymbolMemory, lenses, lenses_with, memory_at, memory_at_with, memory_of};
pub use render::render_markdown;
pub use review::{SessionListing, list_sessions, session_project, session_review};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] huella_store::StoreError),
    #[error(transparent)]
    Transcript(#[from] huella_claude_code::TranscriptError),
    #[error(transparent)]
    Extraction(#[from] huella_anchoring::ExtractionError),
    #[error("{0}")]
    Git(String),
    #[error("ninguna sesión empieza por «{0}»")]
    SessionNotFound(String),
    #[error("«{prefix}» coincide con varias sesiones: {}", candidates.join(", "))]
    AmbiguousSession { prefix: String, candidates: Vec<String> },
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Base de datos por defecto: `$HUELLA_DB`, o la carpeta de datos del usuario
/// (`~/Library/Application Support/huella/huella.db` en macOS,
/// `~/.local/share/huella/huella.db` en Linux).
pub fn default_db_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HUELLA_DB") {
        return Some(PathBuf::from(path));
    }
    dirs::data_dir().map(|dir| dir.join("huella").join("huella.db"))
}

/// Transcripciones de Claude Code: `~/.claude/projects`.
pub fn default_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("projects"))
}
