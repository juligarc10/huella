//! Identidad de un fichero: la misma ruta tanto al indexar como al consultar.

use std::path::{Component, Path, PathBuf};

/// Ruta absoluta con la que se guarda y se busca la historia de un fichero.
///
/// Las rutas relativas se resuelven contra el directorio de la sesión, y las
/// de un worktree de Claude Code (`repo/.claude/worktrees/<nombre>/src/a.rs`)
/// se llevan al repositorio principal (`repo/src/a.rs`): es el mismo código
/// trabajado en una copia aislada.
pub fn normalize_path(path: &Path, cwd: Option<&Path>) -> String {
    let absolute = match cwd {
        Some(cwd) if path.is_relative() => cwd.join(path),
        _ => path.to_path_buf(),
    };
    let components: Vec<Component> = absolute.components().collect();
    let mut normalized = PathBuf::new();
    let mut index = 0;
    while index < components.len() {
        let is_worktree = components.get(index).is_some_and(|c| c.as_os_str() == ".claude")
            && components.get(index + 1).is_some_and(|c| c.as_os_str() == "worktrees")
            && index + 3 < components.len();
        if is_worktree {
            index += 3;
            continue;
        }
        match components[index] {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
        index += 1;
    }
    normalized.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_paths_map_to_the_main_repository() {
        let path = Path::new("/dev/huella/.claude/worktrees/feature+f1/crates/core/src/lib.rs");
        assert_eq!(normalize_path(path, None), "/dev/huella/crates/core/src/lib.rs");
    }

    #[test]
    fn relative_paths_resolve_against_the_session_directory() {
        assert_eq!(normalize_path(Path::new("./src/../src/a.ts"), Some(Path::new("/repo"))), "/repo/src/a.ts");
        assert_eq!(normalize_path(Path::new("/abs/a.ts"), Some(Path::new("/repo"))), "/abs/a.ts");
    }

    #[test]
    fn a_file_named_like_the_marker_is_left_alone() {
        assert_eq!(normalize_path(Path::new("/repo/.claude/worktrees"), None), "/repo/.claude/worktrees");
    }
}
