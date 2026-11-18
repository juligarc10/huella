//! El idioma en que el modelo explica: guías, guion, respuestas, cobertura y
//! decisiones. Sin fijarlo, cada instrucción pide escribir en el idioma de la
//! petición o de la pregunta; fijado (`[review] language`), manda sobre eso.
//!
//! Se aplica en un solo sitio: el modelo se envuelve al construirlo y añade la
//! regla al final de cada instrucción, así no hay que tocar cada prompt. Igual
//! va el nivel de detalle (`[review] detail`), que una petición puede cambiar.

use huella_decisions::{ExtractError, JsonModel};
use huella_engine::detail::Detail;
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

/// El nivel de detalle fijado en `config.toml`; sin fijar (o desconocido), el de por defecto.
pub(crate) fn detail() -> Detail {
    config::get().review.detail.as_deref().and_then(Detail::parse).unwrap_or_default()
}

/// El nivel de detalle releyendo antes `config.toml`, para el servidor MCP: vive
/// lo que dura la sesión del agente y un cambio en Ajustes debe llegarle ya. Si
/// el fichero está mal escrito, sigue el último que se leyó bien.
pub(crate) fn current_detail() -> Detail {
    #[cfg(not(test))]
    let _ = config::init();
    detail()
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

/// Lo que se añade a las instrucciones del agente si se pide código que se lea
/// de arriba abajo (`[review] narrative_code`): así el recorrido guiado sale
/// natural, porque el código ya cuenta la historia en orden.
pub(crate) fn narrative_rule() -> Option<&'static str> {
    config::get().review.narrative_code.unwrap_or(false).then_some(
        " Escribe el código para que se lea de arriba abajo como una explicación: el método público cuenta los \
         pasos con nombres del dominio, los detalles van debajo en el orden en que se llaman, y solo hay un \
         comentario donde se decide algo que no es obvio.",
    )
}

/// Lo que se añade a las instrucciones del agente si se le pide contar la
/// lectura guiada al terminar (`[review] agent_walkthrough`).
pub(crate) fn walkthrough_rule() -> Option<&'static str> {
    config::get().review.agent_walkthrough.unwrap_or(false).then_some(
        " Al terminar una tarea, después de huella_storyline, cuenta también el caso principal con huella_walkthrough: \
         un caso concreto seguido por el código en el orden en que se ejecuta, con los valores reales de los tests \
         que ejecutaste marcados con from_test, y corrige lo que te responda que no cuadra.",
    )
}

/// Un modelo que escribe en un idioma fijo y con un nivel de detalle.
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

/// `model`, escribiendo en el idioma fijado (si lo hay) y con el nivel de
/// detalle de la petición o, sin él, el de la configuración.
pub(crate) fn wrap(model: Box<dyn JsonModel>, detail: Option<Detail>) -> Box<dyn JsonModel> {
    let mut rule = detail.unwrap_or_else(self::detail).model_rule();
    if let Some(name) = configured() {
        rule.push_str(&self::rule(name));
    }
    Box::new(InLanguage { inner: model, rule })
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
        system_asking(config_text, None)
    }

    fn system_asking(config_text: &str, detail: Option<Detail>) -> String {
        let _guard = config::TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("huella-language-{}-{}", std::process::id(), config_text.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, config_text).unwrap();
        config::init_from(Some(&path)).unwrap();
        let seen = Arc::new(Mutex::new(String::new()));
        let model = wrap(Box::new(Sees(Arc::clone(&seen))), detail);
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
    fn without_a_language_only_the_detail_level_is_added() {
        let plain = "Explica el cambio. Escribe en el idioma de la petición.";
        let first_time = format!("{plain}{}", Detail::FirstTime.model_rule());
        assert_eq!(system_with("[llm]\n"), first_time, "por defecto se pide explícitamente la primera vez");
        assert_eq!(system_with("[review]\nlanguage = \"xx\"\n"), first_time);
    }

    #[test]
    fn the_detail_level_comes_from_the_config_unless_the_request_asks_another() {
        let code = system_with("[review]\ndetail = \"code\"\n");
        assert!(code.contains(Detail::Code.audience()), "{code}");
        let asked = system_asking("[review]\ndetail = \"code\"\nlanguage = \"en\"\n", Some(Detail::FirstTime));
        assert!(asked.contains(Detail::FirstTime.audience()) && !asked.contains(Detail::Code.audience()), "{asked}");
        assert!(asked.ends_with("citas literales."), "el idioma sigue al final: {asked}");
    }
}
