//! Extensión de Zed: solo arranca `huella lsp`, toda la lógica vive en el binario.

use zed_extension_api::{self as zed, LanguageServerId, Result};

struct Huella;

impl zed::Extension for Huella {
    fn new() -> Self {
        Self
    }

    fn language_server_command(&mut self, _id: &LanguageServerId, worktree: &zed::Worktree) -> Result<zed::Command> {
        let command = worktree.which("huella").ok_or_else(|| {
            "No se encontró `huella` en el PATH. Instálalo con `cargo install --path crates/huella-cli`.".to_owned()
        })?;
        Ok(zed::Command { command, args: vec!["lsp".to_owned()], env: worktree.shell_env() })
    }
}

zed::register_extension!(Huella);
