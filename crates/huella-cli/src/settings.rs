//! Los ajustes del modelo desde la aplicación: lo mismo que `[llm]` de
//! `config.toml`, sin editar el fichero ni reabrir `huella ui`.
//!
//! Se escribe en el mismo fichero, conservando lo que la persona escribió a mano.
//! Lo que fija una variable de entorno (u opción de la orden) manda sobre el
//! fichero: se enseña, pero no se cambia desde aquí. La clave nunca va en
//! `config.toml` ni vuelve a la página: se guarda en su fichero con permisos 600.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{ModelArgs, Provider, config};

/// La última prueba de conexión: `None` si no se ha probado.
pub(crate) type Probing = Arc<Mutex<Option<Value>>>;

/// El contexto que se supone si no se dice (el de `config.toml`).
const DEFAULT_CONTEXT: usize = 32_768;

fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "anthropic",
        Provider::OpenaiCompatible => "openai-compatible",
    }
}

/// De dónde sale un valor: `env` (variable u opción de la orden, manda),
/// `file` (`config.toml`) o `default`.
fn source(env: bool, file: bool) -> &'static str {
    if env {
        "env"
    } else if file {
        "file"
    } else {
        "default"
    }
}

fn key_env(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "ANTHROPIC_API_KEY",
        Provider::OpenaiCompatible => "HUELLA_LLM_API_KEY",
    }
}

/// Lo que se sabe de la clave, nunca la clave.
fn key_view(provider: Provider) -> Value {
    let env = key_env(provider);
    if std::env::var(env).is_ok_and(|key| !key.trim().is_empty()) {
        return json!({"from": "env", "env": env});
    }
    match config::get().llm.api_key_file.clone() {
        Some(file) => {
            let problem = config::read_key_file(&file).err().map(|error| format!("{error:#}"));
            json!({"from": "file", "file": file, "env": env, "problem": problem})
        }
        None => json!({"from": "missing", "env": env}),
    }
}

/// Los ajustes como los ve la página. Relee `config.toml` por si se cambió a mano.
pub(crate) fn view(args: &ModelArgs, probing: &Probing) -> Value {
    let reload = config::init().err().map(|error| format!("{error:#}"));
    let config = config::get();
    let llm = &config.llm;
    let provider = args.provider();
    json!({
        "path": config::path(),
        "invalid": reload,
        "provider": {"value": provider_name(provider), "from": source(args.provider.is_some(), llm.provider.is_some()), "env": "HUELLA_LLM_PROVIDER"},
        "base_url": {"value": args.base_url(), "from": source(args.base_url.is_some(), llm.base_url.is_some()), "env": "HUELLA_LLM_BASE_URL"},
        "model": {"value": args.model(), "from": source(args.model.is_some(), llm.model.is_some()), "env": "HUELLA_LLM_MODEL"},
        "thinking": {"value": llm.thinking, "from": source(false, llm.thinking.is_some())},
        "context_tokens": {"value": llm.context_tokens.unwrap_or(DEFAULT_CONTEXT), "from": source(false, llm.context_tokens.is_some())},
        "language": {
            "value": config.review.language,
            "options": crate::language::LANGUAGES.iter().map(|(code, label, _)| json!({"code": code, "label": label})).collect::<Vec<_>>(),
        },
        "key": key_view(provider),
        "problem": crate::model_problem(args),
        "probe": probing.lock().unwrap_or_else(PoisonError::into_inner).clone(),
    })
}

/// Si se puede pedir «pensar más» en una pregunta y qué hace el modelo si no.
/// Solo los servidores compatibles con `OpenAI` lo aceptan (Qwen3 y similares).
pub(crate) fn thinking_option(args: &ModelArgs) -> Value {
    json!({
        "available": matches!(args.provider(), Provider::OpenaiCompatible),
        "default": config::get().llm.thinking,
    })
}

/// Un texto de una línea, sin caracteres de control y de longitud razonable.
fn plain(value: &Value, name: &str, max: usize) -> Result<Option<String>> {
    let Some(text) = value.as_str() else {
        if value.is_null() {
            return Ok(None);
        }
        bail!("{name} debe ser texto");
    };
    let text = text.trim();
    if text.chars().any(char::is_control) || text.len() > max {
        bail!("{name} no es válido");
    }
    Ok(Some(text.to_owned()).filter(|text| !text.is_empty()))
}

fn config_path() -> Result<std::path::PathBuf> {
    config::path().context("no se sabe dónde está config.toml")
}

/// Guarda en `[llm]` de `config.toml` los campos que trae `body` (`null` o vacío
/// quita el campo: vuelve el valor por defecto) y recarga la configuración.
pub(crate) fn save(body: &str) -> Result<()> {
    save_in(&config_path()?, body)
}

fn save_in(path: &std::path::Path, body: &str) -> Result<()> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let Some(fields) = request.as_object() else { bail!("faltan los ajustes") };
    let mut edits: Vec<(&str, &str, Option<toml_edit::Value>)> = Vec::new();
    for (name, value) in fields {
        let edit = match name.as_str() {
            "provider" => match plain(value, "el proveedor", 40)?.as_deref() {
                None => None,
                Some(provider @ ("anthropic" | "openai-compatible")) => Some(provider.into()),
                Some(_) => bail!("proveedor desconocido"),
            },
            "base_url" => match plain(value, "la URL", 2048)? {
                None => None,
                Some(url) if (url.starts_with("http://") || url.starts_with("https://")) && !url.contains(' ') => {
                    Some(url.into())
                }
                Some(_) => bail!("la URL debe empezar por http:// o https://"),
            },
            "model" => plain(value, "el modelo", 200)?.map(Into::into),
            "thinking" => match value {
                Value::Null => None,
                Value::Bool(enabled) => Some((*enabled).into()),
                _ => bail!("thinking debe ser sí, no o sin definir"),
            },
            "language" => match plain(value, "el idioma", 10)?.as_deref() {
                None | Some("auto") => None,
                Some(code) if crate::language::LANGUAGES.iter().any(|(known, _, _)| *known == code) => {
                    Some(code.into())
                }
                Some(_) => bail!("idioma desconocido"),
            },
            "context_tokens" if value.is_null() => None,
            "context_tokens" => {
                let tokens = value.as_i64().filter(|tokens| (1024..=10_000_000).contains(tokens));
                Some(tokens.context("el contexto debe estar entre 1024 y 10 000 000 tokens")?.into())
            }
            other => bail!("ajuste desconocido: {other}"),
        };
        let (table, key) = match name.as_str() {
            "provider" => ("llm", "provider"),
            "base_url" => ("llm", "base_url"),
            "model" => ("llm", "model"),
            "thinking" => ("llm", "thinking"),
            "language" => ("review", "language"),
            _ => ("llm", "context_tokens"),
        };
        edits.push((table, key, edit));
    }
    config::update(path, |document| {
        for (table, key, value) in edits {
            let section = config::table(document.as_table_mut(), table);
            match value {
                Some(value) => section[key] = toml_edit::Item::Value(value),
                None => {
                    section.remove(key);
                }
            }
        }
    })?;
    config::init_from(Some(path))
}

/// Guarda la clave en su fichero (el de `api_key_file`, o el de siempre) con
/// permisos 600, y apunta a él desde `config.toml` si hacía falta.
pub(crate) fn save_key(body: &str) -> Result<()> {
    save_key_in(&config_path()?, body)
}

fn save_key_in(path: &std::path::Path, body: &str) -> Result<()> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let key = request["key"].as_str().map(str::trim).unwrap_or_default();
    if key.is_empty() || key.len() > 4096 || key.chars().any(|c| c.is_control() || c.is_whitespace()) {
        bail!("la clave no es válida");
    }
    let configured = config::get().llm.api_key_file.clone();
    let file = match &configured {
        Some(file) => config::expand_home(file),
        None => config::default_key_file().context("no se encuentra la carpeta personal")?,
    };
    config::write_key_file(&file, key)?;
    if configured.is_none() {
        config::update(path, |document| {
            config::table(document.as_table_mut(), "llm")["api_key_file"] =
                toml_edit::value(file.display().to_string());
        })?;
    }
    config::init_from(Some(path))
}

/// Prueba el modelo con una petición mínima, en otro hilo: puede tardar si el
/// servidor tiene que cargarlo. El resultado se lee con `view`.
pub(crate) fn probe(args: &ModelArgs, probing: &Probing) {
    *probing.lock().unwrap_or_else(PoisonError::into_inner) = Some(json!({"running": true}));
    let (args, probing) = (args.clone(), Arc::clone(probing));
    std::thread::spawn(move || {
        let started = Instant::now();
        let outcome = (|| -> Result<String> {
            let model = crate::build_model(&args)?;
            let schema = json!({
                "type": "object",
                "properties": {"ok": {"type": "boolean"}},
                "required": ["ok"],
                "additionalProperties": false
            });
            model
                .complete_json("Respondes solo con el JSON que se pide.", "Responde {\"ok\": true}.", &schema)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Ok(model.model_id())
        })();
        let millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let result = match outcome {
            Ok(model) => json!({"running": false, "ok": true, "millis": millis, "model": model}),
            // Un error de red o del servidor no debería llevar la clave, pero por si acaso.
            Err(error) => json!({
                "running": false,
                "ok": false,
                "millis": millis,
                "error": huella_core::redact::redact(&format!("{error:#}")).into_owned(),
            }),
        };
        *probing.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_config<T>(name: &str, text: &str, check: impl FnOnce(&std::path::Path) -> T) -> T {
        let _guard = config::TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("huella-settings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        config::init_from(Some(&path)).unwrap();
        let result = check(&path);
        config::init_from(None).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        result
    }

    #[test]
    fn saving_keeps_comments_removes_cleared_fields_and_applies_at_once() {
        with_config("save", "# mío\n[llm]\nmodel = \"viejo\" # a mano\nthinking = true\n", |path| {
            save_in(path, r#"{"provider": "openai-compatible", "base_url": "http://localhost:8080/v1", "model": "qwen", "thinking": null, "context_tokens": 65536}"#)
                .unwrap();
            let text = std::fs::read_to_string(path).unwrap();
            assert!(text.contains("# mío"), "{text}");
            assert!(!text.contains("thinking"), "null quita el campo: {text}");
            let llm = &config::get().llm;
            assert_eq!(llm.model.as_deref(), Some("qwen"));
            assert_eq!(llm.context_tokens, Some(65_536));
            assert!(matches!(llm.provider, Some(Provider::OpenaiCompatible)));
            save_in(path, r#"{"language": "en"}"#).unwrap();
            assert_eq!(config::get().review.language.as_deref(), Some("en"));
            assert!(std::fs::read_to_string(path).unwrap().contains("[review]\nlanguage = \"en\""));
            save_in(path, r#"{"language": "auto"}"#).unwrap();
            assert_eq!(config::get().review.language, None, "automático quita el campo");
        });
    }

    #[test]
    fn invalid_settings_are_rejected_without_touching_the_file() {
        with_config("invalid", "[llm]\nmodel = \"qwen\"\n", |path| {
            for body in [
                r#"{"base_url": "file:///etc/passwd"}"#,
                r#"{"provider": "otro"}"#,
                r#"{"context_tokens": 10}"#,
                r#"{"model": "a\nb"}"#,
                r#"{"api_key_file": "/tmp/x"}"#,
                r#"{"language": "klingon"}"#,
            ] {
                assert!(save_in(path, body).is_err(), "{body}");
            }
            assert_eq!(std::fs::read_to_string(path).unwrap(), "[llm]\nmodel = \"qwen\"\n");
        });
    }

    #[cfg(unix)]
    #[test]
    fn the_key_goes_to_a_private_file_never_to_config_toml() {
        use std::os::unix::fs::PermissionsExt;
        with_config("key", "[llm]\nmodel = \"qwen\"\n", |path| {
            let file = path.with_file_name("llm-key");
            config::update(path, |document| {
                config::table(document.as_table_mut(), "llm")["api_key_file"] =
                    toml_edit::value(file.display().to_string());
            })
            .unwrap();
            config::init_from(Some(path)).unwrap();
            save_key_in(path, r#"{"key": "sk-secreta-123"}"#).unwrap();
            assert!(!std::fs::read_to_string(path).unwrap().contains("sk-secreta"));
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(config::read_key_file(&file).unwrap(), "sk-secreta-123");
            assert!(save_key_in(path, r#"{"key": "con espacio"}"#).is_err());
            let shown = key_view(Provider::OpenaiCompatible).to_string();
            assert!(!shown.contains("sk-secreta"), "{shown}");
        });
    }
}
