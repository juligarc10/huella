//! Recorrido guiado: el cambio contado como lo contaría alguien a tu lado con
//! un depurador. Un caso concreto con datos recorre el código en el orden en
//! que se ejecuta; en cada parada, qué llega, qué pasa, qué sale y por qué, con
//! una transición desde la anterior y los valores que toman las líneas.
//!
//! Es otra forma de leer la revisión, no sustituye a las ideas del guion: lo
//! que el recorrido no toca queda en un apéndice.
//!
//! Se escribe en dos llamadas para que quepa en un modelo local aunque el
//! cambio tenga cientos de pasos: con un índice compacto, el modelo elige el
//! caso y las paradas; con el código solo de esas paradas, las cuenta. Lo que
//! el modelo no puede inventar se comprueba aquí: que cada valor cite una línea
//! que existe y que entre dos paradas seguidas haya una llamada que las una.

use std::fmt::Write as _;

use huella_decisions::JsonModel;
use huella_review::flow::{FlowStep, StepRole};
use huella_review::{DiffLine, SessionReview};
use huella_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::storyline::clip_line;
use crate::{EngineError, Result};

/// Paradas del recorrido: menos no cuenta nada; más se convierte en un catálogo.
const MIN_STOPS: usize = 2;
const MAX_STOPS: usize = 12;
/// Líneas de código por parada que se envían para contarla.
const STOP_LINES: usize = 80;

const PICK_SYSTEM: &str = "\
Preparas un recorrido guiado por un cambio de código: un caso concreto que se sigue por el código en el orden en \
que se ejecuta, como lo enseñaría alguien con un depurador a quien no conoce el código.

Recibes lo que se pidió y un índice de los pasos del cambio (número, fichero, símbolo, tipo, líneas añadidas y \
quitadas, y a qué otros pasos llama). Elige:
- `title` y `situation`: el caso, realista y con datos concretos (quién hace qué, con qué valores). El que mejor \
  enseña el cambio: el que pasa por la lógica nueva importante, no el trivial.
- `stops`: los números de los pasos por los que pasa ese caso, en orden de ejecución: de la entrada (endpoint, \
  pantalla, comando, evento, test) a la lógica, los datos y los efectos. Entre 3 y 12. Sigue las llamadas (\"llama \
  a\"): si un paso llama a otro, el llamado va justo después, salvo que el caso no pase por él. Sin tests ni \
  configuración, salvo que sean la entrada del caso.
- Escribe en el idioma de lo que se pidió.";

const TELL_SYSTEM: &str = "\
Cuentas un recorrido guiado por un cambio de código: el caso que se te da, parada a parada, en orden, para alguien \
que no conoce ni el código ni el dominio. Que al leerlo de corrido sienta que alguien le va explicando y vea cómo \
funciona de verdad.

Para cada parada (`step` es su número):
- `title`: qué ocurre aquí, en pocas palabras.
- `transition`: una frase que enlaza con la parada anterior (qué trae de allí y por qué llega aquí). En la primera, \
  cómo empieza el caso.
- `arrives`: qué llega, con los valores del caso (\"un pedido de 3 unidades; stock = 2\").
- `does`: qué hace este código con eso y por qué, en dos o cuatro frases. Nombra funciones y variables.
- `leaves`: qué sale o qué cambia (valor devuelto, estado, efecto), con valores.
- `why`: por qué se hace así, si consta en lo declarado o se deduce claramente; vacío si no.
- `values`: de 0 a 6 líneas clave del código de esa parada con el valor que toman en el caso: `quote` es el texto \
  exacto de una línea del código que se te da (sin el + ni el número), `value` lo que vale o decide ahí \
  (\"roll = 20 → crítico\").

`forks`: de 0 a 4 bifurcaciones que importan del camino: `after_step` (la parada donde se decide), `condition` \
(qué tendría que ser distinto) y `outcome` (qué pasaría entonces y por dónde seguiría).

No inventes llamadas ni valores que el código no permita. Escribe en el idioma de lo que se pidió.";

/// Lo que se le cuenta al modelo además de los pasos.
#[derive(Debug, Clone, Copy)]
pub struct WalkInput<'a> {
    /// Lo que se pidió: la descripción de la tarea o las peticiones de la sesión.
    pub intent: &'a str,
    /// Pistas del guion, si lo hay (su resumen y su ejemplo de principio a fin).
    pub hints: &'a str,
}

/// Un valor del caso en una línea del código.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineValue {
    pub quote: String,
    pub value: String,
}

/// Una parada del recorrido.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    /// Índice del paso en el plan.
    pub step: usize,
    pub title: String,
    pub transition: String,
    pub arrives: String,
    pub does: String,
    pub leaves: String,
    pub why: String,
    pub values: Vec<LineValue>,
    /// Si no se ve una llamada que traiga aquí desde las paradas anteriores, por qué.
    #[serde(default)]
    pub gap: Option<String>,
}

/// Lo que pasaría si el caso fuera otro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fork {
    /// Índice del paso en el plan donde se decide.
    pub step: usize,
    pub condition: String,
    pub outcome: String,
}

/// El recorrido guiado de una revisión.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Walkthrough {
    pub title: String,
    pub situation: String,
    pub stops: Vec<Stop>,
    pub forks: Vec<Fork>,
    /// Pasos de código que el recorrido no toca: el apéndice.
    pub outside: Vec<usize>,
}

/// Un recorrido guardado, con si el cambio ha cambiado desde entonces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredWalkthrough {
    pub walkthrough: Walkthrough,
    pub model: String,
    pub recorded_at: String,
    pub stale: bool,
}

/// El recorrido guardado de una revisión, si lo hay.
///
/// # Errors
/// Si falla la base de datos.
pub fn stored(store: &Store, review: &SessionReview, plan: &[FlowStep]) -> Result<Option<StoredWalkthrough>> {
    let Some(saved) = store.walkthrough(&review.session_id)? else { return Ok(None) };
    let Ok(walkthrough) = serde_json::from_str::<Walkthrough>(&saved.content) else { return Ok(None) };
    // Si el cambio avanzó, los números de paso pueden apuntar a otro código: no se enseña como bueno.
    let stale = saved.input_hash != change_hash(review, plan);
    let fits = walkthrough.stops.iter().all(|stop| stop.step < plan.len());
    Ok(fits.then_some(StoredWalkthrough { walkthrough, model: saved.model, recorded_at: saved.recorded_at, stale }))
}

/// Escribe el recorrido guiado de una revisión y lo guarda.
///
/// # Errors
/// Si el cambio no tiene código que recorrer, o fallan el modelo o la base de datos.
pub fn generate(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    input: WalkInput<'_>,
    model: &dyn JsonModel,
    context_tokens: Option<usize>,
) -> Result<Walkthrough> {
    let budget = crate::storyline::request_chars(context_tokens);
    let candidates: Vec<usize> = (0..plan.len()).filter(|&index| plan[index].role != StepRole::Other).collect();
    if candidates.is_empty() {
        return Err(EngineError::Model("el cambio no tiene código que recorrer".into()));
    }

    // 1. El caso y por dónde pasa.
    let pick = pick_request(review, plan, input, &candidates, budget);
    let picked = call(model, PICK_SYSTEM, &pick, &pick_schema())?;
    let stops: Vec<usize> = picked["stops"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .filter_map(|number| usize::try_from(number).ok()?.checked_sub(1))
        .filter(|&index| index < plan.len() && plan[index].role != StepRole::Other)
        .fold(Vec::new(), |mut seen, index| {
            if !seen.contains(&index) {
                seen.push(index);
            }
            seen
        })
        .into_iter()
        .take(MAX_STOPS)
        .collect();
    if stops.len() < MIN_STOPS {
        return Err(EngineError::Model("el modelo no eligió un recorrido con al menos dos paradas".into()));
    }
    let title = picked["title"].as_str().unwrap_or_default().trim().to_owned();
    let situation = picked["situation"].as_str().unwrap_or_default().trim().to_owned();

    // 2. Cada parada contada, con el código solo de esas paradas.
    let tell = tell_request(review, plan, input, &title, &situation, &stops, budget);
    let told = call(model, TELL_SYSTEM, &tell, &tell_schema())?;
    let walkthrough = assemble(plan, title, situation, &stops, &told);

    store.save_walkthrough(
        &review.session_id,
        &model.model_id(),
        &change_hash(review, plan),
        &serde_json::to_string(&walkthrough).map_err(|error| EngineError::Model(error.to_string()))?,
    )?;
    Ok(walkthrough)
}

fn call(model: &dyn JsonModel, system: &str, request: &str, schema: &Value) -> Result<Value> {
    let request = huella_core::redact::redact(request).into_owned();
    let text = model.complete_json(system, &request, schema).map_err(|error| EngineError::Model(error.to_string()))?;
    serde_json::from_str(&text).map_err(|error| EngineError::Model(error.to_string()))
}

/// Junta lo que contó el modelo con lo que se sabe sin él: el orden elegido,
/// los valores que citan líneas de verdad, los saltos y el apéndice.
fn assemble(plan: &[FlowStep], title: String, situation: String, order: &[usize], told: &Value) -> Walkthrough {
    let text = |value: &Value, key: &str| value[key].as_str().unwrap_or_default().trim().to_owned();
    let told_stops: Vec<&Value> = told["stops"].as_array().map(|stops| stops.iter().collect()).unwrap_or_default();
    let mut stops: Vec<Stop> = Vec::new();
    for &step in order {
        let found = told_stops
            .iter()
            .find(|stop| stop["step"].as_u64().and_then(|n| usize::try_from(n).ok()) == Some(step + 1));
        let Some(stop) = found else { continue };
        let values = stop["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| {
                let quote = text(value, "quote");
                let shown = text(value, "value");
                line_of(&plan[step], &quote).map(|line| LineValue { quote: line, value: shown })
            })
            .filter(|value| !value.value.is_empty())
            .take(6)
            .collect();
        stops.push(Stop {
            step,
            title: text(stop, "title"),
            transition: text(stop, "transition"),
            arrives: text(stop, "arrives"),
            does: text(stop, "does"),
            leaves: text(stop, "leaves"),
            why: text(stop, "why"),
            values,
            gap: None,
        });
    }
    mark_gaps(plan, &mut stops);
    let forks = told["forks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|fork| {
            let step = fork["after_step"].as_u64().and_then(|n| usize::try_from(n).ok())?.checked_sub(1)?;
            stops.iter().any(|stop| stop.step == step).then(|| Fork {
                step,
                condition: text(fork, "condition"),
                outcome: text(fork, "outcome"),
            })
        })
        .filter(|fork| !fork.condition.is_empty())
        .take(4)
        .collect();
    let outside = (0..plan.len())
        .filter(|&index| plan[index].role == StepRole::Flow && !stops.iter().any(|stop| stop.step == index))
        .collect();
    Walkthrough { title, situation, stops, forks, outside }
}

/// La línea del paso que cita `quote`: igual, sin espacios de los lados, o que
/// la contiene. Solo líneas que siguen en el código.
fn line_of(step: &FlowStep, quote: &str) -> Option<String> {
    let wanted = quote.trim().trim_start_matches(['+', ' ']).trim();
    if wanted.len() < 3 {
        return None;
    }
    let lines = step.lines.iter().filter_map(|line| match line {
        DiffLine::Added { text, .. } | DiffLine::Context(text) => Some(text.as_str()),
        DiffLine::Removed(_) => None,
    });
    let mut contains = None;
    for line in lines {
        if line.trim() == wanted {
            return Some(line.trim().to_owned());
        }
        if contains.is_none() && line.contains(wanted) {
            contains = Some(line.trim().to_owned());
        }
    }
    contains
}

/// Marca las paradas a las que no se ve llegar: ninguna parada anterior las
/// llama ni son llamadas por la anterior (el camino de vuelta de una llamada).
fn mark_gaps(plan: &[FlowStep], stops: &mut [Stop]) {
    let name = |index: usize| {
        plan[index]
            .symbol
            .as_ref()
            .map_or_else(|| format!("el paso {}", index + 1), |symbol| format!("`{}`", symbol.qualified_name))
    };
    for position in 1..stops.len() {
        let here = stops[position].step;
        let previous = stops[position - 1].step;
        let reached = stops[..position].iter().any(|before| plan[before.step].calls.contains(&here))
            || plan[here].calls.contains(&previous);
        if !reached {
            stops[position].gap = Some(format!(
                "No se ve en el código que {} lleve a {}: el salto lo cuenta el modelo, compruébalo.",
                name(previous),
                name(here)
            ));
        }
    }
}

/// Lo que define el cambio: si cambia, el recorrido puede no valer.
fn change_hash(review: &SessionReview, plan: &[FlowStep]) -> String {
    let mut hasher = blake3::Hasher::new();
    for step in plan {
        hasher.update(review.files[step.file].path.as_bytes());
        for line in &step.lines {
            let text = match line {
                DiffLine::Added { text, .. } | DiffLine::Context(text) | DiffLine::Removed(text) => text,
            };
            hasher.update(text.as_bytes());
            hasher.update(b"\n");
        }
    }
    hasher.finalize().to_hex()[..16].to_owned()
}

fn step_name(review: &SessionReview, step: &FlowStep) -> String {
    let path = &review.files[step.file].path;
    let file = path.rsplit('/').next().unwrap_or(path);
    let symbol = step.symbol.as_ref().map_or("(fuera de funciones)", |symbol| symbol.qualified_name.as_str());
    format!("{file} · {symbol}")
}

fn intent_block(input: WalkInput<'_>) -> String {
    let mut out = format!("# Lo que se pidió\n\n{}\n\n", clip(input.intent.trim(), 6_000));
    if !input.hints.trim().is_empty() {
        let _ = writeln!(out, "# Lo que contó el guion\n\n{}\n", clip(input.hints.trim(), 4_000));
    }
    out
}

fn pick_request(
    review: &SessionReview,
    plan: &[FlowStep],
    input: WalkInput<'_>,
    candidates: &[usize],
    budget: usize,
) -> String {
    let mut out = intent_block(input);
    out.push_str("# Pasos del cambio\n\n");
    for &index in candidates {
        let step = &plan[index];
        let role = if step.role == StepRole::Test { " · test" } else { "" };
        let _ =
            write!(out, "[{}] {}{role} · +{} −{}", index + 1, step_name(review, step), step.added(), step.removed());
        let calls: Vec<String> = step.calls.iter().map(|call| format!("[{}]", call + 1)).collect();
        if !calls.is_empty() {
            let _ = write!(out, " · llama a {}", calls.join(", "));
        }
        out.push('\n');
        if out.len() > budget {
            out.push_str("… (más pasos que no caben)\n");
            break;
        }
    }
    out
}

fn tell_request(
    review: &SessionReview,
    plan: &[FlowStep],
    input: WalkInput<'_>,
    title: &str,
    situation: &str,
    stops: &[usize],
    budget: usize,
) -> String {
    let mut out = intent_block(input);
    let _ = writeln!(out, "# El caso\n\n{title}\n{situation}\n\n# Las paradas, en orden\n");
    let per_stop = (budget.saturating_sub(out.len()) / stops.len().max(1) / 90).clamp(20, STOP_LINES);
    for &index in stops {
        let step = &plan[index];
        let calls: Vec<String> = step.calls.iter().map(|call| format!("[{}]", call + 1)).collect();
        let _ = write!(out, "## [{}] {}", index + 1, step_name(review, step));
        if !calls.is_empty() {
            let _ = write!(out, " · llama a {}", calls.join(", "));
        }
        out.push_str("\n```\n");
        let shown: Vec<&DiffLine> = step.lines.iter().filter(|line| !matches!(line, DiffLine::Removed(_))).collect();
        for line in shown.iter().take(per_stop) {
            if let DiffLine::Added { text, .. } | DiffLine::Context(text) = line {
                let _ = writeln!(out, "{}", clip_line(text));
            }
        }
        if shown.len() > per_stop {
            let _ = writeln!(out, "… ({} líneas más)", shown.len() - per_stop);
        }
        out.push_str("```\n\n");
    }
    out
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

fn pick_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string"},
            "situation": {"type": "string"},
            "stops": {"type": "array", "items": {"type": "integer"}},
        },
        "required": ["title", "situation", "stops"],
        "additionalProperties": false
    })
}

fn tell_schema() -> Value {
    let text = || json!({"type": "string"});
    json!({
        "type": "object",
        "properties": {
            "stops": {"type": "array", "items": {
                "type": "object",
                "properties": {
                    "step": {"type": "integer"},
                    "title": text(), "transition": text(), "arrives": text(), "does": text(), "leaves": text(), "why": text(),
                    "values": {"type": "array", "items": {
                        "type": "object",
                        "properties": {"quote": text(), "value": text()},
                        "required": ["quote", "value"],
                        "additionalProperties": false
                    }},
                },
                "required": ["step", "title", "transition", "arrives", "does", "leaves", "why", "values"],
                "additionalProperties": false
            }},
            "forks": {"type": "array", "items": {
                "type": "object",
                "properties": {"after_step": {"type": "integer"}, "condition": text(), "outcome": text()},
                "required": ["after_step", "condition", "outcome"],
                "additionalProperties": false
            }},
        },
        "required": ["stops", "forks"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};
    use huella_decisions::ExtractError;

    use super::*;

    struct Fake {
        calls: RefCell<Vec<String>>,
        pick: Value,
        tell: Value,
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
            self.calls.borrow_mut().push(user.to_owned());
            Ok(if system == PICK_SYSTEM { self.pick.to_string() } else { self.tell.to_string() })
        }
    }

    const CODE: &str = "\
export function atacar(roll: number) {
  const golpe = resolver(roll)
  return golpe
}
export function resolver(roll: number) {
  if (roll === 20) return 'critico'
  return 'normal'
}
export function registrar(texto: string) {
  console.log(texto)
}
";

    fn review_and_plan() -> (SessionReview, Vec<FlowStep>) {
        let edit = FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "s1".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from("/r/combate.ts"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(CODE.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("s1", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    fn number(plan: &[FlowStep], name: &str) -> usize {
        plan.iter().position(|s| s.symbol.as_ref().is_some_and(|s| s.qualified_name == name)).unwrap() + 1
    }

    #[test]
    fn picks_the_stops_tells_them_and_checks_what_the_model_cannot_invent() {
        let (review, plan) = review_and_plan();
        let (atacar, resolver, registrar) =
            (number(&plan, "atacar"), number(&plan, "resolver"), number(&plan, "registrar"));
        let store = Store::open_in_memory().unwrap();
        let model = Fake {
            calls: RefCell::new(Vec::new()),
            pick: json!({"title": "Un crítico", "situation": "El jugador saca un 20", "stops": [atacar, resolver, registrar, atacar, 999]}),
            tell: json!({
                "stops": [
                    {"step": atacar, "title": "Ataca", "transition": "Empieza el ataque", "arrives": "roll = 20", "does": "…",
                     "leaves": "…", "why": "", "values": [{"quote": "const golpe = resolver(roll)", "value": "golpe = 'critico'"},
                                                          {"quote": "una línea que no existe", "value": "x"}]},
                    {"step": resolver, "title": "Decide", "transition": "atacar le pasa el 20", "arrives": "roll = 20",
                     "does": "…", "leaves": "'critico'", "why": "", "values": [{"quote": "if (roll === 20)", "value": "verdadero"}]},
                    {"step": registrar, "title": "Registra", "transition": "…", "arrives": "", "does": "…", "leaves": "", "why": "", "values": []}
                ],
                "forks": [{"after_step": resolver, "condition": "Si saca un 7", "outcome": "golpe normal"},
                          {"after_step": 999, "condition": "fuera", "outcome": "…"}]
            }),
        };
        let input = WalkInput { intent: "Añadir críticos", hints: "" };
        let walk = generate(&store, &review, &plan, input, &model, None).unwrap();

        assert_eq!(
            walk.stops.iter().map(|s| s.step + 1).collect::<Vec<_>>(),
            vec![atacar, resolver, registrar],
            "sin repetidos ni inventados"
        );
        assert_eq!(walk.stops[0].values.len(), 1, "solo los valores que citan una línea de verdad");
        assert_eq!(walk.stops[1].values[0].quote, "if (roll === 20) return 'critico'");
        assert_eq!(walk.stops[1].gap, None, "atacar llama a resolver");
        assert!(walk.stops[2].gap.as_deref().is_some_and(|gap| gap.contains("registrar")), "nadie llama a registrar");
        assert_eq!(walk.forks.len(), 1);
        let calls = model.calls.borrow();
        assert!(calls[0].contains("llama a"), "el índice dice quién llama a quién: {}", calls[0]);
        assert!(calls[1].contains("if (roll === 20)") && calls[1].contains("Un crítico"));

        let saved = stored(&store, &review, &plan).unwrap().unwrap();
        assert_eq!(saved.walkthrough, walk);
        assert!(!saved.stale);
    }

    #[test]
    fn a_walk_with_less_than_two_stops_is_not_saved() {
        let (review, plan) = review_and_plan();
        let store = Store::open_in_memory().unwrap();
        let model = Fake {
            calls: RefCell::new(Vec::new()),
            pick: json!({"title": "x", "situation": "y", "stops": [1]}),
            tell: json!({"stops": [], "forks": []}),
        };
        let input = WalkInput { intent: "", hints: "" };
        assert!(generate(&store, &review, &plan, input, &model, None).is_err());
        assert!(stored(&store, &review, &plan).unwrap().is_none());
    }
}
