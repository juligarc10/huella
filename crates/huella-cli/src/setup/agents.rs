//! Registro del servidor MCP de Huella en cada agente instalado.
//!
//! Cada agente guarda sus servidores MCP en un sitio distinto. Registrar es
//! idempotente: si `huella` ya está, no se toca; si el fichero del agente no se
//! puede leer como se espera (JSON con comentarios, por ejemplo), no se pisa y
//! se explica cómo hacerlo a mano.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::Dirs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpAgent {
    ClaudeCode,
    Codex,
    CopilotVscode,
    CopilotCli,
}

impl McpAgent {
    pub const ALL: [Self; 4] = [Self::ClaudeCode, Self::Codex, Self::CopilotVscode, Self::CopilotCli];

    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::CopilotVscode => "Copilot (VS Code)",
            Self::CopilotCli => "Copilot CLI",
        }
    }

    /// Fichero donde el agente guarda sus servidores MCP.
    pub fn config_file(self, dirs: &Dirs) -> PathBuf {
        match self {
            Self::ClaudeCode => dirs.home.join(".claude.json"),
            Self::Codex => dirs.home.join(".codex").join("config.toml"),
            Self::CopilotVscode => vscode_user_dir(dirs).join("mcp.json"),
            Self::CopilotCli => dirs.home.join(".copilot").join("mcp-config.json"),
        }
    }

    pub fn detected(self, dirs: &Dirs) -> bool {
        match self {
            Self::ClaudeCode => dirs.home.join(".claude").is_dir() || dirs.on_path("claude").is_some(),
            Self::Codex => dirs.home.join(".codex").is_dir() || dirs.on_path("codex").is_some(),
            Self::CopilotVscode => vscode_user_dir(dirs).is_dir(),
            Self::CopilotCli => dirs.home.join(".copilot").is_dir(),
        }
    }

    /// Si `huella` ya está entre sus servidores MCP.
    pub fn registered(self, dirs: &Dirs) -> Result<bool> {
        let path = self.config_file(dirs);
        if !path.exists() {
            return Ok(false);
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("no se pudo leer {}", path.display()))?;
        Ok(match self {
            Self::ClaudeCode | Self::CopilotCli => read_json(&path, &text)?.pointer("/mcpServers/huella").is_some(),
            Self::CopilotVscode => read_json(&path, &text)?.pointer("/servers/huella").is_some(),
            Self::Codex => {
                let document: toml_edit::DocumentMut =
                    text.parse().with_context(|| format!("{} no es TOML válido", path.display()))?;
                document.get("mcp_servers").and_then(|servers| servers.get("huella")).is_some()
            }
        })
    }

    /// Lo que habría que hacer a mano, para cuando no se puede automáticamente.
    pub fn manual(self, dirs: &Dirs, exe: &Path) -> String {
        let exe = exe.display();
        match self {
            Self::ClaudeCode => format!("claude mcp add --scope user huella -- {exe} mcp"),
            Self::Codex => format!(
                "añade a {}:\n[mcp_servers.huella]\ncommand = \"{exe}\"\nargs = [\"mcp\"]",
                self.config_file(dirs).display()
            ),
            Self::CopilotVscode => format!(
                "añade a {} dentro de \"servers\": \"huella\": {{ \"type\": \"stdio\", \"command\": \"{exe}\", \"args\": [\"mcp\"] }}",
                self.config_file(dirs).display()
            ),
            Self::CopilotCli => format!(
                "añade a {} dentro de \"mcpServers\": \"huella\": {{ \"type\": \"local\", \"command\": \"{exe}\", \"args\": [\"mcp\"], \"tools\": [\"*\"] }}",
                self.config_file(dirs).display()
            ),
        }
    }

    pub fn register(self, dirs: &Dirs, exe: &Path) -> Result<()> {
        if self.registered(dirs)? {
            return Ok(());
        }
        let path = self.config_file(dirs);
        let command = exe.to_string_lossy().into_owned();
        match self {
            // Claude Code tiene su propia orden; su fichero lo reescribe a menudo.
            Self::ClaudeCode => {
                let claude = dirs.on_path("claude").context("no se encontró la orden claude en el PATH")?;
                let status = Command::new(claude)
                    .args(["mcp", "add", "--scope", "user", "huella", "--", &command, "mcp"])
                    .status()
                    .context("no se pudo ejecutar claude mcp add")?;
                if !status.success() {
                    bail!("claude mcp add terminó con {status}");
                }
                Ok(())
            }
            Self::Codex => crate::config::update(&path, |document| {
                let servers = crate::config::table(document.as_table_mut(), "mcp_servers");
                // Sin cabecera [mcp_servers] vacía: solo [mcp_servers.huella].
                servers.set_implicit(true);
                let huella = crate::config::table(servers, "huella");
                huella["command"] = toml_edit::value(command.as_str());
                huella["args"] = toml_edit::value(toml_edit::Array::from_iter(["mcp"]));
            }),
            Self::CopilotVscode => {
                update_json(&path, "servers", json!({"type": "stdio", "command": command, "args": ["mcp"]}))
            }
            Self::CopilotCli => update_json(
                &path,
                "mcpServers",
                json!({"type": "local", "command": command, "args": ["mcp"], "tools": ["*"]}),
            ),
        }
    }
}

/// Carpeta de usuario de VS Code (`…/Code/User`).
pub fn vscode_user_dir(dirs: &Dirs) -> PathBuf {
    dirs.config.join("Code").join("User")
}

fn read_json(path: &Path, text: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(text)
        .with_context(|| format!("{} no es JSON puro (¿tiene comentarios?); edítalo a mano", path.display()))
}

/// Añade `huella` bajo `key` sin tocar el resto del fichero.
fn update_json(path: &Path, key: &str, server: Value) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("no se pudo leer {}", path.display())),
    };
    let mut root = read_json(path, &text)?;
    let object = root.as_object_mut().with_context(|| format!("{} no es un objeto JSON", path.display()))?;
    let servers = object.entry(key).or_insert_with(|| json!({}));
    servers
        .as_object_mut()
        .with_context(|| format!("\"{key}\" en {} no es un objeto", path.display()))?
        .insert("huella".to_owned(), server);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("no se pudo crear {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(&root)?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("no se pudo escribir {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(name: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("huella-agents-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Dirs::sandbox(&root)
    }

    #[test]
    fn codex_registration_is_idempotent_and_keeps_the_rest() {
        let dirs = dirs("codex");
        let path = McpAgent::Codex.config_file(&dirs);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# mío\nmodel = \"gpt-5\"\n").unwrap();
        assert!(!McpAgent::Codex.registered(&dirs).unwrap());
        McpAgent::Codex.register(&dirs, Path::new("/usr/local/bin/huella")).unwrap();
        McpAgent::Codex.register(&dirs, Path::new("/usr/local/bin/huella")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mío") && text.contains("model = \"gpt-5\""));
        assert_eq!(text.matches("[mcp_servers.huella]").count(), 1, "{text}");
        assert!(McpAgent::Codex.registered(&dirs).unwrap());
    }

    #[test]
    fn copilot_files_are_created_or_extended() {
        let dirs = dirs("copilot");
        let vscode = McpAgent::CopilotVscode.config_file(&dirs);
        std::fs::create_dir_all(vscode.parent().unwrap()).unwrap();
        std::fs::write(&vscode, r#"{"servers": {"otro": {"command": "x"}}, "inputs": []}"#).unwrap();
        McpAgent::CopilotVscode.register(&dirs, Path::new("/bin/huella")).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&vscode).unwrap()).unwrap();
        assert_eq!(saved["servers"]["otro"]["command"], "x");
        assert_eq!(saved["servers"]["huella"]["args"][0], "mcp");
        assert!(saved["inputs"].is_array());

        McpAgent::CopilotCli.register(&dirs, Path::new("/bin/huella")).unwrap();
        assert!(McpAgent::CopilotCli.registered(&dirs).unwrap());
    }

    #[test]
    fn json_with_comments_is_not_overwritten() {
        let dirs = dirs("jsonc");
        let vscode = McpAgent::CopilotVscode.config_file(&dirs);
        std::fs::create_dir_all(vscode.parent().unwrap()).unwrap();
        let original = "{\n  // mis servidores\n  \"servers\": {}\n}\n";
        std::fs::write(&vscode, original).unwrap();
        assert!(McpAgent::CopilotVscode.register(&dirs, Path::new("/bin/huella")).is_err());
        assert_eq!(std::fs::read_to_string(&vscode).unwrap(), original);
    }

    #[test]
    fn claude_code_is_read_from_its_user_file() {
        let dirs = dirs("claude");
        std::fs::create_dir_all(&dirs.home).unwrap();
        std::fs::write(dirs.home.join(".claude.json"), r#"{"mcpServers": {"huella": {"command": "huella"}}}"#).unwrap();
        assert!(McpAgent::ClaudeCode.registered(&dirs).unwrap());
    }
}
