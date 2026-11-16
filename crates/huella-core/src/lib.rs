//! Modelo de dominio de Huella.
//!
//! Los adaptadores de agentes producen [`FileEdit`] normalizados; el anclaje los
//! convierte en un [`AnchorOutcome`] que apunta a [`Symbol`]s del código. Este
//! crate no conoce ningún formato de transcripción ni ningún parser.

pub mod activity;
pub mod agent;
mod fingerprint;
pub mod redact;

use std::ops::Range;
use std::path::PathBuf;

use serde::Serialize;

pub use activity::{Activity, ActivityKind, PlanItem};
pub use agent::{Agent, AgentBatch, AgentError};
pub use fingerprint::{Fingerprint, SELF_TOKEN, SKETCH_SIZE};

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
    /// Como `FragmentOnly`, pero con los reemplazos exactos: quien conozca el
    /// fichero de antes (la edición anterior de la sesión, o el disco deshaciendo
    /// la sesión) puede reconstruir el de después.
    Replaced { fragment: String, replacements: Vec<Replacement> },
}

impl AfterState {
    /// El texto insertado, si no se conoce el fichero completo.
    #[must_use]
    pub fn fragment(&self) -> Option<&str> {
        match self {
            Self::Known(_) => None,
            Self::FragmentOnly(fragment) | Self::Replaced { fragment, .. } => Some(fragment),
        }
    }
}

/// Un reemplazo de texto de una edición (`Edit`/`MultiEdit` de Claude Code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pub old: String,
    pub new: String,
    pub replace_all: bool,
}

impl Replacement {
    /// Aplica los reemplazos en orden. `None` si alguno no encaja.
    #[must_use]
    pub fn apply(text: &str, replacements: &[Self]) -> Option<String> {
        let mut text = text.to_owned();
        for r in replacements {
            if r.old.is_empty() {
                // Claude Code permite crear un fichero vacío con old_string = "".
                if !text.is_empty() {
                    return None;
                }
                text.clone_from(&r.new);
            } else if !text.contains(&r.old) {
                return None;
            } else if r.replace_all {
                text = text.replace(&r.old, &r.new);
            } else {
                text = text.replacen(&r.old, &r.new, 1);
            }
        }
        Some(text)
    }

    /// Deshace los reemplazos, del último al primero. Solo si es inequívoco: el
    /// texto nuevo tiene que aparecer una vez (o, con `replace_all`, al menos una
    /// y sin estar ya en el texto viejo). `None` si no se puede.
    #[must_use]
    pub fn undo(text: &str, replacements: &[Self]) -> Option<String> {
        let mut text = text.to_owned();
        for r in replacements.iter().rev() {
            if r.new.is_empty() {
                return None;
            }
            let count = text.matches(r.new.as_str()).count();
            if r.replace_all {
                if count == 0 || r.old.contains(&r.new) {
                    return None;
                }
                text = text.replace(&r.new, &r.old);
            } else if count == 1 {
                text = text.replacen(&r.new, &r.old, 1);
            } else {
                return None;
            }
        }
        Some(text)
    }
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

/// Punto de lectura de una transcripción: byte siguiente a la última línea
/// completa y el contexto de conversación vigente en ese punto. Permite
/// indexar solo lo nuevo sin perder qué se estaba pidiendo.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TranscriptCursor {
    pub offset: u64,
    pub context: EditContext,
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
    /// Huella independiente del nombre, para reconocerlo si se renombra o se mueve.
    #[serde(skip)]
    pub fingerprint: Fingerprint,
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

/// Quién habla en un turno de conversación.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    Person,
    Agent,
}

/// Un mensaje de la conversación, sin herramientas ni sus resultados.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub speaker: Speaker,
    pub text: String,
    pub timestamp: Option<String>,
}

/// El diálogo de una sesión: de ahí salen las decisiones que no tocan código.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub session_id: String,
    pub cwd: Option<PathBuf>,
    pub turns: Vec<Turn>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement(old: &str, new: &str, replace_all: bool) -> Replacement {
        Replacement { old: old.into(), new: new.into(), replace_all }
    }

    #[test]
    fn replacements_apply_and_undo_only_when_unambiguous() {
        let edits = [replacement("uno", "UNO", false), replacement("dos", "DOS", true)];
        let after = Replacement::apply("uno dos dos", &edits).unwrap();
        assert_eq!(after, "UNO DOS DOS");
        assert_eq!(Replacement::undo(&after, &edits).as_deref(), Some("uno dos dos"));
        assert_eq!(Replacement::apply("tres", &edits), None, "lo viejo no está");
        assert_eq!(
            Replacement::undo("UNO UNO DOS", &edits),
            None,
            "lo nuevo aparece dos veces: no se sabe cuál deshacer"
        );
        assert_eq!(Replacement::undo("x", &[replacement("x", "", false)]), None, "un borrado no se puede deshacer");
        assert_eq!(Replacement::apply("", &[replacement("", "nuevo", false)]).as_deref(), Some("nuevo"));
    }
}
