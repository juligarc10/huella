//! Anclaje de ediciones a símbolos de código con tree-sitter.

mod anchor;
mod language;
mod symbols;

pub use anchor::{AnchorReport, anchor_edit};
pub use language::Language;
pub use symbols::{Extraction, ExtractionError, extract_symbols, identifiers};
