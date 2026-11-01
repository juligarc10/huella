//! `huella hook`: lo que ejecuta Claude Code en sus hooks para los puntos de
//! control. Recibe el evento en JSON por stdin y responde por stdout en el
//! formato de Claude Code. Un fallo de Huella nunca debe parar al agente: los
//! errores se cuentan por stderr y el hook sale bien sin decir nada.

use std::io::Read as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// Evento de Claude Code que atiende el hook.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum HookEvent {
    /// Al acabar un turno: avisa de lo obligatorio sin revisar.
    Stop,
    /// Antes de cada edición: la para si la política lo pide.
    PreEdit,
}

/// Lee el evento de stdin, lo atiende y escribe la respuesta, si la hay.
/// Siempre sale bien: los fallos solo se cuentan.
#[allow(clippy::unnecessary_wraps, reason = "encaja con el resto de órdenes")]
pub fn run(event: HookEvent, db: Option<PathBuf>, projects: Option<PathBuf>) -> Result<()> {
    let mut input = String::new();
    let outcome = std::io::stdin()
        .read_to_string(&mut input)
        .context("no se pudo leer el evento")
        .and_then(|_| serde_json::from_str::<Value>(&input).context("el evento no es JSON"))
        .and_then(|input| respond(event, &input, db, projects));
    match outcome {
        Ok(Some(output)) => println!("{output}"),
        Ok(None) => {}
        Err(error) => eprintln!("huella: {error:#}"),
    }
    Ok(())
}

fn respond(event: HookEvent, input: &Value, db: Option<PathBuf>, projects: Option<PathBuf>) -> Result<Option<Value>> {
    let session = input["session_id"].as_str().context("el evento no trae session_id")?;
    let cwd = input["cwd"].as_str();
    match event {
        HookEvent::Stop => {
            let mut store = crate::open_store(db)?;
            let sources = crate::sources(projects)?;
            huella_engine::index(&mut store, &sources)?;
            let checkpoint = huella_engine::checkpoint::check(&store, &sources, session, cwd)?;
            Ok(checkpoint.as_ref().and_then(huella_engine::checkpoint::notice).map(|notice| stop_output(&notice)))
        }
        HookEvent::PreEdit => {
            // Sin límite en la política no hace falta ni abrir la base de datos.
            let policy = huella_engine::triage::load_policy(cwd.map(std::path::Path::new)).unwrap_or_default();
            if policy.checkpoints.block_after.is_none() {
                return Ok(None);
            }
            let store = crate::open_store(db)?;
            let sources = crate::sources(projects)?;
            Ok(huella_engine::checkpoint::gate(&store, &sources, session, cwd, &policy)?
                .map(|reason| deny_output(&reason)))
        }
    }
}

/// Mensaje para la persona al acabar el turno.
fn stop_output(notice: &str) -> Value {
    json!({ "systemMessage": notice })
}

/// Rechazo de la edición, con el porqué para el agente.
fn deny_output(reason: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_without_a_session_are_errors_not_decisions() {
        assert!(respond(HookEvent::PreEdit, &json!({}), None, None).is_err());
    }

    #[test]
    fn edits_pass_without_a_limit_in_the_policy() {
        let dir = std::env::temp_dir().join(format!("huella-hook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = json!({"session_id": "s1", "cwd": dir, "hook_event_name": "PreToolUse"});
        // Sin .huella.toml no se abre ni la base de datos: una ruta imposible no falla.
        let db = Some(PathBuf::from("/no-existe/huella.db"));
        assert_eq!(respond(HookEvent::PreEdit, &input, db, None).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn outputs_follow_the_claude_code_format() {
        assert_eq!(stop_output("hola"), json!({"systemMessage": "hola"}));
        let deny = deny_output("para");
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(deny["hookSpecificOutput"]["permissionDecisionReason"], "para");
    }
}
