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
    /// Activa o desactiva el razonamiento de los modelos que lo tienen (Qwen3…)
    /// con `chat_template_kwargs`; `None` no envía nada.
    thinking: Option<bool>,
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
            thinking: None,
        }
    }

    /// Pide al servidor que active o desactive el razonamiento del modelo. Lo
    /// entienden llama.cpp y vLLM; la API oficial de `OpenAI` lo rechaza.
    #[must_use]
    pub fn with_thinking(mut self, thinking: Option<bool>) -> Self {
        self.thinking = thinking;
        self
    }
}

impl JsonModel for OpenAiCompatible {
    fn model_id(&self) -> String {
        format!("{PROVIDER}:{}", self.model)
    }

    fn complete_json(&self, system: &str, user: &str, schema: &Value) -> Result<String, ExtractError> {
        // Un corte sin respuesta suele ser el proxy cerrando por inactividad
        // mientras el servidor carga el modelo: el segundo intento ya lo encuentra cargado.
        match self.attempt(system, user, schema) {
            Err(error) if is_cut(&error) => {
                std::thread::sleep(RETRY_AFTER);
                self.attempt(system, user, schema).map_err(explain)
            }
            other => other.map_err(explain),
        }
    }
}

/// Espera antes de reintentar una petición cortada.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// Un error de red que merece otro intento: la conexión se abrió y se cortó.
/// Si se rechazó (el servidor está apagado), reintentar no sirve.
fn is_cut(error: &ExtractError) -> bool {
    matches!(error, ExtractError::Network { message, .. } if !message.to_lowercase().contains("refused"))
}

/// Los errores de red de rustls o del sistema no dicen qué hacer: se explica.
fn explain(error: ExtractError) -> ExtractError {
    match error {
        ExtractError::Network { provider, message } if is_cut_message(&message) => ExtractError::Network {
            provider,
            message: format!(
                "el servidor cortó la conexión sin responder ({message}). Si el modelo se estaba cargando, vuelve a \
                 intentarlo; si pasa siempre, sube el tiempo de espera del proxy que tengas delante del servidor"
            ),
        },
        other => other,
    }
}

fn is_cut_message(message: &str) -> bool {
    let message = message.to_lowercase();
    ["close_notify", "closed", "reset", "eof", "timed out", "broken pipe"].iter().any(|hint| message.contains(hint))
}

impl OpenAiCompatible {
    fn attempt(&self, system: &str, user: &str, schema: &Value) -> Result<String, ExtractError> {
        let network = |error: ureq::Error| ExtractError::Network { provider: PROVIDER, message: error.to_string() };
        let mut call = self.agent.post(format!("{}/chat/completions", self.base_url));
        if let Some(key) = &self.api_key {
            call = call.header("Authorization", &format!("Bearer {key}"));
        }
        let body = request_body(&self.model, system, user, schema, self.thinking);
        let mut response = call.send_json(body).map_err(network)?;
        let status = response.status().as_u16();
        if status != 200 {
            let body = response.body_mut().read_to_string().map_err(network)?;
            return Err(ExtractError::Api { provider: PROVIDER, status, body });
        }
        collect_stream(BufReader::new(response.body_mut().as_reader()))
    }
}

fn request_body(model: &str, system: &str, user: &str, schema: &Value, thinking: Option<bool>) -> Value {
    let mut body = json!({
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
    });
    // El razonamiento se descarta y cuenta dentro de max_tokens: sin él, las
    // respuestas llegan antes y el JSON no se corta.
    if let Some(enabled) = thinking {
        body["chat_template_kwargs"] = json!({"enable_thinking": enabled});
    }
    body
}

/// Embeddings con la API de `OpenAI` (`POST /embeddings`): Ollama
/// (`nomic-embed-text`), llama.cpp con `--embedding`, `OpenAI`…
pub struct OpenAiEmbeddings {
    agent: Agent,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAiEmbeddings {
    pub fn new(base_url: Option<String>, model: String, api_key: Option<String>) -> Self {
        let config = Agent::config_builder().http_status_as_error(false).timeout_global(Some(MAX_SILENCE)).build();
        Self {
            agent: config.into(),
            base_url: base_url.unwrap_or_else(|| OLLAMA_BASE_URL.to_owned()).trim_end_matches('/').to_owned(),
            model,
            api_key: api_key.filter(|key| !key.trim().is_empty()),
        }
    }
}

/// Prefijos de tarea que esperan algunas familias de modelos de embeddings:
/// sin ellos, la calidad de la búsqueda cae bastante. (documento, consulta)
fn task_prefixes(model: &str) -> (&'static str, &'static str) {
    let model = model.to_lowercase();
    if model.contains("nomic-embed") {
        ("search_document: ", "search_query: ")
    } else if model.contains("e5") {
        ("passage: ", "query: ")
    } else {
        ("", "")
    }
}

impl OpenAiEmbeddings {
    fn request(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, ExtractError> {
        let network = |error: ureq::Error| ExtractError::Network { provider: PROVIDER, message: error.to_string() };
        let mut call = self.agent.post(format!("{}/embeddings", self.base_url));
        if let Some(key) = &self.api_key {
            call = call.header("Authorization", &format!("Bearer {key}"));
        }
        let mut response = call.send_json(json!({"model": self.model, "input": texts})).map_err(network)?;
        let status = response.status().as_u16();
        let body = response.body_mut().read_to_string().map_err(network)?;
        if status != 200 {
            return Err(ExtractError::Api { provider: PROVIDER, status, body });
        }
        parse_embeddings(&body, texts.len())
    }
}

impl crate::Embedder for OpenAiEmbeddings {
    fn model_id(&self) -> String {
        format!("{PROVIDER}:{}", self.model)
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, ExtractError> {
        let (document, _) = task_prefixes(&self.model);
        let prefixed: Vec<String> = texts.iter().map(|text| format!("{document}{text}")).collect();
        self.request(&prefixed.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, ExtractError> {
        let (_, query) = task_prefixes(&self.model);
        self.request(&[format!("{query}{text}").as_str()])?
            .pop()
            .ok_or_else(|| ExtractError::BadResponse("sin vector".into()))
    }
}

/// Vectores de la respuesta, ordenados por su `index`.
fn parse_embeddings(body: &str, expected: usize) -> Result<Vec<Vec<f32>>, ExtractError> {
    let bad = |message: String| ExtractError::BadResponse(message);
    let value: Value = serde_json::from_str(body).map_err(|error| bad(error.to_string()))?;
    let mut items: Vec<(u64, Vec<f32>)> = value["data"]
        .as_array()
        .ok_or_else(|| bad("falta `data`".into()))?
        .iter()
        .enumerate()
        .map(|(position, item)| {
            let index = item["index"].as_u64().unwrap_or(position as u64);
            #[allow(clippy::cast_possible_truncation)]
            let vector = item["embedding"]
                .as_array()
                .map(|values| values.iter().filter_map(Value::as_f64).map(|v| v as f32).collect());
            vector.map(|vector| (index, vector)).ok_or_else(|| bad("un elemento no trae `embedding`".into()))
        })
        .collect::<Result<_, _>>()?;
    if items.len() != expected {
        return Err(bad(format!("{} vectores para {expected} textos", items.len())));
    }
    items.sort_by_key(|(index, _)| *index);
    Ok(items.into_iter().map(|(_, vector)| vector).collect())
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
    fn models_trained_with_task_instructions_get_their_prefixes() {
        assert_eq!(task_prefixes("nomic-embed-text"), ("search_document: ", "search_query: "));
        assert_eq!(task_prefixes("intfloat/multilingual-e5-large"), ("passage: ", "query: "));
        assert_eq!(task_prefixes("text-embedding-3-small"), ("", ""));
    }

    #[test]
    fn embeddings_are_returned_in_input_order() {
        let body = r#"{"data": [{"index": 1, "embedding": [0.5, 1]}, {"index": 0, "embedding": [1, 0]}]}"#;
        assert_eq!(parse_embeddings(body, 2).unwrap(), vec![vec![1.0, 0.0], vec![0.5, 1.0]]);
        assert!(matches!(parse_embeddings(body, 3), Err(ExtractError::BadResponse(_))));
        assert!(matches!(parse_embeddings("{}", 1), Err(ExtractError::BadResponse(_))));
    }

    #[test]
    fn request_streams_with_json_schema_response_format() {
        let body = request_body("qwen2.5:7b", "instrucciones", "x", &output_schema(), None);
        assert_eq!(body["stream"], true);
        assert!(body.get("chat_template_kwargs").is_none(), "sin la opción no se envía nada");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["response_format"]["json_schema"]["schema"], output_schema());
    }

    #[test]
    fn cut_connections_are_retried_and_explained_but_refusals_are_not() {
        let cut = ExtractError::Network {
            provider: PROVIDER,
            message: "io: peer closed connection without sending TLS close_notify".into(),
        };
        assert!(is_cut(&cut));
        assert!(explain(cut).to_string().contains("tiempo de espera del proxy"));
        let refused = ExtractError::Network { provider: PROVIDER, message: "Connection refused (os error 61)".into() };
        assert!(!is_cut(&refused));
        assert!(!explain(refused).to_string().contains("proxy"));
        assert!(!is_cut(&ExtractError::Truncated));
    }

    #[test]
    fn thinking_is_sent_only_when_configured() {
        let off = request_body("qwen3", "s", "u", &output_schema(), Some(false));
        assert_eq!(off["chat_template_kwargs"]["enable_thinking"], false);
        let on = request_body("qwen3", "s", "u", &output_schema(), Some(true));
        assert_eq!(on["chat_template_kwargs"]["enable_thinking"], true);
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
