//! Modelo de dominio de Huella.
//!
//! Los adaptadores de agentes producen [`FileEdit`] normalizados; el anclaje los
//! convierte en un [`AnchorOutcome`] que apunta a [`Symbol`]s del código. Este
//! crate no conoce ningún formato de transcripción ni ningún parser.

use std::ops::Range;
use std::path::PathBuf;

use serde::Serialize;

/// De dónde viene una edición: agente, sesión y evento concreto.
#[derive(Debug, Clone, Serialize)]
pub struct EditOrigin {
    pub agent: &'static str,
    pub session_id: String,
    pub event_id: String,
    pub timestamp: Option<String>,
    /// Directorio de trabajo de la sesión; sirve para mostrar rutas relativas.
    pub cwd: Option<PathBuf>,
}

/// Cómo se produjo la edición.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditKind {
    /// Fichero nuevo.
    Create,
    /// Fichero existente reescrito entero.
    Rewrite,
    /// Sustitución de uno o varios fragmentos.
    Replace,
}

/// Contenido del fichero justo después de la edición.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterState {
    /// Se conoce el fichero completo.
    Known(String),
    /// La transcripción no guardó el fichero original: solo se conoce el texto
    /// insertado, que habrá que localizar en el fichero actual del disco.
    FragmentOnly(String),
}

/// Edición de un fichero, ya independiente del agente que la hizo.
#[derive(Debug, Clone)]
pub struct FileEdit {
    pub origin: EditOrigin,
    pub path: PathBuf,
    pub kind: EditKind,
    /// `None` si el fichero no existía.
    pub before: Option<String>,
    pub after: AfterState,
    /// El usuario modificó la propuesta del agente antes de aceptarla.
    pub user_modified: bool,
    pub context: EditContext,
}

/// Por qué se hizo la edición, tal como quedó en la conversación. Es el
/// contexto que se puede mostrar sin llamar a ningún modelo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EditContext {
    /// Lo que pidió la persona en el turno que llevó a la edición.
    pub prompt: Option<String>,
    /// Lo que explicó el agente justo antes de editar.
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    TypeAlias,
    Enum,
    Struct,
    Trait,
    Impl,
    Module,
    Variable,
    /// Bloque de test (`describe`, `it`, `test`).
    Test,
}

/// Símbolo de código con un nombre estable dentro de su fichero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Symbol {
    /// Nombre cualificado dentro del fichero: `Clase.metodo`, `Tipo::metodo`.
    pub qualified_name: String,
    pub kind: SymbolKind,
    /// Líneas 1-based, inclusivas.
    pub start_line: usize,
    pub end_line: usize,
    /// Hash del cuerpo normalizado (sin comentarios ni formato): permite saber
    /// si el código cambió desde que se ancló.
    pub body_hash: String,
    #[serde(skip)]
    pub byte_range: Range<usize>,
    /// Profundidad de anidamiento (0 = nivel de módulo).
    #[serde(skip)]
    pub depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// El fichero tras la edición se reconstruyó desde la transcripción.
    Exact,
    /// El fragmento se localizó en el fichero actual del disco, que puede
    /// haber cambiado después de la sesión.
    Approximate,
}

/// Resultado de anclar una edición.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AnchorOutcome {
    /// La edición se ancló a código.
    Anchored(Anchor),
    /// Lenguaje o formato sin extracción de símbolos: se ancla al fichero.
    FileOnly,
    /// El contenido final es idéntico al inicial.
    NoChange,
    /// No se pudo anclar; `reason` explica por qué.
    Unresolved { reason: String },
}

/// Código que tocó una edición.
#[derive(Debug, Clone, Serialize)]
pub struct Anchor {
    /// Símbolos más internos que contienen cada cambio, en el fichero final.
    pub symbols: Vec<Symbol>,
    /// Símbolos que existían antes de la edición y ya no existen.
    pub removed: Vec<Symbol>,
    /// Cambió código fuera de cualquier símbolo (imports, sentencias sueltas).
    pub module_level: bool,
    pub confidence: Confidence,
}

impl AnchorOutcome {
    /// La edición quedó anclada a código (símbolo o módulo).
    pub fn is_anchored(&self) -> bool {
        matches!(self, Self::Anchored(_))
    }

    pub fn anchor(&self) -> Option<&Anchor> {
        match self {
            Self::Anchored(anchor) => Some(anchor),
            _ => None,
        }
    }
}

impl Anchor {
    /// Solo cambió código de módulo: ningún símbolo tocado ni borrado.
    pub fn is_module_only(&self) -> bool {
        self.symbols.is_empty() && self.removed.is_empty()
    }
}
