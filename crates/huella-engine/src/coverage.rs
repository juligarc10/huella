//! Cobertura de la petición: lo que la persona pidió en un turno, troceado en
//! requisitos, con los pasos que cumplen cada uno, lo que quedó sin hacer y
//! lo que el agente hizo sin que se le pidiera.
//!
//! Es texto generado por un modelo: se guarda con el modelo que lo escribió,
//! se vuelve a pedir solo si el turno cambia y se marca como desfasado.

use std::collections::BTreeMap;

use huella_decisions::{ExtractError, JsonModel};
use huella_review::SessionReview;
use huella_review::attention::Attention;
use huella_review::flow::FlowStep;
use huella_store::{Store, StoredCoverage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::narration::{reviewable_turns, turn_request, turn_steps};

pub const SYSTEM_PROMPT: &str = "\
Ayudas a una persona desarrolladora a comprobar si el código que escribió un agente de IA hace lo que pidió. \
Recibes su petición, las explicaciones del agente y los pasos del cambio numerados, con su diff.

Devuelve:
- requirements: la petición troceada en requisitos concretos y comprobables, en el orden en que se pidieron. \
Incluye también lo que se pide de forma implícita pero inequívoca. Para cada uno: text (el requisito, breve), \
status (done si los pasos lo cumplen, partial si lo cumplen a medias, missing si ningún paso lo hace), \
steps (los números de los pasos que lo cumplen) y note (qué falta o qué conviene comprobar; vacío si nada).
- unrequested: pasos que hacen algo que la petición no pide ni se deduce de ella (refactors, cambios de \
comportamiento, ficheros ajenos). step es el número del paso y note qué hace. Los tests, la configuración y los \
imports necesarios para lo pedido no cuentan.

Juzga solo por el diff y las explicaciones; si algo no se ve, es partial o missing, no done. \
Escribe en el idioma de la petición.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelOutput {
    requirements: Vec<ModelRequirement>,
    unrequested: Vec<ModelNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelRequirement {
    text: String,
    status: String,
    steps: Vec<u32>,
    note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelNote {
    step: u32,
    note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatus {
    Done,
    Partial,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Requirement {
    pub text: String,
    pub status: RequirementStatus,
    /// Índices en el plan de los pasos que lo cumplen.
    pub steps: Vec<usize>,
    pub note: Option<String>,
}

/// Cobertura de un turno, referida al plan actual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub turn: usize,
    pub model: String,
    pub requirements: Vec<Requirement>,
    /// Pasos (índice en el plan) que hacen algo que no se pidió, con qué hacen.
    pub unrequested: Vec<(usize, String)>,
    /// El turno cambió desde que se generó.
    pub stale: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CoverageReport {
    pub covered: usize,
    pub cached: usize,
    pub failures: Vec<String>,
}

fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "requirements": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "text": {"type": "string"},
                        "status": {"type": "string", "enum": ["done", "partial", "missing"]},
                        "steps": {"type": "array", "items": {"type": "integer"}},
                        "note": {"type": "string"}
                    },
                    "required": ["text", "status", "steps", "note"],
                    "additionalProperties": false
                }
            },
            "unrequested": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"step": {"type": "integer"}, "note": {"type": "string"}},
                    "required": ["step", "note"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["requirements", "unrequested"],
        "additionalProperties": false
    })
}

fn input_hash(model: &str, request: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    hasher.update(SYSTEM_PROMPT.as_bytes());
    hasher.update(request.as_bytes());
    hasher.finalize().to_hex()[..16].to_owned()
}

/// Calcula la cobertura de los turnos que no la tienen vigente. `progress`
/// recibe el turno antes de cada llamada al modelo.
///
/// # Errors
/// Si falla la base de datos. Los fallos del modelo se anotan en el informe.
pub fn cover(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    points: &[Attention],
    model: &(impl JsonModel + ?Sized),
    mut progress: impl FnMut(usize),
) -> crate::Result<CoverageReport> {
    let mut report = CoverageReport::default();
    let stored = store.coverages(&review.session_id)?;
    let model_id = model.model_id();
    for turn in reviewable_turns(plan) {
        let request = turn_request(review, plan, points, turn);
        let hash = input_hash(&model_id, &request);
        if stored.get(&turn).is_some_and(|s| s.input_hash == hash && validate(s.coverage.clone()).is_ok()) {
            report.cached += 1;
            continue;
        }
        progress(turn);
        match model.complete_json(SYSTEM_PROMPT, &request, &output_schema()).and_then(validate) {
            Ok(coverage) => {
                store.save_coverage(
                    &review.session_id,
                    turn,
                    &StoredCoverage { input_hash: hash, model: model_id.clone(), coverage },
                )?;
                report.covered += 1;
            }
            Err(error) => report.failures.push(format!("turno {}: {error}", turn + 1)),
        }
    }
    Ok(report)
}

/// Una respuesta sin requisitos no sirve: se descarta para volver a pedirla.
fn validate(text: String) -> Result<String, ExtractError> {
    let output: ModelOutput =
        serde_json::from_str(&text).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
    if output.requirements.is_empty() {
        return Err(ExtractError::BadResponse("el modelo no encontró ningún requisito".into()));
    }
    Ok(text)
}

/// Coberturas guardadas, referidas al plan actual. Las de turnos que han
/// cambiado se marcan como desfasadas en lugar de descartarse.
///
/// # Errors
/// Si falla la base de datos.
pub fn coverages(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    points: &[Attention],
) -> crate::Result<BTreeMap<usize, Coverage>> {
    let mut result = BTreeMap::new();
    for (turn, stored) in store.coverages(&review.session_id)? {
        let Ok(output) = serde_json::from_str::<ModelOutput>(&stored.coverage) else { continue };
        let steps = turn_steps(plan, turn);
        let to_plan = |number: u32| {
            usize::try_from(number).ok().and_then(|n| n.checked_sub(1)).and_then(|n| steps.get(n).copied())
        };
        let current = input_hash(&stored.model, &turn_request(review, plan, points, turn));
        let requirements = output
            .requirements
            .into_iter()
            .map(|r| Requirement {
                status: match r.status.as_str() {
                    "done" => RequirementStatus::Done,
                    "partial" => RequirementStatus::Partial,
                    _ => RequirementStatus::Missing,
                },
                steps: r.steps.into_iter().filter_map(to_plan).collect(),
                note: Some(r.note).filter(|note| !note.trim().is_empty()),
                text: r.text,
            })
            .collect();
        result.insert(
            turn,
            Coverage {
                turn,
                model: stored.model,
                requirements,
                unrequested: output.unrequested.into_iter().filter_map(|n| Some((to_plan(n.step)?, n.note))).collect(),
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
        calls: RefCell<usize>,
        answer: String,
    }

    impl JsonModel for Fake {
        fn model_id(&self) -> String {
            "fake:1".into()
        }

        fn complete_json(&self, system: &str, user: &str, schema: &Value) -> Result<String, ExtractError> {
            assert_eq!(system, SYSTEM_PROMPT);
            assert_eq!(schema["required"][0], "requirements");
            assert!(user.contains("valida el email"), "la petición va al modelo");
            *self.calls.borrow_mut() += 1;
            Ok(self.answer.clone())
        }
    }

    #[test]
    fn requirements_point_to_plan_steps_and_are_cached_until_the_turn_changes() {
        let edit = FileEdit {
            origin: EditOrigin { agent: "t", session_id: "s".into(), event_id: "e".into(), timestamp: None, cwd: None },
            path: PathBuf::from("/r/signup.ts"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known("export function signup(email: string) {\n  return save(email);\n}\n".into()),
            user_modified: false,
            context: EditContext { prompt: Some("valida el email y guárdalo".into()), rationale: None },
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let store = Store::open_in_memory().unwrap();
        let model = Fake {
            calls: RefCell::new(0),
            answer: json!({
                "requirements": [
                    {"text": "Guardar el email", "status": "done", "steps": [1], "note": ""},
                    {"text": "Validar el email", "status": "missing", "steps": [], "note": "No hay validación"}
                ],
                "unrequested": [{"step": 9, "note": "paso que no existe"}]
            })
            .to_string(),
        };

        let report = cover(&store, &review, &plan, &[], &model, |_| {}).unwrap();
        assert_eq!((report.covered, report.cached), (1, 0));
        let again = cover(&store, &review, &plan, &[], &model, |_| {}).unwrap();
        assert_eq!((again.covered, again.cached), (0, 1), "no se vuelve a pedir si nada cambió");
        assert_eq!(*model.calls.borrow(), 1);

        let coverage = &coverages(&store, &review, &plan, &[]).unwrap()[&0];
        assert_eq!(coverage.requirements[0].status, RequirementStatus::Done);
        assert_eq!(coverage.requirements[0].steps, vec![0]);
        assert_eq!(coverage.requirements[0].note, None);
        assert_eq!(coverage.requirements[1].status, RequirementStatus::Missing);
        assert!(coverage.unrequested.is_empty(), "un paso inexistente se ignora");
        assert!(!coverage.stale);
    }
}
