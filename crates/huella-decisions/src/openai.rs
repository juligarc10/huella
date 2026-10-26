//! Adaptador para servidores con la API de chat de `OpenAI`: Ollama
//! (`http://localhost:11434/v1`), LM Studio, llama.cpp, vLLM, `OpenRouter`…

use std::time::Duration;

use serde_json::{Value, json};
use ureq::Agent;

use crate::{
    DecisionExtractor, ExtractError, ExtractedDecision, ExtractionRequest, SYSTEM_PROMPT, output_schema, parse_output,
    user_message,
};

pub const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
const PROVIDER: &str = "openai-compatible";
/// Margen de salida; sin él algunos servidores aplican un límite corto por defecto.
const MAX_TOKENS: u32 = 8192;

pub struct OpenAiCompatible {
    agent: Agent,
    base_url: String,
    model: String,
    /// Los servidores locales no suelen pedirla.
    api_key: Option<String>,
}

impl OpenAiCompatible {
    pub fn new(base_url: Option<String>, model: String, api_key: Option<String>) -> Self {
        let config = Agent::config_builder()
            .http_status_as_error(false)
            // Los modelos locales pueden tardar minutos en un trozo largo.
            .timeout_global(Some(Duration::from_secs(900)))
            .build();
        Self {
            agent: config.into(),
            base_url: base_url.unwrap_or_else(|| OLLAMA_BASE_URL.to_owned()).trim_end_matches('/').to_owned(),
            model,
            api_key: api_key.filter(|key| !key.trim().is_empty()),
        }
    }
}

impl DecisionExtractor for OpenAiCompatible {
    fn id(&self) -> String {
        format!("{PROVIDER}:{}", self.model)
    }

    fn extract(&self, request: &ExtractionRequest) -> Result<Vec<ExtractedDecision>, ExtractError> {
        let network = |error: ureq::Error| ExtractError::Network { provider: PROVIDER, message: error.to_string() };
        let mut call = self.agent.post(format!("{}/chat/completions", self.base_url));
        if let Some(key) = &self.api_key {
            call = call.header("Authorization", &format!("Bearer {key}"));
        }
        let mut response = call.send_json(request_body(&self.model, request)).map_err(network)?;
        let status = response.status().as_u16();
        let body = response.body_mut().read_to_string().map_err(network)?;
        if status != 200 {
            return Err(ExtractError::Api { provider: PROVIDER, status, body });
        }
        let value: Value = serde_json::from_str(&body).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
        parse_output(&response_text(&value)?)
    }
}

fn request_body(model: &str, request: &ExtractionRequest) -> Value {
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_message(request)},
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": "decisions", "strict": true, "schema": output_schema()},
        },
    })
}

fn response_text(response: &Value) -> Result<String, ExtractError> {
    let choice = &response["choices"][0];
    match choice["finish_reason"].as_str() {
        Some("length") => return Err(ExtractError::Truncated),
        Some("content_filter") => return Err(ExtractError::Refused("content_filter".into())),
        _ => {}
    }
    choice["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| ExtractError::BadResponse("sin contenido en la respuesta".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uses_json_schema_response_format() {
        let request = ExtractionRequest { dialogue: "x".into(), known_titles: vec![] };
        let body = request_body("qwen2.5:7b", &request);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["response_format"]["json_schema"]["schema"], output_schema());
    }

    #[test]
    fn reads_the_message_content_and_detects_truncation() {
        let ok = json!({"choices": [{"finish_reason": "stop", "message": {"content": "{\"decisions\": []}"}}]});
        assert_eq!(response_text(&ok).unwrap(), "{\"decisions\": []}");
        let cut = json!({"choices": [{"finish_reason": "length", "message": {"content": "{"}}]});
        assert!(matches!(response_text(&cut), Err(ExtractError::Truncated)));
    }

    #[test]
    fn defaults_to_local_ollama_without_key() {
        let extractor = OpenAiCompatible::new(None, "qwen2.5:7b".into(), Some(" ".into()));
        assert_eq!(extractor.base_url, OLLAMA_BASE_URL);
        assert_eq!(extractor.api_key, None);
        assert_eq!(extractor.id(), "openai-compatible:qwen2.5:7b");
    }
}
