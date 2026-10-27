//! Adaptador para servidores con la API de chat de `OpenAI`: Ollama
//! (`http://localhost:11434/v1`), LM Studio, llama.cpp, vLLM, `OpenRouter`…
//!
//! La respuesta se pide en streaming (SSE): un modelo local puede tardar
//! minutos en un trozo largo, y los proxies cortan las conexiones que pasan
//! un rato sin enviar datos.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{Value, json};
use ureq::Agent;

use crate::{ExtractError, JsonModel};

pub const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
const PROVIDER: &str = "openai-compatible";
/// Margen de salida; sin él algunos servidores aplican un límite corto por defecto.
const MAX_TOKENS: u32 = 8192;
/// Silencio máximo entre dos fragmentos (incluye procesar el prompt).
const MAX_SILENCE: Duration = Duration::from_secs(600);

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
            .timeout_global(None)
            .timeout_per_read(Some(MAX_SILENCE))
            .build();
        Self {
            agent: config.into(),
            base_url: base_url.unwrap_or_else(|| OLLAMA_BASE_URL.to_owned()).trim_end_matches('/').to_owned(),
            model,
            api_key: api_key.filter(|key| !key.trim().is_empty()),
        }
    }
}

impl JsonModel for OpenAiCompatible {
    fn model_id(&self) -> String {
        format!("{PROVIDER}:{}", self.model)
    }

    fn complete_json(&self, system: &str, user: &str, schema: &Value) -> Result<String, ExtractError> {
        let network = |error: ureq::Error| ExtractError::Network { provider: PROVIDER, message: error.to_string() };
        let mut call = self.agent.post(format!("{}/chat/completions", self.base_url));
        if let Some(key) = &self.api_key {
            call = call.header("Authorization", &format!("Bearer {key}"));
        }
        let mut response = call.send_json(request_body(&self.model, system, user, schema)).map_err(network)?;
        let status = response.status().as_u16();
        if status != 200 {
            let body = response.body_mut().read_to_string().map_err(network)?;
            return Err(ExtractError::Api { provider: PROVIDER, status, body });
        }
        collect_stream(BufReader::new(response.body_mut().as_reader()))
    }
}

fn request_body(model: &str, system: &str, user: &str, schema: &Value) -> Value {
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": "respuesta", "strict": true, "schema": schema},
        },
    })
}

/// Junta el texto de los eventos SSE (`data: {...}`) hasta `data: [DONE]`.
/// El razonamiento (`reasoning_content`) se descarta: solo mantiene viva la
/// conexión.
fn collect_stream(reader: impl BufRead) -> Result<String, ExtractError> {
    let network = |error: std::io::Error| ExtractError::Network { provider: PROVIDER, message: error.to_string() };
    let mut text = String::new();
    for line in reader.lines() {
        let line = line.map_err(network)?;
        let Some(payload) = line.strip_prefix("data:").map(str::trim) else { continue };
        if payload == "[DONE]" {
            break;
        }
        let event: Value =
            serde_json::from_str(payload).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
        if let Some(error) = event.get("error") {
            return Err(ExtractError::BadResponse(error.to_string()));
        }
        let choice = &event["choices"][0];
        if let Some(fragment) = choice["delta"]["content"].as_str() {
            text.push_str(fragment);
        }
        match choice["finish_reason"].as_str() {
            Some("length") => return Err(ExtractError::Truncated),
            Some("content_filter") => return Err(ExtractError::Refused("content_filter".into())),
            _ => {}
        }
    }
    if text.trim().is_empty() {
        return Err(ExtractError::BadResponse("la respuesta llegó vacía".into()));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output_schema;

    #[test]
    fn request_streams_with_json_schema_response_format() {
        let body = request_body("qwen2.5:7b", "instrucciones", "x", &output_schema());
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["response_format"]["json_schema"]["schema"], output_schema());
    }

    #[test]
    fn joins_streamed_content_and_ignores_reasoning() {
        let stream = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"pienso\"}}]}\n\n\
                      data: {\"choices\":[{\"delta\":{\"content\":\"{\\\"decisions\\\"\"}}]}\n\n\
                      : comentario de keep-alive\n\
                      data: {\"choices\":[{\"delta\":{\"content\":\": []}\"},\"finish_reason\":\"stop\"}]}\n\n\
                      data: [DONE]\n";
        assert_eq!(collect_stream(stream.as_bytes()).unwrap(), "{\"decisions\": []}");
    }

    #[test]
    fn truncation_errors_and_empty_streams_are_reported() {
        let cut = "data: {\"choices\":[{\"delta\":{\"content\":\"{\"},\"finish_reason\":\"length\"}]}\n";
        assert!(matches!(collect_stream(cut.as_bytes()), Err(ExtractError::Truncated)));
        let failed = "data: {\"error\":{\"message\":\"context too long\"}}\n";
        assert!(matches!(collect_stream(failed.as_bytes()), Err(ExtractError::BadResponse(_))));
        assert!(matches!(collect_stream("data: [DONE]\n".as_bytes()), Err(ExtractError::BadResponse(_))));
    }

    #[test]
    fn defaults_to_local_ollama_without_key() {
        let extractor = OpenAiCompatible::new(None, "qwen2.5:7b".into(), Some(" ".into()));
        assert_eq!(extractor.base_url, OLLAMA_BASE_URL);
        assert_eq!(extractor.api_key, None);
        assert_eq!(extractor.model_id(), "openai-compatible:qwen2.5:7b");
    }
}
