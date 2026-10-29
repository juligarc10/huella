//! Narración de la revisión con un modelo: un resumen de cada turno contado
//! como flujo, una nota por paso y riesgos concretos. Es texto generado, no
//! literal de la sesión: se guarda con el modelo que lo escribió y se vuelve a
//! pedir solo si el turno cambia.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use huella_decisions::{ExtractError, JsonModel};
use huella_review::attention::{Attention, AttentionKind};
use huella_review::flow::{FlowStep, StepRole};
use huella_review::{DiffLine, SessionReview, Step};
use huella_store::{Store, StoredNarration};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Líneas de diff por paso que se envían al modelo.
const DIFF_LINES_PER_STEP: usize = 60;

pub const SYSTEM_PROMPT: &str = "\
Ayudas a una persona desarrolladora a revisar el código que escribió un agente de IA durante un turno de trabajo. \
Recibes su petición, las explicaciones que dio el agente, los pasos a revisar en el orden del flujo (numerados) con \
su diff, y los puntos de atención ya detectados.

Devuelve:
- summary: 2 a 4 frases que cuenten qué hace el cambio como un flujo (de la entrada a la persistencia o la salida), \
para que la persona tome contexto antes de leer el código. Sin repetir el código.
- steps: una nota de una frase para los pasos que no sean triviales: su papel en el flujo y qué conviene mirar. \
step es el número del paso.
- risks: riesgos concretos que se vean en el diff: errores sin manejar, casos límite, validaciones ausentes, \
incoherencias con la petición, efectos secundarios. step es el número del paso, o 0 si afecta a todo el turno. \
No repitas los puntos de atención ya detectados. Si no ves riesgos reales, devuelve una lista vacía.

Escribe en el idioma de la petición. No afirmes nada que no se vea en el diff o en las explicaciones.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelOutput {
    summary: String,
    steps: Vec<ModelNote>,
    risks: Vec<ModelNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelNote {
    step: u32,
    note: String,
}

/// Narración de un turno, con las notas ya referidas a pasos del plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Narration {
    pub turn: usize,
    pub model: String,
    pub summary: String,
    /// Índice en el plan → nota.
    pub step_notes: BTreeMap<usize, String>,
    /// Riesgos; `None` = del turno entero.
    pub risks: Vec<(Option<usize>, String)>,
    /// El turno cambió desde que se generó.
    pub stale: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NarrationReport {
    pub narrated: usize,
    pub cached: usize,
    pub failures: Vec<String>,
}

fn output_schema() -> Value {
    let notes = json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {"step": {"type": "integer"}, "note": {"type": "string"}},
            "required": ["step", "note"],
            "additionalProperties": false
        }
    });
    json!({
        "type": "object",
        "properties": {"summary": {"type": "string"}, "steps": notes, "risks": notes},
        "required": ["summary", "steps", "risks"],
        "additionalProperties": false
    })
}

/// Pasos del plan que pertenecen a un turno, en orden.
fn turn_steps(plan: &[FlowStep], turn: usize) -> Vec<usize> {
    (0..plan.len()).filter(|&i| plan[i].turn == Some(turn)).collect()
}

/// Lo que se envía al modelo para un turno.
fn turn_request(review: &SessionReview, plan: &[FlowStep], points: &[Attention], turn: usize) -> String {
    let steps = turn_steps(plan, turn);
    let local = |plan_index: usize| steps.iter().position(|&i| i == plan_index).map(|p| p + 1);
    let mut out = String::new();
    let prompt = review.turns.get(turn).and_then(|t| t.prompt.as_deref()).unwrap_or("(sin petición)");
    let _ = writeln!(out, "Petición de la persona:\n{prompt}\n");

    let mut rationales: Vec<&str> = Vec::new();
    for &i in &steps {
        for &source in &plan[i].sources {
            if let Step::Agent { rationale: Some(text), turn: t, .. } = &review.steps[source]
                && *t == turn
                && !rationales.contains(&text.as_str())
            {
                rationales.push(text);
            }
        }
    }
    out.push_str("Explicaciones del agente durante el turno:\n");
    for text in &rationales {
        let _ = writeln!(out, "- {}", text.split_whitespace().collect::<Vec<_>>().join(" "));
    }

    out.push_str("\nPasos a revisar, en el orden del flujo:\n");
    for (number, &i) in steps.iter().enumerate() {
        let step = &plan[i];
        let role = match step.role {
            StepRole::Flow => "flujo",
            StepRole::Test => "test",
            StepRole::Other => "otros cambios",
        };
        let name = step.symbol.as_ref().map_or("(fuera de funciones)", |s| s.qualified_name.as_str());
        let path = &review.files[step.file].path;
        let calls: Vec<String> = step.calls.iter().filter_map(|&c| local(c)).map(|n| format!("[{n}]")).collect();
        let _ = write!(out, "\n[{}] {role} · {name} ({path}) +{} −{}", number + 1, step.added(), step.removed());
        if !calls.is_empty() {
            let _ = write!(out, " · llama a {}", calls.join(", "));
        }
        out.push_str("\n```diff\n");
        for line in step.lines.iter().take(DIFF_LINES_PER_STEP) {
            let _ = match line {
                DiffLine::Context(text) => writeln!(out, " {text}"),
                DiffLine::Added { text, .. } => writeln!(out, "+{text}"),
                DiffLine::Removed(text) => writeln!(out, "-{text}"),
            };
        }
        if step.lines.len() > DIFF_LINES_PER_STEP {
            let _ = writeln!(out, "… ({} líneas más)", step.lines.len() - DIFF_LINES_PER_STEP);
        }
        out.push_str("```\n");
    }

    let detected: Vec<String> = points
        .iter()
        .filter(|p| p.turn == Some(turn) && p.kind != AttentionKind::Unexplained)
        .map(|p| match p.step.and_then(local) {
            Some(n) => format!("- [{n}] {}", p.message),
            None => format!("- {}", p.message),
        })
        .collect();
    if !detected.is_empty() {
        let _ = write!(out, "\nPuntos de atención ya detectados:\n{}\n", detected.join("\n"));
    }
    huella_core::redact::redact(&out).into_owned()
}

fn input_hash(model: &str, request: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    hasher.update(SYSTEM_PROMPT.as_bytes());
    hasher.update(request.as_bytes());
    hasher.finalize().to_hex()[..16].to_owned()
}

/// Turnos con pasos que revisar.
fn reviewable_turns(plan: &[FlowStep]) -> Vec<usize> {
    let mut turns: Vec<usize> = plan.iter().filter_map(|s| s.turn).collect();
    turns.sort_unstable();
    turns.dedup();
    turns
}

/// Narra los turnos que no tienen narración vigente. `progress` recibe el
/// turno antes de cada llamada al modelo.
///
/// # Errors
/// Si falla la base de datos. Los fallos del modelo se anotan en el informe.
pub fn narrate(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    points: &[Attention],
    model: &(impl JsonModel + ?Sized),
    mut progress: impl FnMut(usize),
) -> crate::Result<NarrationReport> {
    let mut report = NarrationReport::default();
    let stored = store.narrations(&review.session_id)?;
    let model_id = model.model_id();
    for turn in reviewable_turns(plan) {
        let request = turn_request(review, plan, points, turn);
        let hash = input_hash(&model_id, &request);
        if stored.get(&turn).is_some_and(|s| s.input_hash == hash && validate(s.narration.clone()).is_ok()) {
            report.cached += 1;
            continue;
        }
        progress(turn);
        match model.complete_json(SYSTEM_PROMPT, &request, &output_schema()).and_then(validate) {
            Ok(narration) => {
                store.save_narration(
                    &review.session_id,
                    turn,
                    &StoredNarration { input_hash: hash, model: model_id.clone(), narration },
                )?;
                report.narrated += 1;
            }
            Err(error) => report.failures.push(format!("turno {}: {error}", turn + 1)),
        }
    }
    Ok(report)
}

/// Una respuesta sin resumen no sirve: se descarta para volver a pedirla.
fn validate(text: String) -> Result<String, ExtractError> {
    let output: ModelOutput =
        serde_json::from_str(&text).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
    if output.summary.trim().is_empty() {
        return Err(ExtractError::BadResponse("el modelo no escribió el resumen".into()));
    }
    Ok(text)
}

/// Narraciones guardadas, referidas al plan actual. Las de turnos que han
/// cambiado se marcan como desfasadas en lugar de descartarse.
///
/// # Errors
/// Si falla la base de datos.
pub fn narrations(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    points: &[Attention],
) -> crate::Result<BTreeMap<usize, Narration>> {
    let mut result = BTreeMap::new();
    for (turn, stored) in store.narrations(&review.session_id)? {
        let Ok(output) = serde_json::from_str::<ModelOutput>(&stored.narration) else { continue };
        let steps = turn_steps(plan, turn);
        let to_plan = |number: u32| {
            usize::try_from(number).ok().and_then(|n| n.checked_sub(1)).and_then(|n| steps.get(n).copied())
        };
        let current = input_hash(&stored.model, &turn_request(review, plan, points, turn));
        result.insert(
            turn,
            Narration {
                turn,
                model: stored.model,
                summary: output.summary,
                step_notes: output.steps.into_iter().filter_map(|n| Some((to_plan(n.step)?, n.note))).collect(),
                risks: output.risks.into_iter().map(|n| (to_plan(n.step), n.note)).collect(),
                stale: current != stored.input_hash,
            },
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    struct Fake {
        calls: RefCell<Vec<String>>,
        answer: String,
    }

    impl JsonModel for Fake {
        fn model_id(&self) -> String {
            "fake:1".into()
        }

        fn complete_json(&self, system: &str, user: &str, schema: &Value) -> Result<String, ExtractError> {
            assert_eq!(system, SYSTEM_PROMPT);
            assert_eq!(schema["required"][0], "summary");
            self.calls.borrow_mut().push(user.to_owned());
            Ok(self.answer.clone())
        }
    }

    fn review_of(content: &str) -> (SessionReview, Vec<FlowStep>, Vec<Attention>) {
        let edit = FileEdit {
            origin: EditOrigin { agent: "t", session_id: "s".into(), event_id: "e".into(), timestamp: None, cwd: None },
            path: PathBuf::from("/r/canje.ts"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(content.into()),
            user_modified: false,
            context: EditContext {
                prompt: Some("añade el canje".into()),
                rationale: Some("el canje valida el código".into()),
            },
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let points = huella_review::attention::attention(&review, &plan);
        (review, plan, points)
    }

    const CODE: &str = "export function redeem(code: string) {\n  return check(code);\n}\n\nexport function check(code: string) {\n  return code.length > 3;\n}\n";

    #[test]
    fn request_lists_prompt_rationale_steps_and_detected_points() {
        let (review, plan, points) = review_of(CODE);
        let request = turn_request(&review, &plan, &points, 0);
        assert!(request.starts_with("Petición de la persona:\nañade el canje\n"), "{request}");
        assert!(request.contains("- el canje valida el código"));
        assert!(request.contains("[1] flujo · redeem (/r/canje.ts) +4 −0 · llama a [2]"), "{request}");
        assert!(request.contains("+  return check(code);"));
        assert!(request.contains("Puntos de atención ya detectados:\n- cambia código sin añadir ni tocar tests"));
    }

    #[test]
    fn narrations_are_cached_mapped_to_the_plan_and_marked_stale() {
        let (review, plan, points) = review_of(CODE);
        let store = Store::open_in_memory().unwrap();
        let fake = Fake {
            calls: RefCell::new(vec![]),
            answer: r#"{"summary": "redeem delega en check", "steps": [{"step": 2, "note": "valida la longitud"}],
                        "risks": [{"step": 0, "note": "no se prueba"}, {"step": 9, "note": "paso inexistente"}]}"#
                .into(),
        };
        let first = narrate(&store, &review, &plan, &points, &fake, |_| {}).unwrap();
        let again = narrate(&store, &review, &plan, &points, &fake, |_| {}).unwrap();
        assert_eq!((first.narrated, again.cached, fake.calls.borrow().len()), (1, 1, 1));

        let narration = &narrations(&store, &review, &plan, &points).unwrap()[&0];
        assert_eq!(narration.model, "fake:1");
        assert_eq!(narration.step_notes.get(&1).map(String::as_str), Some("valida la longitud"));
        assert_eq!(narration.risks, vec![(None, "no se prueba".to_owned()), (None, "paso inexistente".to_owned())]);
        assert!(!narration.stale);

        let (changed, changed_plan, changed_points) = review_of(&CODE.replace("> 3", "> 4"));
        assert!(narrations(&store, &changed, &changed_plan, &changed_points).unwrap()[&0].stale);
    }

    #[test]
    fn an_empty_summary_is_not_accepted() {
        let (review, plan, points) = review_of(CODE);
        let store = Store::open_in_memory().unwrap();
        let fake = Fake { calls: RefCell::new(vec![]), answer: r#"{"summary": " ", "steps": [], "risks": []}"#.into() };
        let report = narrate(&store, &review, &plan, &points, &fake, |_| {}).unwrap();
        assert_eq!((report.narrated, report.failures.len()), (0, 1));
    }

    #[test]
    fn malformed_answers_are_reported_not_saved() {
        let (review, plan, points) = review_of(CODE);
        let store = Store::open_in_memory().unwrap();
        let fake = Fake { calls: RefCell::new(vec![]), answer: "{\"summary\": 1}".into() };
        let report = narrate(&store, &review, &plan, &points, &fake, |_| {}).unwrap();
        assert_eq!(report.failures.len(), 1);
        assert!(narrations(&store, &review, &plan, &points).unwrap().is_empty());
    }
}
