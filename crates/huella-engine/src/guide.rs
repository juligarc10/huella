//! Revisión guiada: para cada paso, un modelo explica qué hace el código, lleva
//! a la persona por él en orden de lectura y le dice qué comprobar, centrándose
//! en por qué el triaje lo marcó. Todo va anclado a líneas del diff del paso, que
//! son estables mientras el paso no cambie; si cambia, la guía queda desfasada.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use huella_decisions::{ExtractError, JsonModel};
use huella_review::flow::{FlowStep, StepRole};
use huella_review::{DiffLine, SessionReview, Step};
use huella_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evidence::StepEvidence;
use crate::triage::{Level, StepTriage};
use crate::{EngineError, Result};

/// Líneas de diff que se envían al modelo.
const MAX_DIFF_LINES: usize = 200;
/// Razonamientos del agente que se envían, de los más recientes.
const MAX_REASONING: usize = 3;

pub const SYSTEM_PROMPT: &str = "\
Guías a una persona desarrolladora por un trozo de código que escribió un agente de IA, para que lo entienda antes \
de aceptarlo. Recibes su petición, lo que pensó y explicó el agente, por qué se marcó este paso para revisarlo y el \
diff del paso con las líneas numeradas (L1, L2…).

Devuelve:
- explanation: 2 a 4 frases que digan qué hace este código, para qué lo necesita la petición y cómo encaja con lo que \
llama. Sin repetir el código línea a línea.
- walkthrough: de 1 a 6 paradas en el orden en que conviene leer el diff. from y to son los números de línea (L) que \
abarca la parada; note, en una frase, qué pasa ahí y por qué.
- checks: de 1 a 4 comprobaciones concretas que la persona debe hacer antes de aceptarlo, empezando por los motivos \
por los que se marcó. Cada una empieza por un verbo («Comprueba que…», «Mira si…») y dice qué mirar, no la \
respuesta. from y to son las líneas a las que se refiere, o 0 y 0 si es del paso entero.

Escribe en el idioma de la petición. No afirmes nada que no se vea en el diff o en lo que explicó el agente; si algo \
no se puede saber desde aquí, conviértelo en una comprobación.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelOutput {
    explanation: String,
    walkthrough: Vec<ModelStop>,
    checks: Vec<ModelCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelStop {
    from: u32,
    to: u32,
    note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelCheck {
    from: u32,
    to: u32,
    check: String,
}

/// Una parada del recorrido: líneas del diff (índices desde 0, inclusive).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stop {
    pub lines: (usize, usize),
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// `None` si se refiere al paso entero.
    pub lines: Option<(usize, usize)>,
    pub check: String,
    pub done: bool,
}

/// Guía de un paso, referida a su diff actual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Guide {
    pub model: String,
    pub explanation: String,
    pub walkthrough: Vec<Stop>,
    pub checks: Vec<Check>,
    /// El paso cambió desde que se generó: las líneas pueden no cuadrar.
    pub stale: bool,
}

fn output_schema() -> Value {
    let ranged = |text: &str| {
        json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {"from": {"type": "integer"}, "to": {"type": "integer"}, text: {"type": "string"}},
                "required": ["from", "to", text],
                "additionalProperties": false
            }
        })
    };
    json!({
        "type": "object",
        "properties": {"explanation": {"type": "string"}, "walkthrough": ranged("note"), "checks": ranged("check")},
        "required": ["explanation", "walkthrough", "checks"],
        "additionalProperties": false
    })
}

/// ¿Merece guía? Lo saltable no.
#[must_use]
pub fn wanted(triage: &StepTriage) -> bool {
    triage.level != Level::Skip
}

/// Lo que se envía al modelo para un paso.
#[must_use]
pub fn step_request(
    review: &SessionReview,
    plan: &[FlowStep],
    index: usize,
    evidence: &StepEvidence,
    triage: &StepTriage,
) -> String {
    let step = &plan[index];
    let mut out = String::new();
    let prompt = step.turn.and_then(|turn| review.turns.get(turn)).and_then(|turn| turn.prompt.as_deref());
    let _ = writeln!(out, "Petición de la persona:\n{}\n", prompt.unwrap_or("(sin petición)"));

    let mut rationales: Vec<&str> = Vec::new();
    for &source in &step.sources {
        if let Step::Agent { rationale: Some(text), .. } = &review.steps[source]
            && !rationales.contains(&text.as_str())
        {
            rationales.push(text);
        }
    }
    let thoughts = &evidence.reasoning[evidence.reasoning.len().saturating_sub(MAX_REASONING)..];
    if !rationales.is_empty() || !thoughts.is_empty() {
        out.push_str("Lo que pensó y explicó el agente antes de escribirlo:\n");
        for text in thoughts.iter().map(String::as_str).chain(rationales.iter().copied()) {
            let _ = writeln!(out, "- {}", clip(&one_line(text), 1500));
        }
        out.push('\n');
    }

    if triage.findings.is_empty() {
        out.push_str("Por qué se revisa: es código sin señales de riesgo; basta con entenderlo.\n\n");
    } else {
        out.push_str("Por qué se marcó para revisarlo:\n");
        for finding in &triage.findings {
            let _ = writeln!(out, "- {}", finding.reason);
        }
        out.push('\n');
    }

    let role = match step.role {
        StepRole::Flow => "código del flujo",
        StepRole::Test => "test",
        StepRole::Other => "otros cambios del fichero",
    };
    let name = step.symbol.as_ref().map_or("(fuera de funciones)", |symbol| symbol.qualified_name.as_str());
    let _ = writeln!(out, "Paso: {role} · {name} ({})", review.files[step.file].path);
    let calls: Vec<&str> = step
        .calls
        .iter()
        .filter_map(|&call| plan.get(call)?.symbol.as_ref().map(|symbol| symbol.qualified_name.as_str()))
        .collect();
    if !calls.is_empty() {
        let _ = writeln!(out, "Llama a: {}", calls.join(", "));
    }
    out.push_str("```diff\n");
    for (number, line) in step.lines.iter().take(MAX_DIFF_LINES).enumerate() {
        let _ = match line {
            DiffLine::Context(text) => writeln!(out, "L{}  {text}", number + 1),
            DiffLine::Added { text, .. } => writeln!(out, "L{} +{text}", number + 1),
            DiffLine::Removed(text) => writeln!(out, "L{} -{text}", number + 1),
        };
    }
    if step.lines.len() > MAX_DIFF_LINES {
        let _ = writeln!(out, "… ({} líneas más)", step.lines.len() - MAX_DIFF_LINES);
    }
    out.push_str("```\n");
    huella_core::redact::redact(&out).into_owned()
}

fn input_hash(model: &str, request: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    hasher.update(SYSTEM_PROMPT.as_bytes());
    hasher.update(request.as_bytes());
    hasher.finalize().to_hex()[..16].to_owned()
}

/// Genera la guía de un paso si no tiene una vigente. Devuelve `false` si ya
/// la tenía.
///
/// # Errors
/// Si falla la base de datos o el modelo.
pub fn guide_step(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    index: usize,
    evidence: &StepEvidence,
    triage: &StepTriage,
    model: &(impl JsonModel + ?Sized),
) -> Result<bool> {
    let key = plan[index].key(review);
    let model_id = model.model_id();
    let request = step_request(review, plan, index, evidence, triage);
    let hash = input_hash(&model_id, &request);
    if store.guides(&review.session_id)?.get(&key).is_some_and(|stored| stored.input_hash == hash) {
        return Ok(false);
    }
    let text = model
        .complete_json(SYSTEM_PROMPT, &request, &output_schema())
        .and_then(validate)
        .map_err(|error| EngineError::Model(error.to_string()))?;
    store.save_guide(&review.session_id, &key, &hash, &model_id, &text)?;
    Ok(true)
}

/// Una respuesta sin explicación no sirve.
fn validate(text: String) -> std::result::Result<String, ExtractError> {
    let output: ModelOutput =
        serde_json::from_str(&text).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
    if output.explanation.trim().is_empty() {
        return Err(ExtractError::BadResponse("el modelo no escribió la explicación".into()));
    }
    Ok(text)
}

/// Guías guardadas de la sesión, por índice en el plan. `context` da, para
/// cada paso, su evidencia y su triaje, con los que se sabe si sigue vigente.
///
/// # Errors
/// Si falla la base de datos.
pub fn guides(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    context: &[(&StepEvidence, &StepTriage)],
) -> Result<BTreeMap<usize, Guide>> {
    let stored = store.guides(&review.session_id)?;
    let mut result = BTreeMap::new();
    for (index, (step, (evidence, triage))) in plan.iter().zip(context).enumerate() {
        let Some(found) = stored.get(&step.key(review)) else { continue };
        let Ok(output) = serde_json::from_str::<ModelOutput>(&found.guide) else { continue };
        let current = input_hash(&found.model, &step_request(review, plan, index, evidence, triage));
        let range = |from: u32, to: u32| lines_of(from, to, step.lines.len());
        result.insert(
            index,
            Guide {
                model: found.model.clone(),
                explanation: output.explanation,
                walkthrough: output
                    .walkthrough
                    .into_iter()
                    .filter_map(|stop| Some(Stop { lines: range(stop.from, stop.to)?, note: stop.note }))
                    .collect(),
                checks: output
                    .checks
                    .into_iter()
                    .enumerate()
                    .map(|(number, check)| Check {
                        lines: range(check.from, check.to),
                        check: check.check,
                        done: found.checked.contains(&number),
                    })
                    .collect(),
                stale: current != found.input_hash,
            },
        );
    }
    Ok(result)
}

/// Líneas L`from`..L`to` como índices del diff, o `None` si no están en él.
fn lines_of(from: u32, to: u32, len: usize) -> Option<(usize, usize)> {
    let from = usize::try_from(from).ok()?.checked_sub(1)?;
    let to = usize::try_from(to).ok()?.checked_sub(1)?.max(from);
    (from < len).then_some((from, to.min(len - 1)))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;
    use crate::triage::{Policy, triage};

    struct Fake {
        calls: RefCell<Vec<String>>,
        answer: String,
    }

    impl JsonModel for Fake {
        fn model_id(&self) -> String {
            "fake:1".into()
        }

        fn complete_json(&self, system: &str, user: &str, schema: &Value) -> std::result::Result<String, ExtractError> {
            assert_eq!(system, SYSTEM_PROMPT);
            assert_eq!(schema["required"][0], "explanation");
            self.calls.borrow_mut().push(user.to_owned());
            Ok(self.answer.clone())
        }
    }

    fn review_of(content: &str) -> (SessionReview, Vec<FlowStep>) {
        let edit = FileEdit {
            origin: EditOrigin { agent: "t", session_id: "s".into(), event_id: "e".into(), timestamp: None, cwd: None },
            path: PathBuf::from("/r/src/auth/login.ts"),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(content.into()),
            user_modified: false,
            context: EditContext {
                prompt: Some("añade el login".into()),
                rationale: Some("comparo la contraseña con bcrypt".into()),
            },
        };
        let review = huella_review::build("s", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    const ANSWER: &str = r#"{"explanation":"Comprueba la contraseña.","walkthrough":[{"from":1,"to":2,"note":"entrada"},{"from":40,"to":41,"note":"fuera"}],"checks":[{"from":2,"to":2,"check":"Comprueba que el hash viene de la base de datos"},{"from":0,"to":0,"check":"Mira si hay tests"}]}"#;

    #[test]
    fn the_request_numbers_the_diff_and_says_why_the_step_was_flagged() {
        let (review, plan) =
            review_of("export function login(password: string) {\n  return bcrypt.compare(password, hash);\n}\n");
        let triaged = triage(&review, &plan, &Policy::default(), &BTreeSet::new(), &BTreeSet::new());
        let request = step_request(&review, &plan, 0, &StepEvidence::default(), &triaged[0]);
        assert!(request.contains("L1 +export function login"), "{request}");
        assert!(request.contains("toca seguridad"), "{request}");
        assert!(request.contains("comparo la contraseña con bcrypt"), "{request}");
    }

    #[test]
    fn guides_are_cached_anchored_to_the_diff_and_checked() {
        let store = Store::open_in_memory().unwrap();
        let (review, plan) =
            review_of("export function login(password: string) {\n  return bcrypt.compare(password, hash);\n}\n");
        let triaged = triage(&review, &plan, &Policy::default(), &BTreeSet::new(), &BTreeSet::new());
        let evidence = StepEvidence::default();
        let model = Fake { calls: RefCell::new(Vec::new()), answer: ANSWER.into() };

        assert!(guide_step(&store, &review, &plan, 0, &evidence, &triaged[0], &model).unwrap());
        assert!(!guide_step(&store, &review, &plan, 0, &evidence, &triaged[0], &model).unwrap());
        assert_eq!(model.calls.borrow().len(), 1);

        store.set_guide_check("s", &plan[0].key(&review), 1, true).unwrap();
        let guides = guides(&store, &review, &plan, &[(&evidence, &triaged[0])]).unwrap();
        let guide = &guides[&0];
        assert!(!guide.stale);
        // La parada fuera del diff se descarta; la comprobación del paso entero queda sin líneas.
        assert_eq!(guide.walkthrough.len(), 1);
        assert_eq!(guide.walkthrough[0].lines, (0, 1));
        assert_eq!((guide.checks[0].lines, guide.checks[0].done), (Some((1, 1)), false));
        assert_eq!((guide.checks[1].lines, guide.checks[1].done), (None, true));
    }

    #[test]
    fn a_changed_step_makes_its_guide_stale() {
        let store = Store::open_in_memory().unwrap();
        let (review, plan) = review_of("export function login(password: string) {\n  return check(password);\n}\n");
        let triaged = triage(&review, &plan, &Policy::default(), &BTreeSet::new(), &BTreeSet::new());
        let evidence = StepEvidence::default();
        let model = Fake { calls: RefCell::new(Vec::new()), answer: ANSWER.into() };
        guide_step(&store, &review, &plan, 0, &evidence, &triaged[0], &model).unwrap();

        let (changed, changed_plan) =
            review_of("export function login(password: string) {\n  return check(password.trim());\n}\n");
        let triaged = triage(&changed, &changed_plan, &Policy::default(), &BTreeSet::new(), &BTreeSet::new());
        let guides = guides(&store, &changed, &changed_plan, &[(&evidence, &triaged[0])]).unwrap();
        assert!(guides[&0].stale);
    }

    #[test]
    fn empty_explanations_are_rejected() {
        assert!(validate(r#"{"explanation":" ","walkthrough":[],"checks":[]}"#.into()).is_err());
    }
}
