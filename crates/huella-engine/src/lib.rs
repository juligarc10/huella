//! Casos de uso de Huella: indexar transcripciones y consultar la memoria de
//! un símbolo. Los usan tanto la CLI como el servidor del editor.

mod decisions;
mod index;
mod paths;
mod query;
mod render;

use std::path::PathBuf;

pub use decisions::{ExtractionReport, extract_decisions};
pub use index::{IndexReport, index};
pub use paths::normalize_path;
pub use query::{SymbolLens, SymbolMemory, lenses, memory_at};
pub use render::render_markdown;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] huella_store::StoreError),
    #[error(transparent)]
    Transcript(#[from] huella_claude_code::TranscriptError),
    #[error(transparent)]
    Extraction(#[from] huella_anchoring::ExtractionError),
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
