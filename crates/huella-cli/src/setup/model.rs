//! Comprobar el modelo de lenguaje y el de embeddings, y guardarlos en
//! `config.toml`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use huella_decisions::Embedder;
use serde_json::json;

use super::{Check, Status};
use crate::{ModelArgs, Provider};

pub fn llm_check(model: &ModelArgs, online: bool) -> Check {
    let label = crate::ui::extractor_label(model);
    if let Some(problem) = crate::model_problem(model) {
        return Check::new("Modelo", Status::Warn, problem)
            .hint("huella setup lo configura; sin él no hay decisiones ni preguntas a la sesión");
    }
    if !online {
        return Check::new("Modelo", Status::Ok, label);
    }
    match test_llm(model) {
        Ok(()) => Check::new("Modelo", Status::Ok, format!("{label} · responde")),
        Err(error) => Check::new("Modelo", Status::Fail, format!("{label}: {error:#}")),
    }
}

pub fn embed_check(online: bool) -> Check {
    let Some(embedder) = crate::embedder(None, None) else {
        return Check::new("Embeddings", Status::Warn, "sin configurar: la búsqueda es solo por palabras")
            .hint("opcional; huella setup lo configura (p. ej. bge-m3 con Ollama)");
    };
    let label = embedder.model_id();
    if !online {
        return Check::new("Embeddings", Status::Ok, label);
    }
    match embedder.embed(&["prueba"]) {
        Ok(vectors) if vectors.first().is_some_and(|v| !v.is_empty()) => {
            Check::new("Embeddings", Status::Ok, format!("{label} · {} dimensiones", vectors[0].len()))
        }
        Ok(_) => Check::new("Embeddings", Status::Fail, format!("{label}: respondió sin vectores")),
        Err(error) => Check::new("Embeddings", Status::Fail, format!("{label}: {error}")),
    }
}

/// Una llamada mínima: que el modelo exista, la clave valga y sepa responder JSON.
pub fn test_llm(model: &ModelArgs) -> Result<()> {
    let llm = crate::build_model(model)?;
    let schema = json!({
        "type": "object",
        "properties": {"ok": {"type": "boolean"}},
        "required": ["ok"],
        "additionalProperties": false,
    });
    let text = llm.complete_json("Responde solo con el JSON pedido.", "Responde {\"ok\": true}.", &schema)?;
    let value: serde_json::Value = serde_json::from_str(&text).context("la respuesta no es JSON")?;
    if value.get("ok").is_none() {
        bail!("respondió otra cosa: {text}");
    }
    Ok(())
}

/// Modelos descargados en un Ollama local; vacío si no está en marcha.
pub fn ollama_models(ollama: &str) -> Vec<String> {
    let Ok(mut response) = ureq::get(format!("{ollama}/api/tags")).call() else {
        return Vec::new();
    };
    let Ok(body) = response.body_mut().read_json::<serde_json::Value>() else {
        return Vec::new();
    };
    body["models"]
        .as_array()
        .map(|models| models.iter().filter_map(|m| m["name"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// Lo que se guarda del modelo de lenguaje; la clave, aparte.
pub struct LlmChoice {
    pub provider: Provider,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub key_file: Option<std::path::PathBuf>,
}

pub fn save_llm(config: &Path, choice: &LlmChoice) -> Result<()> {
    crate::config::update(config, |document| {
        let llm = crate::config::table(document.as_table_mut(), "llm");
        llm["provider"] = toml_edit::value(match choice.provider {
            Provider::Anthropic => "anthropic",
            Provider::OpenaiCompatible => "openai-compatible",
        });
        set_or_remove(llm, "base_url", choice.base_url.as_deref());
        set_or_remove(llm, "model", choice.model.as_deref());
        if let Some(path) = &choice.key_file {
            llm["api_key_file"] = toml_edit::value(path.to_string_lossy().as_ref());
        }
    })
}

pub fn save_embed(config: &Path, model: &str, base_url: Option<&str>) -> Result<()> {
    crate::config::update(config, |document| {
        let embed = crate::config::table(document.as_table_mut(), "embed");
        embed["model"] = toml_edit::value(model);
        set_or_remove(embed, "base_url", base_url);
    })
}

fn set_or_remove(table: &mut toml_edit::Table, key: &str, value: Option<&str>) {
    match value {
        Some(value) => table[key] = toml_edit::value(value),
        None => {
            table.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_a_choice_replaces_the_previous_one() {
        let dir = std::env::temp_dir().join(format!("huella-model-{}", std::process::id()));
        let path = dir.join("config.toml");
        save_llm(
            &path,
            &LlmChoice {
                provider: Provider::OpenaiCompatible,
                base_url: Some("http://localhost:11434/v1".to_owned()),
                model: Some("qwen2.5:7b".to_owned()),
                key_file: None,
            },
        )
        .unwrap();
        save_llm(&path, &LlmChoice { provider: Provider::Anthropic, base_url: None, model: None, key_file: None })
            .unwrap();
        save_embed(&path, "bge-m3", Some("http://localhost:11434/v1")).unwrap();
        let config = crate::config::load(&path).unwrap();
        assert!(matches!(config.llm.provider, Some(Provider::Anthropic)));
        assert_eq!(config.llm.base_url, None, "no queda la URL de Ollama con Claude");
        assert_eq!(config.embed.model.as_deref(), Some("bge-m3"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
