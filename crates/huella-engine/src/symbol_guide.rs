//! «Entender este símbolo»: una guía de una clase (o función) entera, para
//! quien tiene que entenderla y no la conoce. Va en dos pasadas, para que
//! quepa en el contexto de un modelo local:
//!
//! 1. **Cada método**: qué hace, qué recibe y devuelve, qué efectos tiene y
//!    qué reglas aplica. Se guarda por el hash de su código: si una clase
//!    cambia en un método, solo se rehace ese.
//! 2. **La clase**: con el esqueleto (campos y firmas), los resúmenes de los
//!    métodos, los tests que la nombran, quién la usa, su historia en Huella y
//!    lo que declaró el agente, una guía con siempre la misma forma:
//!    para qué existe, glosario, mapa de casos de uso, dependencias, reglas
//!    con sus tests, flujos, por qué es así, lo dudoso, orden de lectura y
//!    preguntas para comprobar que se ha entendido.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_core::{Symbol, SymbolKind};
use huella_decisions::{ExtractError, JsonModel};
use huella_store::{CodeExplanation, NewCodeExplanation, Store};
use serde_json::{Value, json};

use crate::code_ask::{clip, write_history};
use crate::explain::{TestBudget, code_hash, tests_naming};
use crate::paths::normalize_path;
use crate::query::memory_at;
use crate::workspace::git;
use crate::{EngineError, Result};

/// Tests de la clase que se envían, y líneas de cada uno.
const MAX_TESTS: usize = 80;
const TEST_LINES: usize = 15;
/// Métodos que se resumen; del resto solo se dice que existen.
const MAX_MEMBERS: usize = 30;
/// Líneas de un método que se envían para resumirlo.
const MAX_MEMBER_LINES: usize = 300;
/// Líneas del esqueleto de la clase.
const MAX_SKELETON_LINES: usize = 200;
/// Usos desde otros ficheros que se envían.
const MAX_CALLERS: usize = 15;
/// Métodos con historia en Huella que se cuentan, además de la clase.
const MAX_MEMBER_HISTORIES: usize = 5;
const MAX_CHARS: usize = 110_000;

const MEMBER_SYSTEM: &str = "\
Resumes métodos o funciones para que luego se pueda explicar la clase entera. Solo con el código que se te da. \
Devuelve uno por cada método numerado, en `methods`, con su número en `index`.

- `purpose`: qué consigue, en una o dos frases, en palabras del dominio.
- `inputs` y `returns`: qué recibe y qué devuelve, con su significado (no solo el tipo).
- `effects`: lo que cambia fuera de él: estado del objeto, base de datos, llamadas a otros servicios, eventos, logs que importan.
- `calls`: las funciones o dependencias a las que llama que importan para entenderlo.
- `rules`: cada condición o validación que decide qué pasa: `when` (la situación, con valores límite exactos), \
  `then` (qué pasa, con el nombre exacto del error o estado), `kind` (validation, invariant, error, calculation, \
  state, permission u other) y sus líneas del fichero [inicio, fin] (null si no se sabe).
- Escribe en el idioma de los comentarios del código; si no hay, en español.";

const GUIDE_SYSTEM: &str = "\
Escribes la guía de una clase (o función) para alguien que tiene que entenderla entera y no conoce ni el código \
ni el dominio. Tienes su esqueleto, el resumen de cada método, los tests que la nombran, quién la usa, su historia \
en Huella (lo que se pidió y lo que explicó el agente que la escribió) y lo que declaró el agente.

- `purpose`: para qué existe, qué responsabilidad tiene y qué no le toca, en un párrafo. Empieza por el problema \
  del dominio que resuelve, no por el código.
- `glossary`: los términos del dominio que hay que conocer para leerla, con su significado aquí.
- `map`: cada método público (o punto de entrada) como un caso de uso: `method` (su nombre), `use_case` (qué \
  pide quien lo llama, en palabras del dominio), `lines` [inicio, fin].
- `dependencies`: con quién habla (repositorios, otros servicios, clientes…) y para qué (`role`).
- `state`: el estado que guarda o cambia, y cuándo.
- `rules`: las reglas de negocio de toda la clase, sin repetir: `kind`, `when`, `then`, `why` (por qué existe, \
  según la historia o deducido, diciéndolo), `method`, `lines` y `tests` (nombres de los tests que la comprueban \
  de verdad; vacío si ninguno). Es la parte más importante: sé exhaustivo y concreto. Incluye todas las reglas \
  de los resúmenes de los métodos, tengan test o no: una regla sin test es justo lo que más interesa ver.
- `flows`: los recorridos que importan (normalmente uno por caso de uso principal): `title`, `method` y `steps` \
  (frases cortas en orden, nombrando las llamadas).
- `why`: por qué es así: decisiones y peticiones de su historia, cada una con `text` y `source` (history si \
  consta en la historia, declared si lo declaró el agente, code si lo deduces del código).
- `doubts`: lo dudoso: reglas sin test, casos que no contempla, código que contradice lo declarado, cosas raras.
- `reading_order`: en qué orden leer los métodos para entenderla, con por qué (`method`, `why`).
- `questions`: de 3 a 6 preguntas para comprobar que se ha entendido, con su respuesta.
- Las líneas son números de línea del fichero, como salen en el contexto: las de esa regla concreta (la \
  condición, el `throw`), no las del método entero. Si no se sabe, null.
- No inventes: si algo no se ve en el contexto, no lo afirmes.
- Escribe en el idioma de la historia y de los comentarios del código; si no hay, en español.";

/// Lo que se pide entender.
#[derive(Debug, Clone, Copy)]
pub struct UnderstandSymbol<'a> {
    pub path: &'a Path,
    /// Nombre cualificado (`Pedidos`, `Pedidos.reservar`).
    pub symbol: &'a str,
    /// El contenido revisado del fichero, si no es el del disco.
    pub reviewed: Option<&'a str>,
    /// Rehacer la guía aunque el código no haya cambiado. Los resúmenes de los
    /// métodos se reutilizan igual: solo dependen de su código.
    pub refresh: bool,
}

/// Una guía guardada.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolGuide {
    pub explanation: CodeExplanation,
    pub content: Value,
    /// El código cambió desde que se escribió: puede no valer ya.
    pub stale: bool,
}

/// La guía guardada más reciente de `symbol`, con si su código ha cambiado.
///
/// # Errors
/// Si falla la base de datos.
pub fn saved_guide(store: &Store, path: &Path, symbol: &str, source: &str) -> Result<Option<SymbolGuide>> {
    let current = find_symbol(path, source, symbol).map(|found| code_hash(&code_of(source, &found)));
    Ok(store
        .code_explanations(&normalize_path(path, None))?
        .into_iter()
        .rev()
        .find(|explanation| explanation.kind == "symbol" && explanation.symbol.as_deref() == Some(symbol))
        .map(|explanation| {
            let content = serde_json::from_str(&explanation.content).unwrap_or(Value::Null);
            let stale = current.as_deref() != Some(explanation.code_hash.as_str());
            SymbolGuide { explanation, content, stale }
        }))
}

/// La guía que escribió el agente, ya comprobada, y lo que no cuadra con el código.
#[derive(Debug, Clone, PartialEq)]
pub struct ToldGuide {
    pub guide: SymbolGuide,
    pub problems: Vec<String>,
}

/// Guarda la guía de un símbolo que escribió el agente (`huella_symbol_guide`),
/// con la misma forma que la del modelo revisor. Comprueba que el símbolo
/// exista, que cada regla esté en una línea suya (por el texto de esa línea,
/// `quote`) y que los tests que cita nombren de verdad la clase; lo que no
/// cuadre se le devuelve para que lo arregle.
///
/// # Errors
/// Si el fichero o el símbolo no existen, la guía no dice para qué existe el
/// símbolo, o falla la base de datos.
pub fn record_told(store: &Store, path: &Path, symbol: &str, mut content: Value) -> Result<ToldGuide> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| EngineError::Invalid(format!("no se pudo leer {}: {error}", path.display())))?;
    let symbols = symbols_of(path, &source);
    let target = symbols
        .iter()
        .find(|s| s.qualified_name == symbol)
        .or_else(|| symbols.iter().find(|s| short_name(&s.qualified_name) == short_name(symbol)))
        .cloned()
        .ok_or_else(|| EngineError::Invalid(format!("no encuentro `{symbol}` en {}", path.display())))?;
    if content["purpose"].as_str().is_none_or(|purpose| purpose.trim().is_empty()) {
        return Err(EngineError::Invalid("la guía necesita purpose: para qué existe y qué no le toca".into()));
    }
    for list in
        ["glossary", "map", "dependencies", "state", "rules", "flows", "why", "doubts", "reading_order", "questions"]
    {
        if !content[list].is_array() {
            content[list] = json!([]);
        }
    }
    let members = members_of(&symbols, &target);
    let range_of = |method: &str| {
        members.iter().find(|m| short_name(&m.qualified_name) == short_name(method)).map(|m| (m.start_line, m.end_line))
    };
    let lines: Vec<&str> = source.lines().collect();
    let mut problems = Vec::new();

    // Los tests que nombran la clase o sus métodos: los que una regla puede citar.
    let mut names: Vec<String> = vec![short_name(&target.qualified_name).to_owned()];
    names.extend(members.iter().map(|m| short_name(&m.qualified_name).to_owned()));
    names.retain(|name| name.len() > 2 && crate::definition::is_identifier(name));
    names.dedup();
    names.truncate(10);
    let tests = tests_naming(path, &names, TestBudget { tests: MAX_TESTS, lines: TEST_LINES });

    if let Some(rules) = content["rules"].as_array_mut() {
        problems.extend(check_rules(rules, &target, &members, &lines, &tests));
    }
    if let Some(map) = content["map"].as_array_mut() {
        for entry in map {
            entry["lines"] = entry["method"].as_str().and_then(range_of).map_or(Value::Null, |(a, b)| json!([a, b]));
        }
    }
    content["tests_found"] = json!(
        tests.iter().map(|t| json!({"name": t.name, "path": t.display_path, "line": t.start_line})).collect::<Vec<_>>()
    );
    content["methods"] = json!(
        members
            .iter()
            .map(|m| json!({"name": m.qualified_name, "lines": [m.start_line, m.end_line]}))
            .collect::<Vec<_>>()
    );

    let code = code_of(&source, &target);
    let key = normalize_path(path, None);
    let id = store.add_code_explanation(&NewCodeExplanation {
        path: &key,
        kind: "symbol",
        symbol: Some(&target.qualified_name),
        start_line: target.start_line,
        end_line: target.end_line,
        quote: &code,
        code_hash: &code_hash(&code),
        content: &content.to_string(),
        model: "agent",
    })?;
    let explanation = store
        .code_explanations(&key)?
        .into_iter()
        .find(|explanation| explanation.id == id)
        .ok_or_else(|| EngineError::Model("no se encontró la guía recién guardada".into()))?;
    Ok(ToldGuide { guide: SymbolGuide { explanation, content, stale: false }, problems })
}

/// Ancla cada regla de la guía del agente a la línea de su `quote`, dentro de su
/// método, y dice qué no cuadra: líneas que no están, métodos que no son de la
/// clase y tests que no la nombran.
fn check_rules(
    rules: &mut [Value],
    target: &Symbol,
    members: &[Symbol],
    lines: &[&str],
    tests: &[crate::explain::TestRef],
) -> Vec<String> {
    let range_of = |method: &str| {
        members.iter().find(|m| short_name(&m.qualified_name) == short_name(method)).map(|m| (m.start_line, m.end_line))
    };
    let mut problems = Vec::new();
    for (index, rule) in rules.iter_mut().enumerate() {
        let label = format!("regla {} («{}»)", index + 1, clip(rule["when"].as_str().unwrap_or_default(), 50));
        let (start, end) = rule["method"].as_str().and_then(range_of).unwrap_or((target.start_line, target.end_line));
        let quote = rule["quote"].as_str().map(str::trim).unwrap_or_default().to_owned();
        let found = (!quote.is_empty())
            .then(|| {
                (start..=end.min(lines.len()))
                    .find(|&n| lines[n - 1].trim() == quote || (quote.len() > 3 && lines[n - 1].contains(&quote)))
            })
            .flatten();
        rule["lines"] = found.map_or(Value::Null, |line| json!([line, line]));
        if found.is_none() {
            problems.push(format!(
                "{label}: no encuentro su línea; pon en quote el texto exacto de la línea donde se decide"
            ));
        }
        if let Some(method) = rule["method"].as_str().filter(|m| !m.is_empty())
            && range_of(method).is_none()
            && short_name(method) != short_name(&target.qualified_name)
        {
            problems.push(format!("{label}: `{method}` no es un método de `{}`", target.qualified_name));
        }
        if !rule["tests"].is_array() {
            rule["tests"] = json!([]);
        }
        for test in rule["tests"].as_array().cloned().unwrap_or_default() {
            let name = test.as_str().unwrap_or_default();
            if !tests.iter().any(|t| t.name == name || short_name(&t.name) == short_name(name)) {
                problems.push(format!("{label}: no encuentro el test `{name}` entre los que nombran la clase"));
            }
        }
        if let Some(object) = rule.as_object_mut() {
            object.remove("quote");
        }
    }
    problems
}

/// Escribe (o reutiliza) la guía de un símbolo. `progress(hechos, total)` se
/// llama con cada método resumido.
///
/// # Errors
/// Si el fichero no se puede leer, el símbolo no está, o fallan el modelo o la
/// base de datos.
pub fn understand_symbol(
    store: &Store,
    sources: Option<&crate::Sources>,
    model: &dyn JsonModel,
    ask: UnderstandSymbol<'_>,
    progress: &dyn Fn(usize, usize),
) -> Result<SymbolGuide> {
    let UnderstandSymbol { path, symbol, reviewed, refresh } = ask;
    let source = match reviewed {
        Some(reviewed) => reviewed.to_owned(),
        None => std::fs::read_to_string(path)
            .map_err(|error| EngineError::Model(format!("no se pudo leer {}: {error}", path.display())))?,
    };
    let symbols = symbols_of(path, &source);
    let target = symbols
        .iter()
        .find(|s| s.qualified_name == symbol)
        .cloned()
        .ok_or_else(|| EngineError::Model(format!("no se encontró {symbol} en {}", path.display())))?;
    let key = normalize_path(path, None);
    let code = code_of(&source, &target);
    let hash = code_hash(&code);
    if !refresh && let Some(saved) = store.code_explanation_for(&key, "symbol", &hash)? {
        let content = serde_json::from_str(&saved.content).unwrap_or(Value::Null);
        return Ok(SymbolGuide { explanation: saved, content, stale: false });
    }

    // 1. Cada método, por separado (o el símbolo entero si no tiene).
    let mut members = members_of(&symbols, &target);
    let skipped = members.len().saturating_sub(MAX_MEMBERS);
    members.truncate(MAX_MEMBERS);
    let parts: Vec<&Symbol> = if members.is_empty() { vec![&target] } else { members.iter().collect() };
    let summaries = summarize_all(store, model, path, &source, &parts, progress)?;

    // 2. La clase entera.
    let (request, tests) = guide_request(store, sources, path, &source, &target, &members, &summaries, skipped)?;
    let request = huella_core::redact::redact(&clip(&request, MAX_CHARS)).into_owned();
    let text = model
        .complete_json(GUIDE_SYSTEM, &request, &guide_schema())
        .map_err(|error| EngineError::Model(error.to_string()))?;
    let mut content: Value = serde_json::from_str(&text).map_err(|error| EngineError::Model(error.to_string()))?;
    if content["purpose"].as_str().is_none_or(|purpose| purpose.trim().is_empty()) {
        return Err(EngineError::Model("el modelo devolvió una guía vacía".into()));
    }
    let total = source.lines().count();
    // Las líneas, dentro de la clase y, si se sabe de qué método son, dentro de ese método:
    // un rango que abarca media clase es un error del modelo, no una regla.
    let ranges_of = |method: &str| -> Vec<(usize, usize)> {
        summaries.iter().filter(|(name, _, _)| short_name(name) == method).map(|(_, lines, _)| *lines).collect()
    };
    for (list, field) in [("map", "method"), ("rules", "method")] {
        if let Some(items) = content[list].as_array_mut() {
            for item in items {
                let ranges = ranges_of(item[field].as_str().unwrap_or_default());
                let lines = valid_lines(&item["lines"], (target.start_line, target.end_line), total);
                let inside = |lines: &Value| {
                    let (Some(first), Some(last)) = (lines[0].as_u64(), lines[1].as_u64()) else { return true };
                    ranges.is_empty()
                        || ranges.iter().any(|&(start, end)| {
                            usize::try_from(first).is_ok_and(|f| f >= start)
                                && usize::try_from(last).is_ok_and(|l| l <= end)
                        })
                };
                item["lines"] = match (list, ranges.as_slice()) {
                    // Un caso de uso es su método entero: sus líneas se saben sin el modelo.
                    ("map", [(start, end)]) => json!([start, end]),
                    // Sobrecargas: la que contiene lo que dijo el modelo o, si no, la más larga.
                    ("map", [_, _, ..]) => {
                        let said = lines[0].as_u64().and_then(|n| usize::try_from(n).ok());
                        let chosen = ranges
                            .iter()
                            .find(|&&(start, end)| said.is_some_and(|line| (start..=end).contains(&line)))
                            .or_else(|| ranges.iter().max_by_key(|&&(start, end)| end - start));
                        chosen.map_or(Value::Null, |&(start, end)| json!([start, end]))
                    }
                    _ if inside(&lines) => lines,
                    _ => Value::Null,
                };
            }
        }
    }
    complete_rules(&mut content, &summaries);
    content["tests_found"] = json!(
        tests.iter().map(|t| json!({"name": t.name, "path": t.display_path, "line": t.start_line})).collect::<Vec<_>>()
    );
    content["methods"] = json!(
        summaries
            .iter()
            .map(|(name, (start, end), _)| json!({"name": name, "lines": [start, end]}))
            .collect::<Vec<_>>()
    );

    let model_id = model.model_id();
    let id = store.add_code_explanation(&NewCodeExplanation {
        path: &key,
        kind: "symbol",
        symbol: Some(&target.qualified_name),
        start_line: target.start_line,
        end_line: target.end_line,
        quote: &code,
        code_hash: &hash,
        content: &content.to_string(),
        model: &model_id,
    })?;
    let explanation = store
        .code_explanations(&key)?
        .into_iter()
        .find(|explanation| explanation.id == id)
        .ok_or_else(|| EngineError::Model("no se encontró la guía recién guardada".into()))?;
    Ok(SymbolGuide { explanation, content, stale: false })
}

/// Lo que ve el modelo para escribir la guía: el esqueleto, los resúmenes de
/// los métodos, los tests, quién la usa, la historia y lo declarado.
#[allow(clippy::too_many_arguments)]
fn guide_request(
    store: &Store,
    sources: Option<&crate::Sources>,
    path: &Path,
    source: &str,
    target: &Symbol,
    members: &[Symbol],
    summaries: &[Summary],
    skipped: usize,
) -> Result<(String, Vec<crate::explain::TestRef>)> {
    let mut request = String::new();
    let _ = writeln!(
        request,
        "# `{}` ({}) en `{}`, líneas {}–{}\n",
        target.qualified_name,
        kind_name(target.kind),
        path.display(),
        target.start_line,
        target.end_line
    );
    if !members.is_empty() {
        request.push_str("## Esqueleto (lo que no está dentro de los métodos)\n\n```\n");
        request.push_str(&skeleton(source, target, members));
        request.push_str("```\n\n");
    }
    request.push_str("## Métodos\n\n");
    for (name, (start, end), summary) in summaries {
        let _ = writeln!(request, "### `{name}` (líneas {start}–{end})\n{summary}\n");
    }
    if skipped > 0 {
        let _ = writeln!(request, "Y {skipped} métodos más que no se resumen aquí.\n");
    }

    let mut names: Vec<String> = vec![short_name(&target.qualified_name).to_owned()];
    names.extend(members.iter().map(|member| short_name(&member.qualified_name).to_owned()));
    names.retain(|name| name.len() > 2 && crate::definition::is_identifier(name));
    names.dedup();
    names.truncate(10);
    // Todos los de la clase, cortos: con pocos, las reglas parecen sin test aunque lo tengan.
    let tests = tests_naming(path, &names, TestBudget { tests: MAX_TESTS, lines: TEST_LINES });
    request.push_str("## Tests que la nombran\n\n");
    if tests.is_empty() {
        request.push_str("No se encontró ningún test que la nombre.\n\n");
    }
    for test in &tests {
        let _ = writeln!(
            request,
            "### {} (`{}`:{})\n```\n{}\n```\n",
            test.name, test.display_path, test.start_line, test.code
        );
    }

    let callers = callers(path, short_name(&target.qualified_name));
    if !callers.is_empty() {
        let _ = writeln!(request, "## Quién la usa (fuera de su fichero y de los tests)\n\n{}\n", callers.join("\n"));
    }

    request.push_str("## Historia en Huella\n\n");
    let mut told = Vec::new();
    let mut activity: HashMap<String, Vec<huella_core::Activity>> = HashMap::new();
    let lines_to_ask = std::iter::once(target.start_line).chain(members.iter().map(|m| m.start_line));
    for line in lines_to_ask {
        if told.len() > MAX_MEMBER_HISTORIES {
            break;
        }
        if let Some(memory) = memory_at(store, path, source, line)?
            && !told.contains(&memory.symbol.qualified_name)
            && !memory.entries.is_empty()
        {
            told.push(memory.symbol.qualified_name.clone());
            write_history(&mut request, &memory, sources, &mut activity)?;
        }
    }
    if told.is_empty() {
        request
            .push_str("Huella no tiene historia de este código: no lo escribió ningún agente o no se ha indexado.\n\n");
    }
    let declared = declared_in(store, path, source, target)?;
    if !declared.is_empty() {
        let _ = writeln!(request, "## Lo que declaró el agente sobre este código\n\n{declared}");
    }

    Ok((request, tests))
}

/// El resumen de un método: su nombre, sus líneas y el resumen en JSON.
type Summary = (String, (usize, usize), String);

/// Líneas de código por llamada al resumir métodos: los pequeños van juntos,
/// para no gastar una llamada en un método de tres líneas.
const BATCH_LINES: usize = 100;
const BATCH_METHODS: usize = 6;

/// El resumen de cada método, en su orden: los guardados se reutilizan (por el
/// hash de su código) y los demás se piden en tandas.
fn summarize_all(
    store: &Store,
    model: &dyn JsonModel,
    path: &Path,
    source: &str,
    parts: &[&Symbol],
    progress: &dyn Fn(usize, usize),
) -> Result<Vec<Summary>> {
    let key = normalize_path(path, None);
    let mut summaries: Vec<Option<String>> = Vec::with_capacity(parts.len());
    for part in parts {
        let saved = store.code_explanation_for(&key, "method", &code_hash(&code_of(source, part)))?;
        summaries.push(saved.map(|saved| saved.content));
    }
    let pending: Vec<usize> = (0..parts.len()).filter(|&index| summaries[index].is_none()).collect();
    let mut done = parts.len() - pending.len();
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut lines = 0;
    for index in pending {
        let size = (parts[index].end_line - parts[index].start_line + 1).min(MAX_MEMBER_LINES);
        let full = batches.last().is_none_or(|batch| batch.len() >= BATCH_METHODS || lines + size > BATCH_LINES);
        if full {
            batches.push(Vec::new());
            lines = 0;
        }
        lines += size;
        if let Some(batch) = batches.last_mut() {
            batch.push(index);
        }
    }
    for batch in batches {
        progress(done, parts.len());
        let batch_parts: Vec<&Symbol> = batch.iter().map(|&index| parts[index]).collect();
        let mut answers = summarize_batch(model, path, source, &batch_parts)?;
        for (position, &index) in batch.iter().enumerate() {
            // Si el modelo se saltó alguno, ese va solo.
            let summary = match answers.get_mut(position).and_then(Option::take) {
                Some(summary) => summary,
                None => {
                    summarize_batch(model, path, source, &[parts[index]])?.into_iter().next().flatten().ok_or_else(
                        || EngineError::Model(format!("el modelo no resumió {}", parts[index].qualified_name)),
                    )?
                }
            };
            let content = summary.to_string();
            store.add_code_explanation(&NewCodeExplanation {
                path: &key,
                kind: "method",
                symbol: Some(&parts[index].qualified_name),
                start_line: parts[index].start_line,
                end_line: parts[index].end_line,
                quote: &code_of(source, parts[index]),
                code_hash: &code_hash(&code_of(source, parts[index])),
                content: &content,
                model: &model.model_id(),
            })?;
            summaries[index] = Some(content);
        }
        done += batch.len();
    }
    progress(parts.len(), parts.len());
    Ok(parts
        .iter()
        .zip(summaries)
        .map(|(part, summary)| {
            (part.qualified_name.clone(), (part.start_line, part.end_line), summary.unwrap_or_default())
        })
        .collect())
}

/// Resume varios métodos en una llamada; cada uno vuelve en su posición (o
/// `None` si el modelo no lo devolvió).
fn summarize_batch(model: &dyn JsonModel, path: &Path, source: &str, parts: &[&Symbol]) -> Result<Vec<Option<Value>>> {
    let mut request = format!("# Métodos de `{}`\n\n", path.display());
    for (index, part) in parts.iter().enumerate() {
        let _ = writeln!(request, "## {} · `{}`\n```", index + 1, part.qualified_name);
        for (offset, line) in source
            .lines()
            .skip(part.start_line - 1)
            .take((part.end_line - part.start_line + 1).min(MAX_MEMBER_LINES))
            .enumerate()
        {
            let _ = writeln!(request, "{:>5}  {}", part.start_line + offset, crate::storyline::clip_line(line));
        }
        request.push_str("```\n\n");
    }
    let request = huella_core::redact::redact(&request).into_owned();
    let text = match model.complete_json(MEMBER_SYSTEM, &request, &member_schema()) {
        Ok(text) => text,
        // La respuesta no cupo (el razonamiento cuenta en el límite): la tanda, en dos mitades.
        Err(ExtractError::Truncated) if parts.len() > 1 => {
            let (first, second) = parts.split_at(parts.len() / 2);
            let mut answers = summarize_batch(model, path, source, first)?;
            answers.extend(summarize_batch(model, path, source, second)?);
            return Ok(answers);
        }
        Err(error) => return Err(EngineError::Model(error.to_string())),
    };
    let parsed: Value = serde_json::from_str(&text).map_err(|error| EngineError::Model(error.to_string()))?;
    let mut answers: Vec<Option<Value>> = vec![None; parts.len()];
    for mut method in parsed["methods"].as_array().cloned().unwrap_or_default() {
        let position = method["index"].as_u64().and_then(|n| usize::try_from(n).ok()).and_then(|n| n.checked_sub(1));
        if let Some(slot) = position.and_then(|position| answers.get_mut(position)) {
            if let Some(object) = method.as_object_mut() {
                object.remove("index");
            }
            *slot = Some(method);
        }
    }
    Ok(answers)
}

fn symbols_of(path: &Path, source: &str) -> Vec<Symbol> {
    Language::from_path(path)
        .and_then(|language| extract_symbols(language, source).ok())
        .map(|extraction| extraction.symbols)
        .unwrap_or_default()
}

fn find_symbol(path: &Path, source: &str, name: &str) -> Option<Symbol> {
    symbols_of(path, source).into_iter().find(|s| s.qualified_name == name)
}

fn code_of(source: &str, symbol: &Symbol) -> String {
    source
        .lines()
        .skip(symbol.start_line - 1)
        .take(symbol.end_line - symbol.start_line + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Los métodos y funciones de primer nivel dentro de `target`: no los que
/// están dentro de otro método.
fn members_of(symbols: &[Symbol], target: &Symbol) -> Vec<Symbol> {
    let inside = |s: &Symbol, outer: &Symbol| {
        s.start_line >= outer.start_line
            && s.end_line <= outer.end_line
            && (s.start_line, s.end_line) != (outer.start_line, outer.end_line)
    };
    let callable = |s: &Symbol| matches!(s.kind, SymbolKind::Method | SymbolKind::Function);
    let candidates: Vec<&Symbol> = symbols.iter().filter(|s| callable(s) && inside(s, target)).collect();
    candidates.iter().filter(|s| !candidates.iter().any(|outer| inside(s, outer))).map(|s| (*s).clone()).collect()
}

/// Métodos más cortos no aportan reglas propias al completar la guía.
const MIN_RULE_METHOD_LINES: usize = 6;

/// Añade a la guía las reglas de los resúmenes de los métodos que la síntesis
/// no cubre (ninguna de sus reglas toca esas líneas): el modelo tiende a quedarse
/// con las que tienen test, y las que no lo tienen son las que más importan.
fn complete_rules(content: &mut Value, summaries: &[Summary]) {
    let mut covered: Vec<(u64, u64)> = content["rules"]
        .as_array()
        .map(|rules| {
            rules.iter().filter_map(|rule| Some((rule["lines"][0].as_u64()?, rule["lines"][1].as_u64()?))).collect()
        })
        .unwrap_or_default();
    let mut missing = Vec::new();
    for (name, (start, end), summary) in summaries {
        // Un método de pocas líneas no tiene reglas que contar: delega o construye.
        if end - start + 1 < MIN_RULE_METHOD_LINES {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(summary) else { continue };
        for rule in parsed["rules"].as_array().into_iter().flatten() {
            let (Some(first), Some(last)) = (rule["lines"][0].as_u64(), rule["lines"][1].as_u64()) else { continue };
            let inside = usize::try_from(first).is_ok_and(|first| first >= *start)
                && usize::try_from(last).is_ok_and(|last| last <= *end);
            let when = rule["when"].as_str().unwrap_or_default().to_lowercase();
            let filler = rule["kind"] == "other"
                || ["por defecto", "estándar", "sin condiciones"].iter().any(|w| when.contains(w));
            if !inside || filler || covered.iter().any(|&(a, b)| first <= b && a <= last) {
                continue;
            }
            covered.push((first, last));
            missing.push(json!({
                "kind": rule["kind"].as_str().unwrap_or("other"),
                "when": rule["when"].as_str().unwrap_or_default(),
                "then": rule["then"].as_str().unwrap_or_default(),
                "why": "",
                "method": name.rsplit(['.', ':']).next().unwrap_or(name),
                "lines": [first, last],
                "tests": [],
            }));
        }
    }
    if let Some(rules) = content["rules"].as_array_mut() {
        rules.extend(missing);
        rules.sort_by_key(|rule| rule["lines"][0].as_u64().unwrap_or(u64::MAX));
    }
}

/// La clase sin el cuerpo de sus métodos: campos, constructor y firmas.
fn skeleton(source: &str, target: &Symbol, members: &[Symbol]) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::new();
    let mut written = 0;
    let mut number = target.start_line;
    while number <= target.end_line && written < MAX_SKELETON_LINES {
        if let Some(member) = members.iter().find(|m| m.start_line == number) {
            // La firma (primera línea) se queda; el cuerpo, no.
            let _ = writeln!(out, "{number:>5}  {}", crate::storyline::clip_line(lines[number - 1]));
            let _ = writeln!(out, "       … {} líneas", member.end_line - member.start_line);
            number = member.end_line + 1;
        } else {
            if !lines[number - 1].trim().is_empty() {
                let _ = writeln!(out, "{number:>5}  {}", crate::storyline::clip_line(lines[number - 1]));
            }
            number += 1;
        }
        written += 1;
    }
    out
}

/// Usos del nombre fuera de su fichero y de los tests, con su línea.
fn callers(path: &Path, name: &str) -> Vec<String> {
    if !crate::definition::is_identifier(name) {
        return Vec::new();
    }
    let dir = path.parent().unwrap_or(path);
    let Ok(root) = git(dir, &["rev-parse", "--show-toplevel"]) else { return Vec::new() };
    let root = Path::new(root.trim());
    let own = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let Ok(found) = git(root, &["grep", "-n", "-w", "-I", "-F", "-e", name]) else { return Vec::new() };
    found
        .lines()
        .filter(|line| {
            let file = line.split(':').next().unwrap_or_default();
            let lower = file.to_lowercase();
            root.join(file).canonicalize().is_ok_and(|full| full != own)
                && !(lower.contains("test") || lower.contains("spec"))
        })
        .take(MAX_CALLERS)
        .map(|line| format!("- {}", crate::storyline::clip_line(line)))
        .collect()
}

fn declared_in(store: &Store, path: &Path, source: &str, target: &Symbol) -> Result<String> {
    let normalized = normalize_path(&std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()), None);
    let views: Vec<crate::declared::DeclaredView> = store
        .declarations(&normalized)?
        .into_iter()
        .filter(|d| d.start_line <= target.end_line && target.start_line <= d.end_line)
        .take(12)
        .map(|declaration| {
            let text = source
                .lines()
                .skip(declaration.start_line.saturating_sub(1))
                .take(declaration.end_line.saturating_sub(declaration.start_line) + 1)
                .collect::<Vec<_>>()
                .join("\n");
            crate::declared::view(declaration, &text)
        })
        .collect();
    Ok(if views.is_empty() { String::new() } else { crate::declared::render(&views) })
}

fn valid_lines(value: &Value, (start, end): (usize, usize), total: usize) -> Value {
    let numbers: Vec<usize> = value
        .as_array()
        .map(|lines| lines.iter().filter_map(Value::as_u64).filter_map(|n| usize::try_from(n).ok()).collect())
        .unwrap_or_default();
    match (numbers.first(), numbers.last()) {
        (Some(&first), Some(&last)) => {
            let (first, last) = (first.min(last), first.max(last));
            if first >= start && last <= end && last <= total { json!([first, last]) } else { Value::Null }
        }
        _ => Value::Null,
    }
}

fn short_name(qualified: &str) -> &str {
    qualified.rsplit(['.', ':']).next().unwrap_or(qualified)
}

fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Class => "clase",
        SymbolKind::Struct => "struct",
        SymbolKind::Impl => "impl",
        SymbolKind::Trait => "trait",
        SymbolKind::Interface => "interfaz",
        SymbolKind::Module => "módulo",
        SymbolKind::Method => "método",
        _ => "función",
    }
}

fn lines_schema() -> Value {
    json!({"type": ["array", "null"], "items": {"type": "integer"}})
}

fn rule_schema(extra: &[(&str, Value)]) -> Value {
    let mut properties = json!({
        "kind": {"type": "string", "enum": ["validation", "invariant", "error", "calculation", "state", "permission", "other"]},
        "when": {"type": "string"},
        "then": {"type": "string"},
        "lines": lines_schema(),
    });
    let mut required = vec!["kind", "when", "then", "lines"];
    for (name, schema) in extra {
        properties[*name] = schema.clone();
        required.push(name);
    }
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn strings() -> Value {
    json!({"type": "array", "items": {"type": "string"}})
}

fn member_schema() -> Value {
    json!({"type": "object", "properties": {"methods": {"type": "array", "items": method_schema()}}, "required": ["methods"], "additionalProperties": false})
}

fn method_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "index": {"type": "integer"},
            "purpose": {"type": "string"},
            "inputs": {"type": "string"},
            "returns": {"type": "string"},
            "effects": strings(),
            "calls": strings(),
            "rules": {"type": "array", "items": rule_schema(&[])},
        },
        "required": ["index", "purpose", "inputs", "returns", "effects", "calls", "rules"],
        "additionalProperties": false
    })
}

fn object(fields: &[(&str, Value)]) -> Value {
    let properties: serde_json::Map<String, Value> = fields.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
    let required: Vec<&str> = fields.iter().map(|(k, _)| *k).collect();
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn list_of(fields: &[(&str, Value)]) -> Value {
    json!({"type": "array", "items": object(fields)})
}

fn guide_schema() -> Value {
    let text = || json!({"type": "string"});
    object(&[
        ("purpose", text()),
        ("glossary", list_of(&[("term", text()), ("meaning", text())])),
        ("map", list_of(&[("method", text()), ("use_case", text()), ("lines", lines_schema())])),
        ("dependencies", list_of(&[("name", text()), ("role", text())])),
        ("state", strings()),
        (
            "rules",
            json!({"type": "array", "items": rule_schema(&[("why", text()), ("method", text()), ("tests", strings())])}),
        ),
        ("flows", list_of(&[("title", text()), ("method", text()), ("steps", strings())])),
        (
            "why",
            list_of(&[
                ("text", text()),
                ("source", json!({"type": "string", "enum": ["history", "declared", "code"]})),
            ]),
        ),
        ("doubts", strings()),
        ("reading_order", list_of(&[("method", text()), ("why", text())])),
        ("questions", list_of(&[("question", text()), ("answer", text())])),
    ])
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Responde según qué se le pida: resumir un método o escribir la guía.
    struct Fake {
        calls: RefCell<Vec<(bool, String)>>,
    }

    impl JsonModel for Fake {
        fn model_id(&self) -> String {
            "fake:1".into()
        }

        fn complete_json(
            &self,
            system: &str,
            user: &str,
            _schema: &Value,
        ) -> std::result::Result<String, ExtractError> {
            let member = system == MEMBER_SYSTEM;
            self.calls.borrow_mut().push((member, user.to_owned()));
            Ok(if member {
                // Uno por cada método numerado («## 1 · `…`»).
                let count = user.matches("\n## ").count() + usize::from(user.starts_with("## "));
                let methods: Vec<Value> = (1..=count)
                    .map(|index| {
                        let rules = if user.contains("liberar") && index == count {
                            json!([{"kind": "calculation", "when": "siempre", "then": "suma", "lines": [7, 7]}])
                        } else {
                            json!([])
                        };
                        json!({"index": index, "purpose": "Reserva.", "inputs": "", "returns": "", "effects": [], "calls": [], "rules": rules})
                    })
                    .collect();
                json!({ "methods": methods }).to_string()
            } else {
                json!({
                    "purpose": "Gestiona las reservas de stock.",
                    "glossary": [], "map": [{"method": "reservar", "use_case": "Reservar unidades", "lines": [2, 6]}],
                    "dependencies": [], "state": [],
                    "rules": [{"kind": "error", "when": "más que el stock", "then": "SinStock", "why": "", "method": "reservar",
                               "lines": [400, 401], "tests": ["sin stock falla"]},
                              {"kind": "error", "when": "abarca media clase", "then": "x", "why": "", "method": "reservar",
                               "lines": [2, 9], "tests": []}],
                    "flows": [], "why": [], "doubts": [], "reading_order": [], "questions": []
                })
                .to_string()
            })
        }
    }

    const SERVICE: &str = "\
export class Pedidos {
  reservar(cantidad: number, stock: number) {
    if (cantidad > stock) throw new Error('SinStock')
    return stock - cantidad
  }
  liberar(cantidad: number, stock: number) {
    if (cantidad < 0) {
      return stock
    }
    const total = stock + cantidad
    return total
  }
}
";

    fn repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-guide-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/pedidos.ts"), SERVICE).unwrap();
        std::fs::write(dir.join("src/tienda.ts"), "const p = new Pedidos()\n").unwrap();
        std::fs::write(
            dir.join("src/pedidos.test.ts"),
            "test('sin stock falla', () => {\n  new Pedidos().reservar(3, 2)\n})\n",
        )
        .unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        git(&dir, &["add", "."]).unwrap();
        dir
    }

    #[test]
    fn summarizes_each_method_then_the_class_and_reuses_unchanged_methods() {
        let dir = repo("class");
        let path = dir.join("src/pedidos.ts");
        let store = Store::open_in_memory().unwrap();
        let model = Fake { calls: RefCell::new(Vec::new()) };
        let seen = RefCell::new(Vec::new());
        let ask = UnderstandSymbol { path: &path, symbol: "Pedidos", reviewed: None, refresh: false };

        let guide =
            understand_symbol(&store, None, &model, ask, &|done, total| seen.borrow_mut().push((done, total))).unwrap();
        assert_eq!(guide.content["purpose"], "Gestiona las reservas de stock.");
        assert_eq!(guide.content["map"][0]["lines"], json!([2, 5]), "las del método, no las que diga el modelo");
        let rules = guide.content["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 3, "la regla que la síntesis olvidó se añade: {rules:?}");
        assert!(
            rules.iter().any(|r| r["when"] == "abarca media clase" && r["lines"].is_null()),
            "fuera de su método: {rules:?}"
        );
        assert_eq!((rules[0]["lines"].clone(), rules[0]["method"].clone()), (json!([7, 7]), json!("liberar")));
        assert_eq!(rules[1]["lines"], Value::Null, "líneas fuera de la clase se quitan");
        assert_eq!(guide.content["methods"].as_array().unwrap().len(), 2);
        assert_eq!(*seen.borrow(), vec![(0, 2), (2, 2)]);
        let calls = model.calls.borrow().clone();
        assert_eq!(calls.iter().filter(|(member, _)| *member).count(), 1, "los métodos pequeños van en una tanda");
        let (_, synthesis) = calls.last().unwrap();
        assert!(synthesis.contains("Esqueleto") && synthesis.contains("… 3 líneas"), "{synthesis}");
        assert!(synthesis.contains("sin stock falla"), "los tests que la nombran");
        assert!(synthesis.contains("tienda.ts"), "quién la usa");
        assert!(!synthesis.contains("pedidos.test.ts:2"), "los tests no cuentan como usos");

        // Sin cambios: la guía se reutiliza. Cambia un método: solo él se vuelve a resumir.
        understand_symbol(&store, None, &model, ask, &|_, _| {}).unwrap();
        assert_eq!(model.calls.borrow().len(), 2);
        let changed = SERVICE.replace("stock + cantidad", "stock + cantidad + 0");
        std::fs::write(&path, &changed).unwrap();
        let saved = saved_guide(&store, &path, "Pedidos", &changed).unwrap().unwrap();
        assert!(saved.stale, "el código cambió desde la guía");
        understand_symbol(&store, None, &model, ask, &|_, _| {}).unwrap();
        let calls = model.calls.borrow();
        assert_eq!(calls.len(), 4, "solo el método que cambió y la síntesis: {}", calls.len());
        assert!(calls[2].1.contains("liberar") && !calls[2].1.contains("reservar"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// No le caben dos métodos en la respuesta.
    struct Short {
        calls: RefCell<Vec<usize>>,
    }

    impl JsonModel for Short {
        fn model_id(&self) -> String {
            "fake:1".into()
        }

        fn complete_json(
            &self,
            system: &str,
            user: &str,
            _schema: &Value,
        ) -> std::result::Result<String, ExtractError> {
            if system != MEMBER_SYSTEM {
                return Ok(json!({"purpose": "Reservas.", "glossary": [], "map": [], "dependencies": [], "state": [], "rules": [],
                    "flows": [], "why": [], "doubts": [], "reading_order": [], "questions": []}).to_string());
            }
            let count = user.matches("\n## ").count();
            self.calls.borrow_mut().push(count);
            if count > 1 {
                return Err(ExtractError::Truncated);
            }
            Ok(json!({"methods": [{"index": 1, "purpose": "x", "inputs": "", "returns": "", "effects": [], "calls": [], "rules": []}]}).to_string())
        }
    }

    #[test]
    fn a_batch_that_does_not_fit_is_split_in_halves() {
        let dir = repo("split");
        let path = dir.join("src/pedidos.ts");
        let store = Store::open_in_memory().unwrap();
        let model = Short { calls: RefCell::new(Vec::new()) };
        let ask = UnderstandSymbol { path: &path, symbol: "Pedidos", reviewed: None, refresh: false };
        let guide = understand_symbol(&store, None, &model, ask, &|_, _| {}).unwrap();
        assert_eq!(*model.calls.borrow(), vec![2, 1, 1]);
        assert_eq!(guide.content["methods"].as_array().unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_agent_guide_is_checked_against_the_code() {
        let dir = repo("told");
        let path = dir.join("src/pedidos.ts");
        let store = Store::open_in_memory().unwrap();
        let told = json!({
            "purpose": "Gestiona las reservas de stock.",
            "map": [{"method": "reservar", "use_case": "Reservar unidades"}],
            "rules": [
                {"kind": "error", "when": "más que el stock", "then": "SinStock", "why": "", "method": "reservar",
                 "quote": "if (cantidad > stock) throw new Error('SinStock')", "tests": ["sin stock falla"]},
                {"kind": "validation", "when": "cantidad negativa", "then": "nada", "why": "", "method": "liberar",
                 "quote": "una línea que no está", "tests": ["un test inventado"]},
                {"kind": "other", "when": "x", "then": "y", "why": "", "method": "noExiste", "quote": "", "tests": []}
            ]
        });
        let told = record_told(&store, &path, "Pedidos", told).unwrap();
        let rules = told.guide.content["rules"].as_array().unwrap();
        assert_eq!(rules[0]["lines"], json!([3, 3]), "la regla, en la línea de su quote");
        assert_eq!(told.guide.content["map"][0]["lines"], json!([2, 5]));
        assert_eq!(told.guide.explanation.model, "agent");
        let problems = told.problems.join(" | ");
        assert!(problems.contains("regla 2") && problems.contains("un test inventado"), "{problems}");
        assert!(problems.contains("`noExiste` no es un método"), "{problems}");
        assert!(!problems.contains("regla 1"), "{problems}");
        // Se ve como la guía del símbolo.
        let saved = saved_guide(&store, &path, "Pedidos", &std::fs::read_to_string(&path).unwrap()).unwrap().unwrap();
        assert_eq!(saved.explanation.model, "agent");
        assert!(record_told(&store, &path, "Pedidos", json!({"purpose": " "})).is_err(), "sin para qué existe no vale");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_function_without_members_is_summarized_whole() {
        let dir = repo("function");
        let path = dir.join("src/pedidos.ts");
        let store = Store::open_in_memory().unwrap();
        let model = Fake { calls: RefCell::new(Vec::new()) };
        let ask = UnderstandSymbol { path: &path, symbol: "Pedidos.reservar", reviewed: None, refresh: false };
        let guide = understand_symbol(&store, None, &model, ask, &|_, _| {}).unwrap();
        assert_eq!(guide.explanation.symbol.as_deref(), Some("Pedidos.reservar"));
        assert!(model.calls.borrow()[0].1.contains("Pedidos.reservar"));
        assert!(matches!(
            understand_symbol(&store, None, &model, UnderstandSymbol { symbol: "NoExiste", ..ask }, &|_, _| {}),
            Err(EngineError::Model(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
