//! El idioma en que el modelo explica: guías, guion, respuestas, cobertura y
//! decisiones. Sin fijarlo, cada instrucción pide escribir en el idioma de la
//! petición o de la pregunta; fijado (`[review] language`), manda sobre eso.
//!
//! Se aplica en un solo sitio: el modelo se envuelve al construirlo y añade la
//! regla al final de cada instrucción, así no hay que tocar cada prompt.

use huella_decisions::{ExtractError, JsonModel};
use serde_json::Value;

use crate::config;

/// Código, cómo se ve en Ajustes y cómo se nombra en la instrucción al modelo.
pub(crate) const LANGUAGES: &[(&str, &str, &str)] = &[
    ("es", "Español", "español"),
    ("en", "English", "inglés"),
    ("pt", "Português", "portugués"),
    ("fr", "Français", "francés"),
    ("de", "Deutsch", "alemán"),
    ("it", "Italiano", "italiano"),
    ("ca", "Català", "catalán"),
    ("gl", "Galego", "gallego"),
    ("eu", "Euskara", "euskera"),
];

/// El nombre del idioma fijado en `config.toml`, si lo hay y se conoce.
pub(crate) fn configured() -> Option<&'static str> {
    let config = config::get();
    let code = config.review.language.as_deref()?;
    LANGUAGES.iter().find(|(known, _, _)| *known == code).map(|(_, _, name)| *name)
}

fn rule(name: &str) -> String {
    format!(
        "\n\nIdioma: escribe todo el texto que generes (explicaciones, títulos, resúmenes y respuestas) en {name}, \
         aunque estas instrucciones, el código o la petición estén en otro idioma. Esta regla manda sobre cualquier \
         otra sobre el idioma. No traduzcas nombres del código, rutas, órdenes ni citas literales."
    )
}

/// Lo que se añade a las instrucciones del servidor MCP para el agente que registra.
pub(crate) fn agent_rule() -> Option<String> {
    configured().map(|name| {
        format!(
            " Escribe lo que registres con huella_record y cuentes con huella_storyline en {name}: es el idioma en \
             que lo leerá quien revise."
        )
    })
}

/// Un modelo que escribe en un idioma fijo.
struct InLanguage {
    inner: Box<dyn JsonModel>,
    rule: String,
}

impl JsonModel for InLanguage {
    fn model_id(&self) -> String {
        self.inner.model_id()
    }

    fn complete_json(&self, system: &str, user: &str, schema: &Value) -> Result<String, ExtractError> {
        self.inner.complete_json(&format!("{system}{}", self.rule), user, schema)
    }
}

/// `model`, escribiendo en el idioma fijado; sin idioma fijado, tal cual.
pub(crate) fn wrap(model: Box<dyn JsonModel>) -> Box<dyn JsonModel> {
    match configured() {
        Some(name) => Box::new(InLanguage { inner: model, rule: rule(name) }),
        None => model,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;

    struct Sees(Arc<Mutex<String>>);

    impl JsonModel for Sees {
        fn model_id(&self) -> String {
            "fake:1".into()
        }
        fn complete_json(&self, system: &str, _: &str, _: &Value) -> Result<String, ExtractError> {
            system.clone_into(&mut self.0.lock().unwrap());
            Ok("{}".into())
        }
    }

    fn system_with(config_text: &str) -> String {
        let _guard = config::TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("huella-language-{}-{}", std::process::id(), config_text.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, config_text).unwrap();
        config::init_from(Some(&path)).unwrap();
        let seen = Arc::new(Mutex::new(String::new()));
        let model = wrap(Box::new(Sees(Arc::clone(&seen))));
        model.complete_json("Explica el cambio. Escribe en el idioma de la petición.", "u", &Value::Null).unwrap();
        assert_eq!(model.model_id(), "fake:1", "el idioma no cambia el modelo");
        config::init_from(None).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        seen.lock().unwrap().clone()
    }

    #[test]
    fn a_fixed_language_is_added_to_every_instruction_and_wins_over_the_default() {
        let system = system_with("[review]\nlanguage = \"en\"\n");
        assert!(system.starts_with("Explica el cambio."), "{system}");
        assert!(system.ends_with("citas literales."), "la regla va al final: {system}");
        assert!(system.contains("en inglés") && system.contains("manda sobre cualquier otra"), "{system}");
    }

    #[test]
    fn without_a_language_nothing_changes() {
        assert_eq!(system_with("[llm]\n"), "Explica el cambio. Escribe en el idioma de la petición.");
        assert_eq!(
            system_with("[review]\nlanguage = \"xx\"\n"),
            "Explica el cambio. Escribe en el idioma de la petición."
        );
    }
}
