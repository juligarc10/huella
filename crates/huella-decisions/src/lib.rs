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

/// A qué se aplica una decisión.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionScope {
    /// Preferencia o convención de la persona, válida en cualquier proyecto.
    User,
    /// El proyecto entero.
    #[default]
    Project,
    /// Una carpeta o un fichero del proyecto.
    Path,
}

impl DecisionScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Path => "path",
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
    // Con valor por defecto: los modelos sin salida estricta pueden omitirlos.
    #[serde(default)]
    pub scope: DecisionScope,
    /// Carpeta o fichero cuando `scope` es `path`; vacío si no.
    #[serde(default)]
    pub scope_path: String,
    /// Claves (`D3`) de decisiones de sesiones anteriores que esta reemplaza.
    #[serde(default)]
    pub replaces_previous: Vec<String>,
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

/// Un modelo de embeddings: convierte textos en vectores cuya cercanía
/// indica parecido de significado.
pub trait Embedder {
    /// Identificador estable (`openai-compatible:nomic-embed-text`): los
    /// vectores de modelos distintos no se comparan.
    fn model_id(&self) -> String;

    /// Un vector por texto, en el mismo orden.
    ///
    /// # Errors
    /// Si falla la llamada o la respuesta no trae un vector por texto.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, ExtractError>;
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

/// Lo que se envía al modelo: un trozo de diálogo, las decisiones ya
/// extraídas de trozos anteriores de la misma sesión y las vigentes de
/// sesiones anteriores, para poder reemplazarlas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractionRequest {
    pub dialogue: String,
    pub known_titles: Vec<String>,
    pub previous: Vec<PreviousDecision>,
}

/// Decisión vigente de una sesión anterior, con la clave que usará el modelo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousDecision {
    pub key: String,
    pub title: String,
    pub decision: String,
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
- scope: user si es una preferencia o convención de la persona que vale para cualquier proyecto (idioma de los \
commits, estilo, herramientas que prefiere); path si solo afecta a una carpeta o fichero concreto; project en \
el resto de casos.
- scope_path: la carpeta o fichero cuando scope es path (tal como aparece en la conversación); si no, cadena vacía.
- replaces_previous: claves (D1, D2…) de las decisiones vigentes de sesiones anteriores que esta conversación \
cambia o revoca de forma explícita. Lista vacía si no cambia ninguna; no la uses para decisiones que solo \
se mencionan o se confirman.

Escribe en el idioma de la conversación. Usa solo lo que dice la conversación; no inventes motivos. \
Si no hay decisiones, devuelve una lista vacía.";

/// Mensaje de usuario con el diálogo y las decisiones ya conocidas.
pub fn user_message(request: &ExtractionRequest) -> String {
    let mut message = String::new();
    if !request.previous.is_empty() {
        message.push_str("Decisiones vigentes de sesiones anteriores (clave: título — decisión):\n");
        for previous in &request.previous {
            let _ = writeln!(message, "- {}: {} — {}", previous.key, previous.title, previous.decision);
        }
        message.push('\n');
    }
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
                        "supersedes": {"type": "string"},
                        "scope": {"type": "string", "enum": ["user", "project", "path"]},
                        "scope_path": {"type": "string"},
                        "replaces_previous": strings
                    },
                    "required": [
                        "title", "decision", "rationale", "alternatives", "entities", "status", "supersedes",
                        "scope", "scope_path", "replaces_previous"
                    ],
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
            decision.scope_path = decision.scope_path.trim().to_owned();
            if decision.scope != DecisionScope::Path || decision.scope_path.is_empty() {
                // Un alcance de ruta sin ruta no sirve: se queda en el proyecto.
                if decision.scope == DecisionScope::Path {
                    decision.scope = DecisionScope::Project;
                }
                decision.scope_path.clear();
            }
            decision.replaces_previous = decision
                .replaces_previous
                .iter()
                .map(|key| key.trim().to_uppercase())
                .filter(|k| !k.is_empty())
                .collect();
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
        assert_eq!(decisions[0].scope, DecisionScope::Project, "sin alcance, el proyecto");
    }

    #[test]
    fn scopes_and_previous_replacements_are_normalized() {
        let text = r#"{"decisions": [
            {"title": "Commits en español", "decision": "d", "rationale": "r", "alternatives": [], "entities": [],
             "status": "accepted", "supersedes": "", "scope": "user", "scope_path": "x", "replaces_previous": [" d2 "]},
            {"title": "Kysely en la API", "decision": "d", "rationale": "r", "alternatives": [], "entities": [],
             "status": "accepted", "supersedes": "", "scope": "path", "scope_path": " apps/api ", "replaces_previous": []},
            {"title": "Sin ruta", "decision": "d", "rationale": "r", "alternatives": [], "entities": [],
             "status": "accepted", "supersedes": "", "scope": "path", "scope_path": "", "replaces_previous": []}
        ]}"#;
        let decisions = parse_output(text).unwrap();
        let scopes: Vec<_> = decisions.iter().map(|d| (d.scope, d.scope_path.as_str())).collect();
        assert_eq!(
            scopes,
            vec![(DecisionScope::User, ""), (DecisionScope::Path, "apps/api"), (DecisionScope::Project, "")]
        );
        assert_eq!(decisions[0].replaces_previous, vec!["D2"]);
    }

    #[test]
    fn user_message_lists_previous_decisions_with_their_keys() {
        let request = ExtractionRequest {
            dialogue: "hola".into(),
            previous: vec![PreviousDecision { key: "D1".into(), title: "SQLite".into(), decision: "En local".into() }],
            ..ExtractionRequest::default()
        };
        assert!(user_message(&request).starts_with("Decisiones vigentes de sesiones anteriores"));
        assert!(user_message(&request).contains("- D1: SQLite — En local\n"));
    }

    #[test]
    fn user_message_lists_known_decisions_first() {
        let request = ExtractionRequest {
            dialogue: "### Persona\nhola\n\n".into(),
            known_titles: vec!["Rust".into()],
            ..ExtractionRequest::default()
        };
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
