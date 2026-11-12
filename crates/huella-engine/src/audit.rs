//! Contrastar el guion con el diff: el guion es la versión del agente que hizo
//! el cambio, y quien revisa necesita saber dónde no cuadra con el código.
//! El modelo revisor recibe lo que dice el guion y los pasos del cambio, y
//! devuelve lo que no encaja, anclado al paso cuando lo hay.

use std::fmt::Write as _;

use huella_decisions::JsonModel;
use huella_review::SessionReview;
use huella_review::flow::FlowStep;
use huella_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::storyline::{StoryInput, Storyline};
use crate::{EngineError, Result};

pub const SYSTEM: &str = "\
Contrastas lo que cuenta el agente que hizo un cambio (su guion) con lo que de verdad hace el código. Recibes lo que \
pide la tarea, el guion y los pasos del cambio, numerados, con su diff.

Devuelve findings: lo que no cuadra, cada uno con:
- kind: mismatch (el código no hace lo que dice el guion), missing (el guion lo da por hecho y no se ve en el diff) \
o untold (el diff hace algo importante que el guion no cuenta).
- step: el número del paso donde se ve, o -1 si no hay uno concreto.
- claim: lo que dice el guion (o, en untold, qué hace el código), en una frase.
- finding: qué se ve en el código y por qué no encaja, en 1 a 3 frases concretas que citen nombres del código.

Solo diferencias reales y comprobables en lo que recibes: si todo cuadra, devuelve una lista vacía. No repitas lo \
mismo con otras palabras. Escribe en el idioma de la tarea.";

/// Lo que no cuadra entre el guion y el código.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// `mismatch`, `missing` o `untold`.
    pub kind: String,
    /// Paso del plan, si hay uno concreto.
    pub step: Option<usize>,
    pub claim: String,
    pub finding: String,
}

/// Una auditoría guardada y si el cambio sigue siendo el mismo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Audit {
    pub findings: Vec<Finding>,
    pub model: String,
    pub recorded_at: String,
    /// El cambio o el guion no son los de cuando se contrastó.
    pub stale: bool,
}

/// Lo que dice el guion, como lo contrasta el modelo: requisitos que da por
/// cumplidos y lo que cuenta cada idea, con sus pasos.
fn claims(story: &Storyline) -> String {
    let mut out = crate::guide::render_overview(story);
    out.push_str("\nRequisitos según el guion:\n");
    for requirement in &story.requirements {
        let steps: Vec<String> = requirement.steps.iter().map(|step| format!("[{step}]")).collect();
        let _ = writeln!(out, "- ({:?}) {} · pasos {}", requirement.status, requirement.text, steps.join(", "));
    }
    out.push_str("\nLo que cuenta cada parte:\n");
    for chapter in &story.chapters {
        let _ = writeln!(out, "- {}: {} {}", chapter.title, chapter.intent, chapter.narration);
        for idea in &chapter.ideas {
            let steps: Vec<String> = idea.steps.iter().map(|step| format!("[{step}]")).collect();
            let points: Vec<&str> = idea.points.iter().map(|point| point.text.as_str()).collect();
            let _ = writeln!(
                out,
                "  - {} (pasos {}): {} {} {} {}",
                idea.title,
                steps.join(", "),
                idea.explanation,
                idea.what,
                idea.why,
                points.join("; ")
            );
        }
    }
    out
}

fn request(
    review: &SessionReview,
    plan: &[FlowStep],
    input: StoryInput<'_>,
    story: &Storyline,
    budget: usize,
) -> String {
    let told = claims(story);
    let diff = crate::storyline::request(review, plan, input);
    // Lo que no cabe se recorta: mejor contrastar casi todo que no poder hacerlo.
    let room = budget.saturating_sub(told.len()).max(4_000);
    let diff: String = if diff.chars().count() > room {
        let cut: String = diff.chars().take(room).collect();
        format!("{cut}\n(el resto del cambio no cabe: contrasta solo lo que ves)\n")
    } else {
        diff
    };
    format!("## El guion del agente\n\n{told}\n## El cambio\n\n{diff}")
}

fn hash(model: &str, request: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    hasher.update(SYSTEM.as_bytes());
    hasher.update(request.as_bytes());
    hasher.finalize().to_hex()[..16].to_owned()
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"findings": {"type": "array", "items": {
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["mismatch", "missing", "untold"]},
                "step": {"type": "integer"},
                "claim": {"type": "string"},
                "finding": {"type": "string"}
            },
            "required": ["kind", "step", "claim", "finding"],
            "additionalProperties": false
        }}},
        "required": ["findings"],
        "additionalProperties": false
    })
}

/// Contrasta el guion con el cambio y guarda lo que no cuadra.
///
/// # Errors
/// Si falla el modelo o la base de datos.
pub fn run(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    input: StoryInput<'_>,
    told: &Storyline,
    model: &dyn JsonModel,
    context_tokens: Option<usize>,
) -> Result<Vec<Finding>> {
    let request = request(review, plan, input, told, crate::storyline::request_chars(context_tokens));
    let text =
        model.complete_json(SYSTEM, &request, &schema()).map_err(|error| EngineError::Model(error.to_string()))?;
    let parsed: Value = serde_json::from_str(&text).map_err(|error| EngineError::Model(error.to_string()))?;
    let findings: Vec<Finding> = parsed["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let kind = item["kind"].as_str()?;
            let finding = item["finding"].as_str()?.trim();
            (["mismatch", "missing", "untold"].contains(&kind) && !finding.is_empty()).then(|| Finding {
                kind: kind.to_owned(),
                // Un paso que no existe es como no dar ninguno.
                step: item["step"]
                    .as_u64()
                    .and_then(|step| usize::try_from(step).ok())
                    .filter(|step| *step < plan.len()),
                claim: item["claim"].as_str().unwrap_or_default().trim().to_owned(),
                finding: finding.to_owned(),
            })
        })
        .collect();
    let model_id = model.model_id();
    let text = serde_json::to_string(&findings).map_err(|error| EngineError::Invalid(error.to_string()))?;
    store.save_audit(&review.session_id, &model_id, &hash(&model_id, &request), &text)?;
    Ok(findings)
}

/// La auditoría guardada de una revisión, y si sigue vigente.
///
/// # Errors
/// Si falla la base de datos.
pub fn stored(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    input: StoryInput<'_>,
    told: &Storyline,
    context_tokens: Option<usize>,
) -> Result<Option<Audit>> {
    let Some(saved) = store.audit(&review.session_id)? else { return Ok(None) };
    let request = request(review, plan, input, told, crate::storyline::request_chars(context_tokens));
    Ok(Some(Audit {
        findings: serde_json::from_str(&saved.findings).unwrap_or_default(),
        stale: hash(&saved.model, &request) != saved.input_hash,
        model: saved.model,
        recorded_at: saved.recorded_at,
    }))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use huella_decisions::ExtractError;

    use super::*;

    struct Fake(RefCell<String>);

    impl JsonModel for Fake {
        fn model_id(&self) -> String {
            "fake:1".into()
        }
        fn complete_json(&self, system: &str, user: &str, _: &Value) -> std::result::Result<String, ExtractError> {
            assert_eq!(system, SYSTEM);
            user.clone_into(&mut self.0.borrow_mut());
            Ok(json!({"findings": [
                {"kind": "missing", "step": 0, "claim": "Valida el correo", "finding": "No hay ninguna validación en `save`."},
                {"kind": "untold", "step": 99, "claim": "Borra la caché", "finding": "Se vacía `cache` al guardar."},
                {"kind": "inventado", "step": 0, "claim": "x", "finding": "y"},
                {"kind": "mismatch", "step": -1, "claim": "z", "finding": " "}
            ]})
            .to_string())
        }
    }

    #[test]
    fn findings_are_kept_with_valid_steps_and_saved_until_the_change_moves() {
        let edit = huella_core::FileEdit {
            origin: huella_core::EditOrigin {
                agent: "t",
                session_id: "task:A".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: "/r/save.ts".into(),
            kind: huella_core::EditKind::Replace,
            before: Some("function save(user) {\n  return db.put(user);\n}\n".into()),
            after: huella_core::AfterState::Known(
                "function save(user) {\n  cache.clear();\n  return db.put(user);\n}\n".into(),
            ),
            user_modified: false,
            context: huella_core::EditContext::default(),
        };
        let review = huella_review::build("task:A", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let told = crate::storyline::fallback(&review, &plan, &[]);
        let triage = Vec::new();
        let declared = std::collections::BTreeMap::new();
        let input = StoryInput {
            title: Some("Alta"),
            description: "Validar el correo al guardar",
            repos: &[],
            triage: &triage,
            declared: &declared,
        };
        let store = Store::open_in_memory().unwrap();
        let model = Fake(RefCell::new(String::new()));

        let found = run(&store, &review, &plan, input, &told, &model, None).unwrap();
        assert_eq!(found.len(), 2, "sin tipos inventados ni hallazgos vacíos: {found:?}");
        assert_eq!(found[0].step, Some(0));
        assert_eq!(found[1].step, None, "un paso que no existe no se ancla");
        let seen = model.0.borrow().clone();
        assert!(seen.contains("## El guion del agente") && seen.contains("cache.clear()"), "{seen}");

        let saved = stored(&store, &review, &plan, input, &told, None).unwrap().unwrap();
        assert_eq!((saved.findings.len(), saved.stale), (2, false));
        let other = StoryInput { description: "Otra cosa", ..input };
        assert!(stored(&store, &review, &plan, other, &told, None).unwrap().unwrap().stale, "la tarea cambió");
    }
}
