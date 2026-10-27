//! Extracción de decisiones de proyecto a partir del diálogo de una sesión.
//!
//! El conocimiento que no toca una línea de código ("Rust en vez de Python")
//! solo está en la conversación. Un modelo lo extrae a un esquema fijo; el
//! modelo concreto es intercambiable detrás de [`DecisionExtractor`]:
//!
//! - [`anthropic::Anthropic`]: API de Claude.
//! - [`openai::OpenAiCompatible`]: cualquier servidor con la API de chat de
//!   `OpenAI` (Ollama, LM Studio, llama.cpp, vLLM, `OpenRouter`…).

pub mod anthropic;
pub mod openai;

use std::fmt::Write as _;

use huella_core::{Conversation, Speaker};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Tamaño máximo de cada trozo de diálogo enviado al modelo.
pub const DEFAULT_CHUNK_CHARS: usize = 120_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    /// La persona la confirmó o se llevó a cabo.
    Accepted,
    /// Se planteó pero no se confirmó.
    Proposed,
    /// Se descartó de forma explícita.
    Rejected,
    /// Otra decisión posterior la reemplazó.
    Superseded,
}

impl DecisionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Proposed => "proposed",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
        }
    }
}

/// Decisión tal como la devuelve el modelo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedDecision {
    pub title: String,
    pub decision: String,
    pub rationale: String,
    /// Alternativas descartadas, cada una con su motivo.
    pub alternatives: Vec<String>,
    /// Tecnologías o conceptos implicados, en minúsculas (`rust`, `sqlite`).
    pub entities: Vec<String>,
    pub status: DecisionStatus,
    /// Título de una decisión anterior que esta reemplaza; vacío si ninguna.
    pub supersedes: String,
}

#[derive(Debug, Deserialize)]
struct ExtractionOutput {
    decisions: Vec<ExtractedDecision>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("falta la credencial: define {0}")]
    MissingCredential(&'static str),
    #[error("error de red con {provider}: {message}")]
    Network { provider: &'static str, message: String },
    #[error("{provider} respondió {status}: {body}")]
    Api { provider: &'static str, status: u16, body: String },
    #[error("el modelo rechazó la petición ({0})")]
    Refused(String),
    #[error("la respuesta se cortó por el límite de tokens")]
    Truncated,
    #[error("respuesta con un formato inesperado: {0}")]
    BadResponse(String),
}

/// Un modelo que responde con JSON ajustado a un esquema. Los adaptadores
/// solo implementan esto; la extracción de decisiones y la narración de
/// revisiones se construyen encima.
pub trait JsonModel {
    /// Identificador estable (`anthropic:claude-haiku-5-5`).
    fn model_id(&self) -> String;

    /// Texto JSON de la respuesta a `user` con las instrucciones `system`.
    ///
    /// # Errors
    /// Si falla la llamada, el modelo la rechaza o la respuesta se corta.
    fn complete_json(&self, system: &str, user: &str, schema: &Value) -> Result<String, ExtractError>;
}

impl<M: JsonModel + ?Sized> DecisionExtractor for M {
    fn id(&self) -> String {
        self.model_id()
    }

    fn extract(&self, request: &ExtractionRequest) -> Result<Vec<ExtractedDecision>, ExtractError> {
        parse_output(&self.complete_json(SYSTEM_PROMPT, &user_message(request), &output_schema())?)
    }
}

/// Un modelo capaz de extraer decisiones de un trozo de diálogo.
pub trait DecisionExtractor {
    /// Identificador estable (`anthropic:claude-haiku-5-5`) para saber con qué
    /// se extrajo cada decisión.
    fn id(&self) -> String;

    /// # Errors
    /// Si falla la llamada al modelo o su respuesta no sigue el esquema.
    fn extract(&self, request: &ExtractionRequest) -> Result<Vec<ExtractedDecision>, ExtractError>;
}

/// Lo que se envía al modelo: un trozo de diálogo y las decisiones ya
/// extraídas de trozos anteriores de la misma sesión, para poder reemplazarlas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionRequest {
    pub dialogue: String,
    pub known_titles: Vec<String>,
}

pub const SYSTEM_PROMPT: &str = "\
Extraes decisiones técnicas de conversaciones entre una persona desarrolladora y un agente de IA de programación.

Una decisión es una elección sobre el proyecto que alguien querrá recordar más adelante: tecnologías, \
arquitectura, diseño, modelo de datos, alcance, convenciones, o descartar una opción. No son decisiones \
los pasos de implementación (\"ahora escribo el test\"), las correcciones puntuales de errores, ni las \
opiniones sin elección.

Para cada decisión:
- title: frase corta y específica (\"Rust como lenguaje del producto\").
- decision: qué se decidió, en una o dos frases.
- rationale: por qué, con los argumentos que se dieron en la conversación.
- alternatives: opciones descartadas, cada una con su motivo. Lista vacía si no hubo.
- entities: tecnologías, librerías, servicios o conceptos implicados, en minúsculas (\"rust\", \"sqlite\", \"lsp\").
- status: accepted si la persona la confirmó o se llevó a cabo; proposed si solo se planteó; \
rejected si se descartó; superseded si otra decisión posterior de la conversación la reemplaza.
- supersedes: título exacto de la decisión anterior que esta reemplaza (de esta conversación o de la \
lista de decisiones ya extraídas), o cadena vacía.

Escribe en el idioma de la conversación. Usa solo lo que dice la conversación; no inventes motivos. \
Si no hay decisiones, devuelve una lista vacía.";

/// Mensaje de usuario con el diálogo y las decisiones ya conocidas.
pub fn user_message(request: &ExtractionRequest) -> String {
    let mut message = String::new();
    if !request.known_titles.is_empty() {
        message.push_str("Decisiones ya extraídas de partes anteriores de esta sesión:\n");
        for title in &request.known_titles {
            let _ = writeln!(message, "- {title}");
        }
        message.push('\n');
    }
    message.push_str("Conversación:\n\n");
    message.push_str(&request.dialogue);
    message
}

/// Esquema JSON de la salida. Todo obligatorio y sin propiedades extra, como
/// exige la salida estructurada estricta de los proveedores.
pub fn output_schema() -> Value {
    let strings = json!({"type": "array", "items": {"type": "string"}});
    json!({
        "type": "object",
        "properties": {
            "decisions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "decision": {"type": "string"},
                        "rationale": {"type": "string"},
                        "alternatives": strings,
                        "entities": strings,
                        "status": {"type": "string", "enum": ["accepted", "proposed", "rejected", "superseded"]},
                        "supersedes": {"type": "string"}
                    },
                    "required": ["title", "decision", "rationale", "alternatives", "entities", "status", "supersedes"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["decisions"],
        "additionalProperties": false
    })
}

/// Interpreta el JSON que devuelve el modelo y normaliza las entidades.
///
/// # Errors
/// Si el texto no sigue el esquema.
pub fn parse_output(text: &str) -> Result<Vec<ExtractedDecision>, ExtractError> {
    let output: ExtractionOutput =
        serde_json::from_str(text).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
    Ok(output
        .decisions
        .into_iter()
        .map(|mut decision| {
            decision.entities =
                decision.entities.iter().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()).collect();
            decision.entities.sort();
            decision.entities.dedup();
            decision.supersedes = decision.supersedes.trim().to_owned();
            decision
        })
        .collect())
}

/// Trocea el diálogo en bloques de como mucho `max_chars`, sin partir un
/// mensaje salvo que él solo supere el límite.
pub fn chunk_dialogue(conversation: &Conversation, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for turn in &conversation.turns {
        let speaker = match turn.speaker {
            Speaker::Person => "Persona",
            Speaker::Agent => "Agente",
        };
        let block = format!("### {speaker}\n{}\n\n", turn.text);
        for piece in split_chars(&block, max_chars) {
            if !current.is_empty() && current.len() + piece.len() > max_chars {
                chunks.push(std::mem::take(&mut current));
            }
            current.push_str(piece);
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Parte un texto en trozos de como mucho `max` bytes, respetando caracteres.
fn split_chars(text: &str, max: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.len() > max {
        let mut cut = max;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (piece, tail) = rest.split_at(cut);
        pieces.push(piece);
        rest = tail;
    }
    pieces.push(rest);
    pieces
}

#[cfg(test)]
mod tests {
    use huella_core::Turn;

    use super::*;

    fn conversation(texts: &[(Speaker, &str)]) -> Conversation {
        Conversation {
            session_id: "s".into(),
            cwd: None,
            turns: texts
                .iter()
                .map(|(speaker, text)| Turn { speaker: *speaker, text: (*text).into(), timestamp: None })
                .collect(),
        }
    }

    #[test]
    fn dialogue_is_chunked_by_turns() {
        let chat =
            conversation(&[(Speaker::Person, "¿Rust o Python?"), (Speaker::Agent, "Rust, por el binario único.")]);
        assert_eq!(
            chunk_dialogue(&chat, 1000),
            vec!["### Persona\n¿Rust o Python?\n\n### Agente\nRust, por el binario único.\n\n"]
        );
        let chunks = chunk_dialogue(&chat, 45);
        assert_eq!(chunks.len(), 2, "cada mensaje cabe solo en un trozo, pero no juntos");
        assert!(chunks.iter().all(|chunk| chunk.len() <= 45));
    }

    #[test]
    fn a_huge_turn_is_split_on_char_boundaries() {
        let long = "ñ".repeat(100);
        let chunks = chunk_dialogue(&conversation(&[(Speaker::Agent, &long)]), 51);
        assert!(chunks.len() > 3);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 51));
        assert_eq!(chunks.concat(), format!("### Agente\n{long}\n\n"));
    }

    #[test]
    fn output_is_parsed_and_entities_normalized() {
        let text = r#"{"decisions": [{"title": "Rust", "decision": "Todo en Rust", "rationale": "Binario único",
            "alternatives": ["Python: distribución frágil"], "entities": [" Rust", "rust", "SQLite"],
            "status": "accepted", "supersedes": " TypeScript "}]}"#;
        let decisions = parse_output(text).unwrap();
        assert_eq!(decisions[0].entities, vec!["rust", "sqlite"]);
        assert_eq!(decisions[0].supersedes, "TypeScript");
        assert_eq!(decisions[0].status, DecisionStatus::Accepted);
        assert!(matches!(parse_output("{\"decisions\": [{\"title\": 1}]}"), Err(ExtractError::BadResponse(_))));
    }

    #[test]
    fn user_message_lists_known_decisions_first() {
        let request = ExtractionRequest { dialogue: "### Persona\nhola\n\n".into(), known_titles: vec!["Rust".into()] };
        assert_eq!(
            user_message(&request),
            "Decisiones ya extraídas de partes anteriores de esta sesión:\n- Rust\n\nConversación:\n\n### Persona\nhola\n\n"
        );
    }

    #[test]
    fn schema_requires_every_field_for_strict_mode() {
        let schema = output_schema();
        let item = &schema["properties"]["decisions"]["items"];
        let properties: Vec<&String> = item["properties"].as_object().unwrap().keys().collect();
        let required: Vec<&str> = item["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(properties.len(), required.len());
        assert_eq!(item["additionalProperties"], false);
    }
}
