//! `huella mcp`: servidor MCP por stdio para que los agentes consulten la
//! memoria (historia de un símbolo, decisiones, guion de revisión).
//!
//! El transporte stdio de MCP es JSON-RPC 2.0 con un mensaje por línea; con
//! tres métodos basta y se evita arrastrar un runtime asíncrono.

use std::fmt::Write as _;
use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::Result;
use huella_store::{DecisionFilter, Store};
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2025-06-18";

const INSTRUCTIONS: &str = "Huella guarda la memoria de las sesiones con agentes de IA anclada al código: qué se pidió \
y por qué se escribió cada función, las decisiones de proyecto y la revisión guiada de cada sesión. Consulta \
huella_symbol_history antes de cambiar código que no conoces, huella_decisions antes de proponer cambios de \
arquitectura o tecnología, y huella_review para explicar o revisar lo que hizo una sesión. Mientras escribes \
código, registra con huella_record cada decisión entre alternativas, lo que das por hecho, lo que no has \
comprobado y cómo funciona lo que no es obvio: es lo que verá quien revise.";

pub struct Context {
    pub store: Store,
    pub sources: huella_engine::Sources,
}

pub fn serve(context: &mut Context) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(context, &message),
            Err(error) => Some(error_response(&Value::Null, -32700, &format!("JSON inválido: {error}"))),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Respuesta a un mensaje; `None` para las notificaciones.
pub fn handle(context: &mut Context, message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "huella", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let (text, is_error) = match call(context, name, &params["arguments"]) {
                Ok(text) => (text, false),
                Err(error) => (format!("{error:#}"), true),
            };
            json!({"content": [{"type": "text", "text": text}], "isError": is_error})
        }
        other => return Some(error_response(&id, -32601, &format!("método no soportado: {other}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tools() -> Value {
    let mut tools = json!([
        {
            "name": "huella_symbol_history",
            "description": "Qué se pidió y qué explicó el agente en cada sesión que tocó la función, clase o método \
                            que hay en una línea de un fichero. Avisa si el código cambió desde entonces.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": {"type": "string", "description": "Ruta absoluta del fichero"},
                    "line": {"type": "integer", "description": "Línea (empezando en 1)"}
                },
                "required": ["file", "line"]
            }
        },
        {
            "name": "huella_decisions",
            "description": "Decisiones de proyecto vigentes (tecnologías, arquitectura, descartes) con su porqué y \
                            las alternativas descartadas.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {"type": "string", "description": "Tecnología o concepto, en minúsculas (rust, sqlite…)"},
                    "project": {"type": "string", "description": "Ruta del repositorio (incluye las preferencias de la persona)"},
                    "scope": {"type": "string", "enum": ["user", "project", "path"],
                              "description": "user: preferencias de la persona en cualquier proyecto"},
                    "include_inactive": {"type": "boolean", "description": "Incluir reemplazadas y descartadas"}
                }
            }
        },
        {
            "name": "huella_review",
            "description": "Guion de revisión de una sesión: por turnos, los pasos en el orden del flujo, qué líneas no \
                            tienen explicación, puntos de atención y, si existe, la narración. Sin sesión, usa la \
                            revisión activa o la sesión más reciente.",
            "inputSchema": {
                "type": "object",
                "properties": {"session": {"type": "string", "description": "Id de la sesión o su comienzo"}}
            }
        },
        {
            "name": "huella_search",
            "description": "Busca en la memoria de las sesiones por palabras: peticiones de la persona, explicaciones \
                            del agente, ficheros, símbolos y decisiones. Úsala para encontrar dónde y por qué se hizo \
                            algo («migración de usuarios», «rate limit») antes de tocar código relacionado.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Palabras a buscar"},
                    "project": {"type": "string", "description": "Ruta del repositorio, para limitar la búsqueda"},
                    "limit": {"type": "integer"}
                },
                "required": ["query"]
            }
        },
        {
            "name": "huella_review_feedback",
            "description": "Lo que la persona pidió corregir al revisar una sesión: cada comentario con su fichero, su \
                            función, la línea y el código del cambio, y los cambios que pidió deshacer. Úsala cuando te \
                            pidan aplicar las correcciones de la revisión. Sin sesión, la más reciente con cambios pedidos.",
            "inputSchema": {
                "type": "object",
                "properties": {"session": {"type": "string", "description": "Id de la sesión revisada o su comienzo"}}
            }
        },
        {
            "name": "huella_sessions",
            "description": "Sesiones recientes con ediciones, con su proyecto.",
            "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer"}}}
        }
    ]);
    // Antes de huella_sessions, que cierra la lista.
    if let Some(list) = tools.as_array_mut() {
        let at = list.len().saturating_sub(1);
        list.insert(at, record_tool());
    }
    tools
}

/// `huella_record`: el agente declara lo que decidió sobre el código que acaba de escribir.
fn record_tool() -> Value {
    json!({
        "name": "huella_record",
        "description": "Registra, justo después de escribir un trozo de código, una decisión que tomaste, algo \
                        que diste por hecho, algo que no comprobaste o un riesgo, y cómo funciona si no es \
                        obvio. Quien revise lo verá junto a ese código. Una entrada por decisión; no registres \
                        lo trivial (renombrados, formato, código que se explica solo). Sé concreto y honesto: \
                        lo que no probaste va en unverified, no se adorna.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file": {"type": "string", "description": "Ruta absoluta del fichero ya escrito"},
                "symbol": {"type": "string", "description": "Función, método o clase (Clase.metodo o su nombre)"},
                "lines": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2,
                          "description": "[inicio, fin], 1-based, si no hay símbolo o es una parte de él"},
                "kind": {"type": "string", "enum": ["decision", "assumption", "unverified", "risk"],
                         "description": "decision: elegiste entre alternativas; assumption: das algo por hecho \
                                         que la tarea no dice; unverified: dejas algo sin comprobar; risk: \
                                         tocas algo sensible (seguridad, datos, API pública)"},
                "summary": {"type": "string", "description": "Qué hiciste y por qué, en 1 a 3 frases"},
                "how": {"type": "string", "description": "Cómo funciona el código, si no es obvio al leerlo"},
                "alternatives": {"type": "array", "items": {"type": "object", "properties": {
                    "option": {"type": "string"}, "why_not": {"type": "string"}},
                    "required": ["option", "why_not"]},
                    "description": "Opciones que descartaste y por qué"},
                "assumptions": {"type": "array", "items": {"type": "string"}},
                "unverified": {"type": "array", "items": {"type": "string"},
                               "description": "Lo que no has probado ni comprobado"},
                "confidence": {"type": "string", "enum": ["high", "medium", "low"]},
                "review_hint": {"type": "string", "description": "Qué debería mirar quien revise"}
            },
            "required": ["file", "kind", "summary"]
        }
    })
}

fn call(context: &mut Context, name: &str, arguments: &Value) -> Result<String> {
    // Registrar no necesita la memoria: que no espere al indexado.
    if name == "huella_record" {
        return record(&context.store, arguments);
    }
    // Lo nuevo de las transcripciones se indexa antes de responder.
    huella_engine::index(&mut context.store, &context.sources)?;
    match name {
        "huella_symbol_history" => symbol_history(&context.store, arguments),
        "huella_decisions" => decisions(&context.store, arguments),
        "huella_review" => review(context, arguments),
        "huella_review_feedback" => review_feedback(context, arguments),
        "huella_search" => {
            let query = huella_store::SearchQuery {
                text: arguments["query"].as_str().ok_or_else(|| anyhow::anyhow!("falta `query`"))?.to_owned(),
                project: arguments["project"].as_str().map(|p| huella_engine::normalize_path(Path::new(p), None)),
                limit: arguments["limit"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(10),
                include_inactive: false,
            };
            let embedder = crate::embedder(None, None);
            let (hits, warning) = huella_engine::embeddings::search(
                &context.store,
                &query,
                embedder.as_ref().map(|e| e as &dyn huella_decisions::Embedder),
            )?;
            let mut text = crate::search::render_hits(&hits);
            if let Some(warning) = warning {
                let _ = write!(text, "\n(aviso: {warning})");
            }
            Ok(text)
        }
        "huella_sessions" => {
            let limit = arguments["limit"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(10);
            let sessions = huella_engine::list_sessions(&context.store, limit)?;
            Ok(crate::review::render_sessions(&sessions))
        }
        other => anyhow::bail!("herramienta desconocida: {other}"),
    }
}

/// `huella_record`: guarda lo que el agente declara sobre el código que acaba de escribir.
fn record(store: &Store, arguments: &Value) -> Result<String> {
    let text = |key: &str| arguments[key].as_str().map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned);
    let list = |key: &str| -> Vec<String> {
        arguments[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item.as_str().map(str::trim).filter(|item| !item.is_empty()).map(str::to_owned))
            .collect()
    };
    let lines = arguments["lines"].as_array().and_then(|range| {
        let line = |index: usize| range.get(index)?.as_u64().and_then(|n| usize::try_from(n).ok());
        Some((line(0)?, line(1)?))
    });
    let new = huella_engine::declared::NewDeclaration {
        file: text("file").ok_or_else(|| anyhow::anyhow!("falta `file`"))?.into(),
        symbol: text("symbol"),
        lines,
        kind: text("kind").ok_or_else(|| anyhow::anyhow!("falta `kind`"))?,
        summary: text("summary").unwrap_or_default(),
        how: text("how"),
        alternatives: arguments["alternatives"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                Some(huella_store::Alternative {
                    option: item["option"].as_str()?.trim().to_owned(),
                    why_not: item["why_not"].as_str().unwrap_or_default().trim().to_owned(),
                })
            })
            .collect(),
        assumptions: list("assumptions"),
        unverified: list("unverified"),
        confidence: text("confidence"),
        review_hint: text("review_hint"),
    };
    let recorded = huella_engine::declared::record(store, &new)?;
    let place = match &recorded.symbol {
        Some(symbol) => format!("`{symbol}`"),
        None => "sus líneas (no encontré un símbolo)".to_owned(),
    };
    Ok(format!(
        "Registrado #{} en {place}, líneas {}–{} de {}.",
        recorded.id,
        recorded.lines.0,
        recorded.lines.1,
        new.file.display()
    ))
}

fn review_feedback(context: &Context, arguments: &Value) -> Result<String> {
    let session = if let Some(session) = arguments["session"].as_str() {
        session.to_owned()
    } else {
        let states = context.store.review_states()?;
        let pending =
            states.values().filter(|s| s.status == "changes_requested").max_by(|a, b| a.updated_at.cmp(&b.updated_at));
        let Some(state) = pending else { return Ok("No hay ninguna revisión con cambios pedidos.".to_owned()) };
        state.session_id.clone()
    };
    let review = huella_engine::session_review(&context.sources, &session)?;
    let plan = huella_review::flow::plan(&review);
    Ok(huella_engine::review_loop::feedback(&context.store, &review, &plan)?.markdown)
}

fn symbol_history(store: &Store, arguments: &Value) -> Result<String> {
    let file = arguments["file"].as_str().ok_or_else(|| anyhow::anyhow!("falta `file`"))?;
    let line = arguments["line"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| anyhow::anyhow!("falta `line`"))?;
    let source = std::fs::read_to_string(file).map_err(|error| anyhow::anyhow!("no se pudo leer {file}: {error}"))?;
    Ok(match huella_engine::memory_at(store, Path::new(file), &source, line)? {
        Some(memory) => huella_engine::render_markdown(&memory, 10),
        None => format!("Huella no recuerda ninguna sesión que tocara {file}:{line}."),
    })
}

fn decisions(store: &Store, arguments: &Value) -> Result<String> {
    let filter = DecisionFilter {
        entity: arguments["entity"].as_str().map(str::to_owned),
        project: arguments["project"].as_str().map(|p| huella_engine::normalize_path(Path::new(p), None)),
        scope: arguments["scope"].as_str().map(str::to_owned),
        include_inactive: arguments["include_inactive"].as_bool().unwrap_or(false),
    };
    let found = store.decisions(&filter)?;
    if found.is_empty() {
        return Ok("No hay decisiones con ese filtro.".to_owned());
    }
    let mut out = String::new();
    for decision in found {
        let scope = match (decision.scope.as_str(), &decision.scope_path) {
            ("user", _) => " · preferencia de la persona".to_owned(),
            ("path", Some(path)) => format!(" · solo {path}"),
            _ => String::new(),
        };
        let _ = writeln!(out, "## {} ({}{scope})", decision.title, decision.status);
        let _ = writeln!(out, "{}\nPor qué: {}", decision.decision, decision.rationale);
        for alternative in &decision.alternatives {
            let _ = writeln!(out, "- Descartado: {alternative}");
        }
        if let Some(by) = &decision.superseded_by {
            let _ = writeln!(out, "Reemplazada por: {by}");
        }
        let _ = writeln!(
            out,
            "Entidades: {} · proyecto: {} · fecha: {}\n",
            decision.entities.join(", "),
            decision.project.as_deref().unwrap_or("-"),
            decision.decided_at.as_deref().unwrap_or("-")
        );
    }
    Ok(out)
}

fn review(context: &Context, arguments: &Value) -> Result<String> {
    let session = match arguments["session"].as_str() {
        Some(session) => session.to_owned(),
        None => match context.store.active_review()? {
            Some(active) => active,
            None => huella_engine::list_sessions(&context.store, 1)?
                .into_iter()
                .next()
                .map(|listing| listing.summary.id)
                .ok_or_else(|| anyhow::anyhow!("no hay sesiones con ediciones"))?,
        },
    };
    let review = huella_engine::session_review(&context.sources, &session)?;
    let plan = huella_review::flow::plan(&review);
    let points = huella_review::attention::attention(&review, &plan);
    let narrations = huella_engine::narration::narrations(&context.store, &review, &plan, &points)?;
    Ok(strip_ansi(&crate::review::render_flow(&review, &plan, &points, &narrations, None)))
}

/// El renderizado de terminal ya no pinta colores fuera de una terminal, pero
/// por si acaso: los agentes reciben texto plano.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for next in chars.by_ref() {
                if next == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        let projects = std::env::temp_dir().join(format!("huella-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&projects).unwrap();
        Context { store: Store::open_in_memory().unwrap(), sources: huella_engine::Sources::claude_code(projects) }
    }

    #[test]
    fn initializes_and_lists_tools() {
        let mut context = context();
        let init = handle(&mut context, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}))
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "huella");
        assert_eq!(handle(&mut context, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})), None);
        let tools = handle(&mut context, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let names: Vec<&str> =
            tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "huella_symbol_history",
                "huella_decisions",
                "huella_review",
                "huella_search",
                "huella_review_feedback",
                "huella_record",
                "huella_sessions"
            ]
        );
    }

    #[test]
    fn tool_errors_are_reported_to_the_agent_not_as_protocol_errors() {
        let mut context = context();
        let call = handle(
            &mut context,
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "huella_symbol_history", "arguments": {"file": "/no/existe.ts", "line": 1}}}),
        )
        .unwrap();
        assert_eq!(call["result"]["isError"], true);
        assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("no se pudo leer"));
        let unknown = handle(&mut context, &json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"})).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn agents_record_what_they_decided_about_the_code() {
        let mut context = context();
        let dir = std::env::temp_dir().join(format!("huella-mcp-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("total.ts");
        std::fs::write(&file, "export function total(items) {\n  return items.length;\n}\n").unwrap();
        let call = |context: &mut Context, arguments: Value| {
            let response = handle(
                context,
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "huella_record", "arguments": arguments}}),
            )
            .unwrap();
            (
                response["result"]["content"][0]["text"].as_str().unwrap().to_owned(),
                response["result"]["isError"] == true,
            )
        };
        let (text, error) = call(
            &mut context,
            json!({"file": file, "symbol": "total", "kind": "assumption", "summary": "Cuento los elementos, no sumo precios",
                   "unverified": ["listas vacías"], "alternatives": [{"option": "sumar precios", "why_not": "la tarea no lo pide"}]}),
        );
        assert!(!error, "{text}");
        assert!(text.contains("`total`, líneas 1–3"), "{text}");
        let stored = context
            .store
            .declarations(&huella_engine::normalize_path(&std::fs::canonicalize(&file).unwrap(), None))
            .unwrap();
        assert_eq!(stored[0].alternatives[0].option, "sumar precios");
        let (text, error) = call(&mut context, json!({"file": file, "kind": "opinion", "summary": "x"}));
        assert!(error && text.contains("kind"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decisions_tool_reads_the_store() {
        let mut context = context();
        let decision = huella_store::NewDecision {
            title: "Rust para el producto".into(),
            decision: "Todo en Rust".into(),
            rationale: "binario único".into(),
            alternatives: vec!["Python: distribución frágil".into()],
            entities: vec!["rust".into()],
            status: "accepted".into(),
            supersedes: String::new(),
            scope: "project".into(),
            scope_path: None,
            replaces: Vec::new(),
        };
        context
            .store
            .replace_decisions(&huella_store::SessionDecisions {
                session_id: "s",
                project: Some("/repo"),
                extractor: "t",
                turns: 4,
                decided_at: None,
                decisions: &[decision],
            })
            .unwrap();
        let call = handle(
            &mut context,
            &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": {"name": "huella_decisions", "arguments": {"entity": "Rust"}}}),
        )
        .unwrap();
        let text = call["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.starts_with("## Rust para el producto (accepted)\nTodo en Rust\nPor qué: binario único"),
            "{text}"
        );
    }

    #[test]
    fn ansi_codes_are_removed() {
        assert_eq!(strip_ansi("\x1b[1;36m━━ Turno\x1b[0m"), "━━ Turno");
    }
}
