//! La skill de Claude Code que pide al agente registrar sus decisiones con
//! `huella_record`. Para agentes sin skills, el mismo texto va en `AGENTS.md`.

use std::path::PathBuf;

use anyhow::{Context, Result};

use super::Dirs;

/// La skill tal como se instala.
pub const SKILL: &str = include_str!("huella-registro.md");

/// Cómo está la skill instalada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillState {
    Missing,
    /// Es de otra versión de Huella.
    Outdated,
    Current,
}

pub fn skill_file(dirs: &Dirs) -> PathBuf {
    dirs.home.join(".claude").join("skills").join("huella-registro").join("SKILL.md")
}

pub fn state(dirs: &Dirs) -> SkillState {
    match std::fs::read_to_string(skill_file(dirs)) {
        Ok(text) if text == SKILL => SkillState::Current,
        Ok(_) => SkillState::Outdated,
        Err(_) => SkillState::Missing,
    }
}

/// Instala o actualiza la skill.
pub fn install(dirs: &Dirs) -> Result<()> {
    let path = skill_file(dirs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("no se pudo crear {}", parent.display()))?;
    }
    std::fs::write(&path, SKILL).with_context(|| format!("no se pudo escribir {}", path.display()))
}

/// El texto para `AGENTS.md`: la skill sin su cabecera, como una sección.
#[must_use]
pub fn agents_md() -> String {
    let body = SKILL.split("---").nth(2).unwrap_or(SKILL).trim();
    let body = body.strip_prefix("# Registrar y explicar para quien revise").unwrap_or(body).trim_start();
    format!("## Huella: registrar para quien revise\n\n{body}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skill_is_installed_and_kept_up_to_date() {
        let root = std::env::temp_dir().join(format!("huella-skill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dirs = Dirs::sandbox(&root);
        assert_eq!(state(&dirs), SkillState::Missing);
        install(&dirs).unwrap();
        assert_eq!(state(&dirs), SkillState::Current);
        std::fs::write(skill_file(&dirs), "antigua").unwrap();
        assert_eq!(state(&dirs), SkillState::Outdated);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn agents_md_has_the_body_without_the_frontmatter() {
        let text = agents_md();
        assert!(text.starts_with("## Huella: registrar para quien revise\n\nQuien revise"), "{text}");
        assert!(!text.contains("name: huella-registro"));
        assert!(text.contains("huella_record"));
    }
}
