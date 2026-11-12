//! Bloques del guion: la forma que el generador (el agente o el modelo revisor)
//! elige para explicar algo cuando un párrafo no basta. Una tabla para comparar
//! valores, un esquema de qué llama a qué, una secuencia de mensajes…
//!
//! Todos comparten una forma plana (`type` y los campos que use cada tipo): a un
//! modelo local le resulta mucho más fiable generar eso que un esquema con
//! variantes, y sirve igual para el esquema estricto del revisor y el del agente.
//! Un tipo desconocido se descarta sin romper nada.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Los tipos que sabe dibujar la presentación, con cuándo usarlos.
pub const KINDS: &[(&str, &str)] = &[
    ("text", "un párrafo, cuando no hace falta más"),
    ("list", "una enumeración sin orden (items)"),
    ("steps", "un proceso en orden (items)"),
    ("table", "comparar valores o casos: columns y rows"),
    ("flow", "un esquema de cajas y flechas, qué llama a qué: nodes y edges"),
    ("sequence", "quién habla con quién y en qué orden: nodes (participantes) y edges (mensajes en orden)"),
    ("callout", "un aviso destacado: text y tone (info, tip, warn o risk)"),
    ("code", "un ejemplo literal, como un JSON de petición o un comando: text y language"),
];

const MAX_ITEMS: usize = 20;
const MAX_ROWS: usize = 30;
const MAX_COLUMNS: usize = 8;
const MAX_NODES: usize = 24;
const MAX_TEXT: usize = 4000;
/// Bloques por sitio (portada, capítulo o idea): más que esto ya no ayuda.
pub const MAX_BLOCKS: usize = 6;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub text: String,
    pub items: Vec<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub nodes: Vec<BlockNode>,
    pub edges: Vec<BlockEdge>,
    pub tone: String,
    pub language: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockNode {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockEdge {
    pub from: String,
    pub to: String,
    pub label: String,
}

fn clip(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

/// Lo que se puede dibujar: sin tipos desconocidos, sin bloques vacíos y con
/// tamaños razonables. Las aristas a nodos que no existen se quitan.
#[must_use]
pub fn sanitize(blocks: &[StoryBlock]) -> Vec<StoryBlock> {
    blocks
        .iter()
        .filter(|block| KINDS.iter().any(|(kind, _)| *kind == block.kind))
        .map(|block| {
            let mut block = block.clone();
            block.text = clip(&block.text);
            block.items.truncate(MAX_ITEMS);
            block.columns.truncate(MAX_COLUMNS);
            block.rows.truncate(MAX_ROWS);
            for row in &mut block.rows {
                row.truncate(MAX_COLUMNS.max(block.columns.len()));
            }
            block.nodes.truncate(MAX_NODES);
            let ids: Vec<String> = block.nodes.iter().map(|node| node.id.clone()).collect();
            block.edges.retain(|edge| ids.contains(&edge.from) && ids.contains(&edge.to));
            block.edges.truncate(MAX_NODES * 2);
            if !["info", "tip", "warn", "risk"].contains(&block.tone.as_str()) {
                "info".clone_into(&mut block.tone);
            }
            block
        })
        .filter(|block| match block.kind.as_str() {
            "list" | "steps" => !block.items.is_empty(),
            "table" => !block.columns.is_empty() && !block.rows.is_empty(),
            "flow" | "sequence" => block.nodes.len() >= 2 && !block.edges.is_empty(),
            _ => !block.text.trim().is_empty(),
        })
        .take(MAX_BLOCKS)
        .collect()
}

/// El bloque para las salidas estructuradas estrictas: todo obligatorio, plano.
#[must_use]
pub fn output_schema() -> Value {
    let strings = json!({"type": "array", "items": {"type": "string"}});
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "type": {"type": "string", "enum": KINDS.iter().map(|(kind, _)| *kind).collect::<Vec<_>>()},
                "title": {"type": "string"},
                "text": {"type": "string"},
                "items": strings,
                "columns": strings,
                "rows": {"type": "array", "items": strings},
                "nodes": {"type": "array", "items": {
                    "type": "object", "properties": {"id": {"type": "string"}, "label": {"type": "string"}},
                    "required": ["id", "label"], "additionalProperties": false}},
                "edges": {"type": "array", "items": {
                    "type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}, "label": {"type": "string"}},
                    "required": ["from", "to", "label"], "additionalProperties": false}},
                "tone": {"type": "string", "enum": ["info", "tip", "warn", "risk"]},
                "language": {"type": "string"}
            },
            "required": ["type", "title", "text", "items", "columns", "rows", "nodes", "edges", "tone", "language"],
            "additionalProperties": false
        }
    })
}

/// Cómo se le explica al generador qué bloques puede usar.
#[must_use]
pub fn guide() -> String {
    let kinds: Vec<String> = KINDS.iter().map(|(kind, use_for)| format!("{kind} ({use_for})")).collect();
    format!(
        "blocks: la forma que mejor explique algo cuando un párrafo no basta, solo si aporta: {}. Deja vacíos los \
         campos que el tipo no use. Como mucho {MAX_BLOCKS} por sitio.",
        kinds.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(value: Value) -> StoryBlock {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn unknown_or_empty_blocks_are_dropped_and_dangling_edges_removed() {
        let found = sanitize(&[
            block(json!({"type": "hologram", "text": "?"})),
            block(json!({"type": "table", "columns": ["a"], "rows": []})),
            block(json!({"type": "list", "items": ["uno"]})),
            block(json!({"type": "flow", "nodes": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}],
                         "edges": [{"from": "a", "to": "b", "label": "llama"}, {"from": "a", "to": "z", "label": "?"}]})),
            block(json!({"type": "callout", "text": "Cuidado", "tone": "rojo"})),
        ]);
        let kinds: Vec<&str> = found.iter().map(|block| block.kind.as_str()).collect();
        assert_eq!(kinds, ["list", "flow", "callout"]);
        assert_eq!(found[1].edges.len(), 1, "la flecha a un nodo que no existe se quita");
        assert_eq!(found[2].tone, "info", "un tono desconocido vuelve al de por defecto");
    }

    #[test]
    fn the_schema_lists_every_kind_the_presentation_draws() {
        let schema = output_schema();
        let kinds = schema["items"]["properties"]["type"]["enum"].as_array().unwrap();
        assert_eq!(kinds.len(), KINDS.len());
        assert!(guide().contains("sequence"));
    }
}
