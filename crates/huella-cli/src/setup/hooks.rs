//! Hooks de Claude Code para los puntos de control: al acabar cada turno
//! (`Stop`) y antes de cada edición (`PreToolUse`). Se añaden a
//! `~/.claude/settings.json` junto a los que ya haya, sin tocarlos.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::Dirs;
use super::agents::read_json;

/// Herramientas de Claude Code que editan ficheros.
const EDIT_TOOLS: &str = "Edit|Write|MultiEdit";
/// Segundos que Claude Code espera al hook.
const TIMEOUT: u64 = 30;

pub fn settings_file(dirs: &Dirs) -> PathBuf {
    dirs.home.join(".claude").join("settings.json")
}

/// Si ya están los dos hooks de Huella.
pub fn installed(dirs: &Dirs) -> Result<bool> {
    let path = settings_file(dirs);
    let Some(settings) = read(&path)? else { return Ok(false) };
    Ok(has_hook(&settings, "Stop", "hook stop") && has_hook(&settings, "PreToolUse", "hook pre-edit"))
}

/// Añade los hooks que falten.
pub fn install(dirs: &Dirs, exe: &Path) -> Result<()> {
    let path = settings_file(dirs);
    let mut settings = read(&path)?.unwrap_or_else(|| json!({}));
    let hooks = settings
        .as_object_mut()
        .with_context(|| format!("{} no es un objeto JSON", path.display()))?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let exe = shell_quote(&exe.to_string_lossy());
    for (event, matcher, argument) in [("Stop", None, "hook stop"), ("PreToolUse", Some(EDIT_TOOLS), "hook pre-edit")] {
        if hooks.get(event).is_some_and(|groups| groups_have(groups, argument)) {
            continue;
        }
        let mut group =
            json!({"hooks": [{"type": "command", "command": format!("{exe} {argument}"), "timeout": TIMEOUT}]});
        if let Some(matcher) = matcher {
            group["matcher"] = json!(matcher);
        }
        let groups = hooks
            .as_object_mut()
            .with_context(|| format!("\"hooks\" en {} no es un objeto", path.display()))?
            .entry(event)
            .or_insert_with(|| json!([]));
        groups
            .as_array_mut()
            .with_context(|| format!("\"hooks.{event}\" en {} no es una lista", path.display()))?
            .push(group);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("no se pudo crear {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(&settings)?;
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("no se pudo escribir {}", path.display()))
}

/// Lo que habría que añadir a mano.
pub fn manual(dirs: &Dirs, exe: &Path) -> String {
    let exe = shell_quote(&exe.to_string_lossy());
    format!(
        "añade a {} dentro de \"hooks\": \"Stop\" con la orden `{exe} hook stop` y \"PreToolUse\" \
         (matcher \"{EDIT_TOOLS}\") con `{exe} hook pre-edit`",
        settings_file(dirs).display()
    )
}

fn read(path: &Path) -> Result<Option<Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(read_json(path, &text)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("no se pudo leer {}", path.display())),
    }
}

fn has_hook(settings: &Value, event: &str, argument: &str) -> bool {
    settings.pointer(&format!("/hooks/{event}")).is_some_and(|groups| groups_have(groups, argument))
}

/// Si algún grupo del evento ya ejecuta `huella … <argument>`.
fn groups_have(groups: &Value, argument: &str) -> bool {
    groups.as_array().into_iter().flatten().any(|group| {
        group["hooks"].as_array().into_iter().flatten().any(|hook| {
            hook["command"].as_str().is_some_and(|command| command.contains("huella") && command.ends_with(argument))
        })
    })
}

/// La ruta tal cual si no lleva nada que el shell interprete; si no, entre comillas simples.
fn shell_quote(text: &str) -> String {
    if text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(name: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("huella-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Dirs::sandbox(&root)
    }

    #[test]
    fn hooks_are_added_next_to_the_users_and_only_once() {
        let dirs = dirs("merge");
        let path = settings_file(&dirs);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say listo"}]}]}}"#,
        )
        .unwrap();
        assert!(!installed(&dirs).unwrap());
        let exe = Path::new("/Users/ana/mis cosas/huella");
        install(&dirs, exe).unwrap();
        install(&dirs, exe).unwrap();
        assert!(installed(&dirs).unwrap());

        let settings: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["model"], "opus");
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "el hook del usuario se queda y el de Huella no se duplica");
        assert_eq!(stop[0]["hooks"][0]["command"], "say listo");
        assert_eq!(stop[1]["hooks"][0]["command"], "'/Users/ana/mis cosas/huella' hook stop");
        let edit = &settings["hooks"]["PreToolUse"][0];
        assert_eq!(edit["matcher"], EDIT_TOOLS);
        assert_eq!(edit["hooks"][0]["command"], "'/Users/ana/mis cosas/huella' hook pre-edit");
        std::fs::remove_dir_all(dirs.home.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_missing_settings_file_is_created() {
        let dirs = dirs("new");
        assert!(!installed(&dirs).unwrap());
        install(&dirs, Path::new("/usr/local/bin/huella")).unwrap();
        assert!(installed(&dirs).unwrap());
        std::fs::remove_dir_all(dirs.home.parent().unwrap()).unwrap();
    }
}
