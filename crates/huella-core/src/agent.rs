//! Lo que tiene que saber hacer el adaptador de un agente (Claude Code,
//! Copilot, Codex…) para que Huella indexe sus sesiones.
//!
//! Cada agente guarda sus sesiones a su manera: unos añaden eventos al final
//! de un fichero (Claude Code, Codex), otros reescriben el fichero entero
//! (Copilot en VS Code). El cursor permite a los primeros leer solo lo nuevo;
//! los segundos releen el fichero cuando cambia y el almacén descarta las
//! ediciones ya guardadas por su id.

use std::path::{Path, PathBuf};

use crate::{Activity, Conversation, FileEdit, TranscriptCursor};

/// Error de un adaptador al leer las sesiones de su agente.
#[derive(Debug, thiserror::Error)]
#[error("{agent}: no se pudo leer {path}: {message}")]
pub struct AgentError {
    pub agent: &'static str,
    pub path: PathBuf,
    pub message: String,
}

impl AgentError {
    pub fn new(agent: &'static str, path: &Path, message: &impl std::fmt::Display) -> Self {
        Self { agent, path: path.to_path_buf(), message: message.to_string() }
    }
}

/// Ediciones leídas de una sesión y dónde seguir la próxima vez.
#[derive(Debug, Default)]
pub struct AgentBatch {
    pub edits: Vec<FileEdit>,
    pub resume: TranscriptCursor,
}

/// Un agente de IA cuyas sesiones se indexan.
pub trait Agent: Send + Sync {
    /// Nombre estable, el mismo que va en [`crate::EditOrigin::agent`].
    fn name(&self) -> &'static str;

    /// Ficheros de sesión que existen ahora, ordenados.
    ///
    /// # Errors
    /// Si la carpeta del agente existe pero no se puede recorrer. Que no
    /// exista no es un error: el agente no está instalado.
    fn sessions(&self) -> Result<Vec<PathBuf>, AgentError>;

    /// Ediciones de un fichero de sesión a partir de `from`. Los formatos que
    /// reescriben el fichero lo leen entero y devuelven como cursor su tamaño.
    ///
    /// # Errors
    /// Si el fichero no se puede leer. Lo que no se entiende se ignora.
    fn read_from(&self, path: &Path, from: &TranscriptCursor) -> Result<AgentBatch, AgentError>;

    /// Todas las ediciones de un fichero de sesión.
    ///
    /// # Errors
    /// Si el fichero no se puede leer.
    fn read(&self, path: &Path) -> Result<AgentBatch, AgentError> {
        self.read_from(path, &TranscriptCursor::default())
    }

    /// Diálogo de un fichero de sesión (peticiones y respuestas, sin
    /// herramientas), para extraer decisiones.
    ///
    /// # Errors
    /// Si el fichero no se puede leer.
    fn conversations(&self, path: &Path) -> Result<Vec<Conversation>, AgentError>;

    /// Todo lo que hizo el agente en un fichero de sesión, en orden: qué leyó,
    /// qué ejecutó y con qué resultado, su plan y su razonamiento, con sus
    /// ediciones marcadas. Es la evidencia de cada paso de la revisión. Los
    /// agentes que no lo guardan no devuelven nada.
    ///
    /// # Errors
    /// Si el fichero no se puede leer.
    fn activity(&self, path: &Path) -> Result<Vec<Activity>, AgentError> {
        let _ = path;
        Ok(Vec::new())
    }
}

/// Ficheros bajo `root` (recursivo) cuyo nombre cumple `wanted`, ordenados.
/// Si `root` no existe, ninguno.
///
/// # Errors
/// Si un directorio existente no se puede leer.
pub fn find_files(
    agent: &'static str,
    root: &Path,
    wanted: impl Fn(&Path) -> bool,
) -> Result<Vec<PathBuf>, AgentError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|error| AgentError::new(agent, &dir, &error))?;
        for entry in entries {
            let path = entry.map_err(|error| AgentError::new(agent, &dir, &error))?.path();
            if path.is_dir() {
                pending.push(path);
            } else if wanted(&path) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}
