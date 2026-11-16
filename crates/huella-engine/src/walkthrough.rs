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

use crate::blocks::StoryBlock;
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

`terms`: de 0 a 6 términos del dominio o del código que hay que conocer para seguir el caso, con su significado aquí.

`checks`: de 2 a 4 preguntas para comprobar que se ha entendido el caso, con su respuesta y `step` (la parada donde \
se ve). Que obliguen a entender, no a recordar un nombre.

{BLOCKS}

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
    /// El valor salió de ejecutar un test, no de leer el código.
    #[serde(default)]
    pub from_test: bool,
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
    /// Cómo se llega aquí: se calcula del código, no lo cuenta el modelo.
    #[serde(default)]
    pub hop: Option<Hop>,
    /// La añadió Huella para unir el camino: quien contó el recorrido se la saltó.
    #[serde(default)]
    pub bridge: bool,
    /// Fichero y símbolo del paso, para volver a encontrarlo si la lista de pasos
    /// cambia (la sesión siguió editando): el número de paso solo vale mientras tanto.
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
}

/// Cómo se llega a una parada desde las anteriores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    /// `direct`: la parada anterior la llama; `back`: la anterior terminó y el
    /// control volvió a una parada de antes, que la llama; `none`: no se ve.
    pub kind: String,
    /// La parada (su posición en el recorrido) desde la que se llama.
    pub from: Option<usize>,
    /// La línea de esa parada (índice en las líneas de su paso) con la llamada.
    pub line: Option<usize>,
    /// El texto de esa línea.
    pub text: String,
}

/// Un término que hay que conocer para seguir el caso.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    pub term: String,
    pub meaning: String,
}

/// Una pregunta para comprobar que se ha entendido, con la parada donde se ve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub question: String,
    pub answer: String,
    /// Índice del paso en el plan.
    pub step: Option<usize>,
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
    /// Esquemas o tablas del caso, si aportan (los mismos de la presentación).
    #[serde(default)]
    pub blocks: Vec<StoryBlock>,
    #[serde(default)]
    pub terms: Vec<Term>,
    #[serde(default)]
    pub checks: Vec<Check>,
}

/// Un recorrido guardado, con si el cambio ha cambiado desde entonces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredWalkthrough {
    pub walkthrough: Walkthrough,
    pub model: String,
    pub recorded_at: String,
    pub stale: bool,
    /// `agent`: lo contó el agente que hizo el cambio; `model`: el modelo revisor.
    pub source: String,
}

/// El recorrido guardado de una revisión, si lo hay.
///
/// # Errors
/// Si falla la base de datos.
pub fn stored(store: &Store, review: &SessionReview, plan: &[FlowStep]) -> Result<Option<StoredWalkthrough>> {
    let Some(saved) = store.walkthrough(&review.session_id)? else { return Ok(None) };
    let Ok(mut walkthrough) = serde_json::from_str::<Walkthrough>(&saved.content) else { return Ok(None) };
    // Si el cambio avanzó, los números de paso pueden apuntar a otro código: no se enseña como bueno.
    let stale = saved.input_hash != change_hash(review, plan);
    // Si el cambio siguió, los números de paso ya no valen: cada parada se busca
    // por su fichero y su símbolo. Uno guardado sin ellos solo vale si el cambio es el mismo.
    let anchored = walkthrough.stops.iter().all(|stop| stop.file.is_some() && stop.symbol.is_some());
    let fits = if anchored {
        reanchor(review, plan, &mut walkthrough)
    } else {
        !stale && walkthrough.stops.iter().all(|stop| stop.step < plan.len())
    };
    if fits {
        // Los recorridos guardados antes de calcular los saltos también los tienen.
        bridge_gaps(plan, &mut walkthrough.stops);
        anchor_stops(review, plan, &mut walkthrough);
    }
    Ok(fits.then_some(StoredWalkthrough {
        walkthrough,
        model: saved.model,
        recorded_at: saved.recorded_at,
        stale,
        source: "model".to_owned(),
    }))
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
    let system = TELL_SYSTEM.replace("{BLOCKS}", &crate::blocks::guide());
    let told = call(model, &system, &tell, &tell_schema())?;
    let walkthrough = assemble(plan, title, situation, &stops, &told);

    let mut walkthrough = walkthrough;
    anchor_stops(review, plan, &mut walkthrough);
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
                line_of(&plan[step], &quote).map(|line| LineValue { quote: line, value: shown, from_test: false })
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
            hop: None,
            bridge: false,
            file: None,
            symbol: None,
        });
    }
    bridge_gaps(plan, &mut stops);
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
    let blocks: Vec<StoryBlock> = serde_json::from_value(told["blocks"].clone()).unwrap_or_default();
    let blocks = crate::blocks::sanitize(&blocks).into_iter().take(2).collect();
    let terms = told["terms"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|term| Term { term: text(term, "term"), meaning: text(term, "meaning") })
        .filter(|term| !term.term.is_empty() && !term.meaning.is_empty())
        .take(6)
        .collect();
    let checks = told["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|check| Check {
            question: text(check, "question"),
            answer: text(check, "answer"),
            step: check["step"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok()?.checked_sub(1))
                .filter(|step| stops.iter().any(|stop| stop.step == *step)),
        })
        .filter(|check| !check.question.is_empty() && !check.answer.is_empty())
        .take(4)
        .collect();
    Walkthrough { title, situation, stops, forks, outside, blocks, terms, checks }
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

/// Cómo se llega a cada parada: la línea de una parada anterior que la llama
/// (la más cercana), o, si el texto no la muestra, el grafo de llamadas. Si la
/// llama la anterior, el salto es directo; si la llama una de antes, la
/// anterior ya terminó y el control volvió allí. Si nada la llama, se avisa.
fn mark_hops(plan: &[FlowStep], stops: &mut [Stop]) {
    let name = |index: usize| {
        plan[index]
            .symbol
            .as_ref()
            .map_or_else(|| format!("el paso {}", index + 1), |symbol| format!("`{}`", symbol.qualified_name))
    };
    for position in 0..stops.len() {
        if position == 0 {
            stops[0].hop = None;
            stops[0].gap = None;
            continue;
        }
        let here = stops[position].step;
        let short = plan[here]
            .symbol
            .as_ref()
            .map(|symbol| symbol.qualified_name.rsplit(['.', ':']).next().unwrap_or(&symbol.qualified_name).to_owned());
        let by_text = short.as_deref().and_then(|wanted| {
            (0..position).rev().find_map(|from| {
                let lines = &plan[stops[from].step].lines;
                lines.iter().enumerate().find_map(|(index, line)| match line {
                    DiffLine::Added { text, .. } | DiffLine::Context(text) if calls(text, wanted) => {
                        Some((from, Some(index), text.trim().to_owned()))
                    }
                    _ => None,
                })
            })
        });
        let by_graph = || {
            (0..position)
                .rev()
                .find(|&from| plan[stops[from].step].calls.contains(&here))
                .map(|from| (from, None, String::new()))
        };
        let hop = match by_text.or_else(by_graph) {
            Some((from, line, text)) => Hop {
                kind: if from + 1 == position { "direct" } else { "back" }.to_owned(),
                from: Some(from),
                line,
                text,
            },
            None => Hop { kind: "none".to_owned(), from: None, line: None, text: String::new() },
        };
        stops[position].gap = (hop.kind == "none").then(|| {
            format!(
                "No se ve en el código que {} lleve a {}: el salto lo cuenta el modelo, compruébalo.",
                name(stops[position - 1].step),
                name(here)
            )
        });
        stops[position].hop = Some(hop);
    }
}

/// Lo que empieza una declaración, no una llamada.
const DECLARATIONS: [&str; 14] = [
    "public ",
    "private ",
    "protected ",
    "static ",
    "final ",
    "abstract ",
    "override ",
    "fn ",
    "pub ",
    "def ",
    "function ",
    "async ",
    "export ",
    "func ",
];

/// Apunta en cada parada el fichero y el símbolo de su paso.
fn anchor_stops(review: &SessionReview, plan: &[FlowStep], walkthrough: &mut Walkthrough) {
    for stop in &mut walkthrough.stops {
        let step = &plan[stop.step];
        stop.file = Some(review.files[step.file].path.clone());
        stop.symbol = step.symbol.as_ref().map(|symbol| symbol.qualified_name.clone());
    }
}

/// Vuelve a encontrar el paso de cada parada por su fichero y su símbolo, y con
/// él los de las bifurcaciones y las preguntas. Las que ya no están se quitan.
/// `false` si no quedan al menos dos paradas.
fn reanchor(review: &SessionReview, plan: &[FlowStep], walkthrough: &mut Walkthrough) -> bool {
    let mut moved: Vec<(usize, usize)> = Vec::new();
    walkthrough.stops.retain_mut(|stop| {
        let (Some(file), Some(symbol)) = (&stop.file, &stop.symbol) else { return false };
        let Some(step) = step_for(review, plan, file, symbol) else { return false };
        moved.push((stop.step, step));
        stop.step = step;
        true
    });
    let new_step = |old: usize| moved.iter().find(|(before, _)| *before == old).map(|(_, now)| *now);
    walkthrough.forks.retain_mut(|fork| new_step(fork.step).map(|step| fork.step = step).is_some());
    for check in &mut walkthrough.checks {
        check.step = check.step.and_then(new_step);
    }
    walkthrough.outside = (0..plan.len())
        .filter(|&index| plan[index].role == StepRole::Flow && !walkthrough.stops.iter().any(|stop| stop.step == index))
        .collect();
    walkthrough.stops.len() >= MIN_STOPS
}

/// Paradas intermedias que se pueden añadir, como mucho, para unir un recorrido.
const MAX_BRIDGES: usize = 4;

/// Calcula los saltos y, donde una parada no tiene llamada visible, busca entre
/// los pasos del cambio el que la llama y al que se llega desde una parada
/// anterior, y lo añade como parada intermedia. No inventa nada: solo usa el
/// código. Así un recorrido al que le falta un paso (lo que más se salta un
/// modelo pequeño) vuelve a tener hilo.
fn bridge_gaps(plan: &[FlowStep], stops: &mut Vec<Stop>) {
    let short = |index: usize| {
        plan[index]
            .symbol
            .as_ref()
            .map(|symbol| symbol.qualified_name.rsplit(['.', ':']).next().unwrap_or(&symbol.qualified_name).to_owned())
    };
    let calls_step = |caller: usize, callee: usize| {
        plan[caller].calls.contains(&callee)
            || short(callee).is_some_and(|name| {
                plan[caller].lines.iter().any(|line| match line {
                    DiffLine::Added { text, .. } | DiffLine::Context(text) => calls(text, &name),
                    DiffLine::Removed(_) => false,
                })
            })
    };
    for _ in 0..MAX_BRIDGES {
        mark_hops(plan, stops);
        let Some(position) = stops.iter().position(|stop| stop.hop.as_ref().is_some_and(|hop| hop.kind == "none"))
        else {
            return;
        };
        let target = stops[position].step;
        let bridge = (0..plan.len()).find(|&candidate| {
            plan[candidate].role == StepRole::Flow
                && !stops.iter().any(|stop| stop.step == candidate)
                && calls_step(candidate, target)
                && stops[..position].iter().any(|before| calls_step(before.step, candidate))
        });
        let Some(bridge) = bridge else { return };
        stops.insert(
            position,
            Stop {
                step: bridge,
                title: String::new(),
                transition: String::new(),
                arrives: String::new(),
                does: String::new(),
                leaves: String::new(),
                why: String::new(),
                values: Vec::new(),
                gap: None,
                hop: None,
                bridge: true,
                file: None,
                symbol: None,
            },
        );
    }
    mark_hops(plan, stops);
}

/// ¿Llama esta línea a `name`? `x.name(`, `name(` o `new Name(`, pero no su
/// declaración (`public X name(`, `fn name(`, `def name(`) ni un comentario.
fn calls(line: &str, name: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//")
        || trimmed.starts_with('*')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('#')
        || trimmed.starts_with('@')
        || DECLARATIONS.iter().any(|keyword| trimmed.starts_with(keyword))
    {
        return false;
    }
    let bytes = line.as_bytes();
    let mut start = 0;
    while let Some(found) = line[start..].find(name) {
        let at = start + found;
        let end = at + name.len();
        let before_ok = at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let after = line[end..].trim_start();
        if before_ok && after.starts_with('(') {
            return true;
        }
        start = end;
    }
    false
}

// ---------- Lo que cuenta el agente (huella_walkthrough) ----------

/// Una parada tal como la cuenta el agente: anclada a un fichero y un símbolo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToldStop {
    pub file: String,
    pub symbol: String,
    pub title: String,
    pub transition: String,
    pub arrives: String,
    pub does: String,
    pub leaves: String,
    pub why: String,
    pub values: Vec<LineValue>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToldFork {
    pub file: String,
    pub symbol: String,
    pub condition: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToldCheck {
    pub question: String,
    pub answer: String,
    pub file: String,
    pub symbol: String,
}

/// El recorrido que cuenta el agente al terminar.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToldWalk {
    pub title: String,
    pub situation: String,
    pub stops: Vec<ToldStop>,
    pub forks: Vec<ToldFork>,
    pub terms: Vec<Term>,
    pub checks: Vec<ToldCheck>,
    pub blocks: Vec<StoryBlock>,
}

/// Lo que se le responde al agente: qué se guardó y qué tiene que arreglar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedWalk {
    pub id: i64,
    pub stops: usize,
    pub problems: Vec<String>,
}

/// El símbolo de un fichero por su nombre (`Clase.metodo` o solo `metodo`) y sus líneas.
fn find_in_file(path: &std::path::Path, source: &str, symbol: &str) -> Option<(usize, usize)> {
    let symbols = huella_anchoring::Language::from_path(path)
        .and_then(|language| huella_anchoring::extract_symbols(language, source).ok())
        .map(|extraction| extraction.symbols)
        .unwrap_or_default();
    let found = symbols.iter().find(|s| s.qualified_name == symbol).or_else(|| {
        symbols.iter().find(|s| s.qualified_name.rsplit(['.', ':']).next() == symbol.rsplit(['.', ':']).next())
    })?;
    Some((found.start_line, found.end_line))
}

/// Guarda el recorrido que cuenta el agente, con las rutas normalizadas, y
/// dice qué no cuadra con el código: ficheros o símbolos que no están, valores
/// que citan líneas que no existen y paradas a las que no se ve llegar.
///
/// # Errors
/// Si ninguna parada apunta a código que exista, o falla la base de datos.
pub fn record_declared(store: &Store, mut told: ToldWalk) -> Result<RecordedWalk> {
    let mut problems = Vec::new();
    // El código de cada parada, para comprobar valores y saltos.
    let mut codes: Vec<Option<Vec<String>>> = Vec::new();
    for (index, stop) in told.stops.iter_mut().enumerate() {
        let number = index + 1;
        let Ok(path) = std::fs::canonicalize(&stop.file) else {
            problems.push(format!("parada {number}: no encuentro el fichero {}; usa su ruta absoluta", stop.file));
            codes.push(None);
            continue;
        };
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        let Some((start, end)) = find_in_file(&path, &source, &stop.symbol) else {
            problems.push(format!("parada {number}: no encuentro `{}` en {}", stop.symbol, stop.file));
            codes.push(None);
            continue;
        };
        let lines: Vec<String> = source.lines().skip(start - 1).take(end - start + 1).map(str::to_owned).collect();
        for value in &stop.values {
            if !lines.iter().any(|line| {
                line.trim() == value.quote.trim() || (value.quote.trim().len() > 3 && line.contains(value.quote.trim()))
            }) {
                problems.push(format!(
                    "parada {number}: la línea «{}» no está en `{}`",
                    clip(value.quote.trim(), 80),
                    stop.symbol
                ));
            }
        }
        stop.file = crate::normalize_path(&path, None);
        codes.push(Some(lines));
    }
    let resolved = codes.iter().filter(|code| code.is_some()).count();
    if resolved == 0 {
        return Err(EngineError::Invalid(
            "ninguna parada apunta a código que exista: usa rutas absolutas y nombres de símbolos reales".into(),
        ));
    }
    if told.stops.len() < MIN_STOPS {
        problems.push("un recorrido necesita al menos dos paradas".to_owned());
    }
    for index in 1..told.stops.len() {
        let wanted = told.stops[index].symbol.rsplit(['.', ':']).next().unwrap_or_default().to_owned();
        if codes[index].is_none() || wanted.is_empty() {
            continue;
        }
        let reached = codes[..index].iter().flatten().any(|lines| lines.iter().any(|line| calls(line, &wanted)));
        if !reached {
            problems.push(format!(
                "parada {}: no se ve que ninguna parada anterior llame a `{wanted}`; si se llega desde otro sitio, añade esa parada o cuéntalo en transition",
                index + 1
            ));
        }
    }
    for fork in &mut told.forks {
        if let Ok(path) = std::fs::canonicalize(&fork.file) {
            fork.file = crate::normalize_path(&path, None);
        }
    }
    for check in &mut told.checks {
        if let Ok(path) = std::fs::canonicalize(&check.file) {
            check.file = crate::normalize_path(&path, None);
        }
    }
    let content = serde_json::to_string(&told).map_err(|error| EngineError::Invalid(error.to_string()))?;
    let id = store.add_declared_walkthrough(&content)?;
    Ok(RecordedWalk { id, stops: resolved, problems })
}

/// El paso del plan de una parada contada por el agente: mismo fichero y mismo símbolo.
fn step_for(review: &SessionReview, plan: &[FlowStep], file: &str, symbol: &str) -> Option<usize> {
    let short = symbol.rsplit(['.', ':']).next().unwrap_or(symbol);
    let in_file = |index: &usize| review.files[plan[*index].file].path == file;
    let named = |index: &usize, exact: bool| {
        plan[*index].symbol.as_ref().is_some_and(|s| {
            if exact { s.qualified_name == symbol } else { s.qualified_name.rsplit(['.', ':']).next() == Some(short) }
        })
    };
    (0..plan.len())
        .find(|index| in_file(index) && named(index, true))
        .or_else(|| (0..plan.len()).find(|index| in_file(index) && named(index, false)))
}

/// El último recorrido que contó el agente y encaja con esta revisión: al
/// menos dos paradas y la mayoría de ellas en pasos suyos.
///
/// # Errors
/// Si falla la base de datos.
pub fn declared(store: &Store, review: &SessionReview, plan: &[FlowStep]) -> Result<Option<StoredWalkthrough>> {
    for (_, recorded_at, content) in store.declared_walkthroughs(20)? {
        let Ok(told) = serde_json::from_str::<ToldWalk>(&content) else { continue };
        let resolved: Vec<(usize, &ToldStop)> = told
            .stops
            .iter()
            .filter_map(|stop| step_for(review, plan, &stop.file, &stop.symbol).map(|step| (step, stop)))
            .collect();
        if resolved.len() < MIN_STOPS || resolved.len() * 5 < told.stops.len() * 3 {
            continue;
        }
        let mut stops: Vec<Stop> = resolved
            .iter()
            .map(|(step, stop)| Stop {
                step: *step,
                title: stop.title.clone(),
                transition: stop.transition.clone(),
                arrives: stop.arrives.clone(),
                does: stop.does.clone(),
                leaves: stop.leaves.clone(),
                why: stop.why.clone(),
                values: stop
                    .values
                    .iter()
                    .filter_map(|value| {
                        line_of(&plan[*step], &value.quote).map(|line| LineValue {
                            quote: line,
                            value: value.value.clone(),
                            from_test: value.from_test,
                        })
                    })
                    .take(8)
                    .collect(),
                gap: None,
                hop: None,
                bridge: false,
                file: Some(stop.file.clone()),
                symbol: Some(stop.symbol.clone()),
            })
            .collect();
        bridge_gaps(plan, &mut stops);
        let forks = told
            .forks
            .iter()
            .filter_map(|fork| {
                let step = step_for(review, plan, &fork.file, &fork.symbol)?;
                stops.iter().any(|stop| stop.step == step).then(|| Fork {
                    step,
                    condition: fork.condition.clone(),
                    outcome: fork.outcome.clone(),
                })
            })
            .collect();
        let checks = told
            .checks
            .iter()
            .map(|check| Check {
                question: check.question.clone(),
                answer: check.answer.clone(),
                step: step_for(review, plan, &check.file, &check.symbol)
                    .filter(|step| stops.iter().any(|stop| stop.step == *step)),
            })
            .collect();
        let outside = (0..plan.len())
            .filter(|&index| plan[index].role == StepRole::Flow && !stops.iter().any(|stop| stop.step == index))
            .collect();
        let walkthrough = Walkthrough {
            title: told.title,
            situation: told.situation,
            stops,
            forks,
            outside,
            blocks: crate::blocks::sanitize(&told.blocks),
            terms: told.terms,
            checks,
        };
        return Ok(Some(StoredWalkthrough {
            walkthrough,
            model: "el agente".to_owned(),
            recorded_at,
            stale: false,
            source: "agent".to_owned(),
        }));
    }
    Ok(None)
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
            "terms": {"type": "array", "items": {
                "type": "object",
                "properties": {"term": text(), "meaning": text()},
                "required": ["term", "meaning"],
                "additionalProperties": false
            }},
            "checks": {"type": "array", "items": {
                "type": "object",
                "properties": {"question": text(), "answer": text(), "step": {"type": "integer"}},
                "required": ["question", "answer", "step"],
                "additionalProperties": false
            }},
            "blocks": crate::blocks::output_schema(),
        },
        "required": ["stops", "forks", "terms", "checks", "blocks"],
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
        let hop = walk.stops[1].hop.clone().unwrap();
        assert_eq!(
            (hop.kind.as_str(), hop.from, hop.text.as_str()),
            ("direct", Some(0), "const golpe = resolver(roll)")
        );
        assert!(walk.stops[2].gap.as_deref().is_some_and(|gap| gap.contains("registrar")), "nadie llama a registrar");
        assert_eq!(walk.stops[2].hop.as_ref().unwrap().kind, "none");
        assert_eq!(walk.forks.len(), 1);
        let calls = model.calls.borrow();
        assert!(calls[0].contains("llama a"), "el índice dice quién llama a quién: {}", calls[0]);
        assert!(calls[1].contains("if (roll === 20)") && calls[1].contains("Un crítico"));

        let saved = stored(&store, &review, &plan).unwrap().unwrap();
        assert_eq!(
            saved.walkthrough.stops.iter().map(|s| s.step).collect::<Vec<_>>(),
            walk.stops.iter().map(|s| s.step).collect::<Vec<_>>()
        );
        assert!(!saved.stale);
        assert_eq!(saved.walkthrough.stops[0].symbol.as_deref(), Some("atacar"), "cada parada guarda su símbolo");

        // La sesión sigue y aparece un fichero antes: los números de paso cambian, las paradas no.
        let before = FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "s1".into(),
                event_id: "e0".into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from("/r/aaa.ts"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known("export function antes() { return 1 }\n".into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let first = FileEdit {
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
        let grown = huella_review::build("s1", &[before, first], |e| e.path.display().to_string(), |_| None);
        let grown_plan = huella_review::flow::plan(&grown);
        let moved = stored(&store, &grown, &grown_plan).unwrap().unwrap();
        assert!(moved.stale);
        let names: Vec<&str> = moved
            .walkthrough
            .stops
            .iter()
            .map(|s| grown_plan[s.step].symbol.as_ref().unwrap().qualified_name.as_str())
            .collect();
        assert_eq!(names, ["atacar", "resolver", "registrar"], "cada parada sigue en su símbolo");
    }

    #[test]
    fn a_missing_middle_step_is_bridged_from_the_code() {
        // atacar → resolver → registrar, pero el modelo se salta resolver: Huella la añade.
        const CHAIN: &str = "\
export function atacar(roll: number) {
  return resolver(roll)
}
export function resolver(roll: number) {
  return registrar(roll === 20 ? 'critico' : 'normal')
}
export function registrar(texto: string) {
  return texto
}
";
        let edit = FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "s1".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from("/r/cadena.ts"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(CHAIN.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("s1", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let at =
            |name: &str| plan.iter().position(|s| s.symbol.as_ref().is_some_and(|s| s.qualified_name == name)).unwrap();
        let stop = |step: usize| Stop {
            step,
            title: "x".into(),
            transition: String::new(),
            arrives: String::new(),
            does: String::new(),
            leaves: String::new(),
            why: String::new(),
            values: Vec::new(),
            gap: None,
            hop: None,
            bridge: false,
            file: None,
            symbol: None,
        };
        let mut stops = vec![stop(at("atacar")), stop(at("registrar"))];
        bridge_gaps(&plan, &mut stops);
        let order: Vec<usize> = stops.iter().map(|stop| stop.step).collect();
        assert_eq!(order, vec![at("atacar"), at("resolver"), at("registrar")]);
        assert!(stops[1].bridge, "la intermedia la añadió Huella");
        assert!(stops.iter().skip(1).all(|stop| stop.hop.as_ref().unwrap().kind == "direct"), "{stops:?}");
    }

    #[test]
    fn a_call_is_not_its_declaration_nor_a_comment() {
        assert!(calls("    StartedGame started = game.start(request.playerName());", "start"));
        assert!(calls("return Optional.of(new Flavor(dto.title()));", "Flavor"));
        assert!(calls("  if (resolver (roll)) {", "resolver"));
        assert!(!calls("  public StartResponse start(@Valid StartRequest request) {", "start"));
        assert!(!calls("  // luego start(x)", "start"));
        assert!(!calls("  restart(x)", "start"), "otra palabra que acaba igual");
        assert!(!calls("  pub fn start(&self) {", "start"));
    }

    #[test]
    fn the_agent_walk_is_checked_against_the_code_and_matched_to_the_review() {
        let dir = std::env::temp_dir().join(format!("huella-told-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("combate.ts");
        std::fs::write(&file, CODE).unwrap();
        let file = std::fs::canonicalize(&file).unwrap();
        let path = file.display().to_string();
        let stop = |symbol: &str, quote: &str, value: &str| ToldStop {
            file: path.clone(),
            symbol: symbol.into(),
            title: symbol.into(),
            values: vec![LineValue { quote: quote.into(), value: value.into(), from_test: true }],
            ..ToldStop::default()
        };
        let told = ToldWalk {
            title: "Un crítico".into(),
            stops: vec![
                stop("atacar", "const golpe = resolver(roll)", "golpe = 'critico'"),
                stop("resolver", "if (roll === 20) return 'critico'", "verdadero"),
                stop("registrar", "no existe esta línea", "x"),
                stop("NoExiste", "", ""),
            ],
            ..ToldWalk::default()
        };
        let store = Store::open_in_memory().unwrap();
        let recorded = record_declared(&store, told).unwrap();
        assert_eq!(recorded.stops, 3);
        let problems = recorded.problems.join(" | ");
        assert!(problems.contains("no existe esta línea"), "{problems}");
        assert!(problems.contains("`registrar`"), "nadie llama a registrar: {problems}");
        assert!(problems.contains("NoExiste"), "{problems}");
        assert!(!problems.contains("parada 2"), "atacar llama a resolver: {problems}");

        // En una revisión con ese fichero, se resuelve a sus pasos, con los saltos calculados.
        let edit = FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "s1".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: file.clone(),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(CODE.into()),
            user_modified: false,
            context: EditContext::default(),
        };
        let review = huella_review::build("s1", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let found = declared(&store, &review, &plan).unwrap().unwrap();
        assert_eq!(found.source, "agent");
        assert_eq!(found.walkthrough.stops.len(), 3);
        assert_eq!(found.walkthrough.stops[1].hop.as_ref().unwrap().kind, "direct");
        assert!(found.walkthrough.stops[0].values[0].from_test);
        assert!(found.walkthrough.stops[2].values.is_empty(), "el valor de una línea que no existe se quita");
        std::fs::remove_dir_all(&dir).unwrap();
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
