//! De qué agentes se leen las sesiones.

use std::path::PathBuf;
use std::sync::Arc;

use huella_claude_code::ClaudeCode;
use huella_codex::Codex;
use huella_copilot::{CopilotChat, CopilotCli};
use huella_core::Agent;

/// Los agentes cuyas sesiones se indexan. Barato de clonar: se comparte entre
/// el servidor del editor, sus hilos y las tareas de la aplicación.
#[derive(Clone)]
pub struct Sources {
    agents: Arc<Vec<Box<dyn Agent>>>,
}

impl Sources {
    #[must_use]
    pub fn new(agents: Vec<Box<dyn Agent>>) -> Self {
        Self { agents: Arc::new(agents) }
    }

    /// Los agentes instalados en esta máquina, con las transcripciones de
    /// Claude Code bajo `claude_projects`.
    #[must_use]
    pub fn detect(claude_projects: impl Into<PathBuf>) -> Self {
        let mut agents: Vec<Box<dyn Agent>> = vec![Box::new(ClaudeCode { projects: claude_projects.into() })];
        // Los demás viven en carpetas conocidas de la carpeta personal; si no
        // están instalados, simplemente no aportan sesiones.
        if let Some(home) = dirs::home_dir() {
            agents.push(Box::new(CopilotChat::detect(&home)));
            agents.push(Box::new(CopilotCli::detect(&home)));
            agents.push(Box::new(Codex::detect(&home)));
        }
        Self::new(agents)
    }

    /// Solo Claude Code, con sus transcripciones bajo `projects`.
    #[must_use]
    pub fn claude_code(projects: impl Into<PathBuf>) -> Self {
        Self::new(vec![Box::new(ClaudeCode { projects: projects.into() })])
    }

    #[must_use]
    pub fn agents(&self) -> &[Box<dyn Agent>] {
        &self.agents
    }

    /// Nombres de los agentes, para mostrarlos.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.agents.iter().map(|agent| agent.name()).collect()
    }
}

impl std::fmt::Debug for Sources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sources").field("agents", &self.names()).finish()
    }
}
