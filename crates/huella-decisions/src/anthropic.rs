//! Adaptador para la API de Claude (Messages API con salida estructurada).

use std::time::Duration;

use serde_json::{Value, json};
use ureq::Agent;

use crate::{
    DecisionExtractor, ExtractError, ExtractedDecision, ExtractionRequest, SYSTEM_PROMPT, output_schema, parse_output,
    user_message,
};

pub const DEFAULT_MODEL: &str = "claude-haiku-5-5";
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const PROVIDER: &str = "anthropic";
const MAX_TOKENS: u32 = 16_000;

pub struct Anthropic {
    agent: Agent,
    api_key: String,
    model: String,
    base_url: String,
}

impl Anthropic {
    /// Credencial desde `ANTHROPIC_API_KEY`.
    ///
    /// # Errors
    /// Si la variable no está definida.
    pub fn from_env(model: Option<String>, base_url: Option<String>) -> Result<Self, ExtractError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .ok_or(ExtractError::MissingCredential("ANTHROPIC_API_KEY"))?;
        Ok(Self::new(api_key, model, base_url))
    }

    pub fn new(api_key: String, model: Option<String>, base_url: Option<String>) -> Self {
        let config =
            Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(300))).build();
        Self {
            agent: config.into(),
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()).trim_end_matches('/').to_owned(),
        }
    }
}

impl DecisionExtractor for Anthropic {
    fn id(&self) -> String {
        format!("{PROVIDER}:{}", self.model)
    }

    fn extract(&self, request: &ExtractionRequest) -> Result<Vec<ExtractedDecision>, ExtractError> {
        let network = |error: ureq::Error| ExtractError::Network { provider: PROVIDER, message: error.to_string() };
        let mut response = self
            .agent
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .send_json(request_body(&self.model, request))
            .map_err(network)?;
        let status = response.status().as_u16();
        let body = response.body_mut().read_to_string().map_err(network)?;
        if status != 200 {
            return Err(ExtractError::Api { provider: PROVIDER, status, body });
        }
        let value: Value = serde_json::from_str(&body).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
        parse_output(&response_text(&value)?)
    }
}

/// Cuerpo de la petición: salida estructurada con `output_config.format`.
fn request_body(model: &str, request: &ExtractionRequest) -> Value {
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "messages": [{"role": "user", "content": user_message(request)}],
        "output_config": {"format": {"type": "json_schema", "schema": output_schema()}},
    })
}

/// Texto JSON de la respuesta, tras comprobar por qué se detuvo el modelo.
fn response_text(response: &Value) -> Result<String, ExtractError> {
    match response["stop_reason"].as_str() {
        Some("refusal") => {
            let category = response["stop_details"]["category"].as_str().unwrap_or("sin categoría");
            return Err(ExtractError::Refused(category.to_owned()));
        }
        Some("max_tokens") => return Err(ExtractError::Truncated),
        _ => {}
    }
    response["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|block| block["type"] == "text"))
        .and_then(|block| block["text"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| ExtractError::BadResponse("sin bloque de texto".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uses_structured_output_and_the_chosen_model() {
        let request = ExtractionRequest { dialogue: "### Persona\nhola\n\n".into(), known_titles: vec![] };
        let body = request_body("claude-haiku-5-5", &request);
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["format"]["schema"], output_schema());
        assert!(body["messages"][0]["content"].as_str().unwrap().ends_with("hola\n\n"));
        assert!(body.get("thinking").is_none() && body.get("temperature").is_none());
    }

    #[test]
    fn reads_the_text_block_after_thinking() {
        let response = json!({
            "stop_reason": "end_turn",
            "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "{\"decisions\": []}"}]
        });
        assert_eq!(response_text(&response).unwrap(), "{\"decisions\": []}");
    }

    #[test]
    fn refusals_and_truncations_are_errors() {
        let refused = json!({"stop_reason": "refusal", "stop_details": {"category": "cyber"}, "content": []});
        assert!(matches!(response_text(&refused), Err(ExtractError::Refused(category)) if category == "cyber"));
        let cut = json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "{\"dec"}]});
        assert!(matches!(response_text(&cut), Err(ExtractError::Truncated)));
    }

    #[test]
    fn identifies_itself_by_provider_and_model() {
        let extractor = Anthropic::new("clave".into(), None, Some("http://localhost:9/".into()));
        assert_eq!(extractor.id(), "anthropic:claude-haiku-5-5");
        assert_eq!(extractor.base_url, "http://localhost:9");
    }
}
