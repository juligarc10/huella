//! El guion de una revisión: el cambio contado como una presentación. Los
//! requisitos de lo que se pidió, con lo que los cumple; capítulos en el orden
//! en que conviene entender lo añadido (de la entrada a la lógica, la
//! persistencia y los efectos, cruzando repositorios), y lo que queda fuera.
//!
//! Lo escribe un modelo una vez por revisión y se guarda con el hash de lo que
//! se le envió: si el cambio cambia, se marca como desfasado. Sin modelo, el
//! guion es un capítulo por repositorio en el orden del plan.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use huella_decisions::{ExtractError, JsonModel};
use huella_review::flow::{FlowStep, StepRole};
use huella_review::{DiffLine, SessionReview};
use huella_store::{Store, StoredStoryline};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::coverage::RequirementStatus;
use crate::declared::DeclaredView;
use crate::triage::{Level, StepTriage};
use crate::{EngineError, Result};

pub const SYSTEM_PROMPT: &str = "\
Preparas la presentación de un cambio para que una persona desarrolladora lo entienda antes de aceptarlo. Recibes lo \
que pide la tarea y los pasos del cambio, numerados, en todos los repositorios que toca: su diff, su nivel de \
riesgo y lo que declaró el agente que los escribió.

Devuelve:
- summary: 2 a 4 frases que cuenten qué hace el cambio en conjunto, como un flujo.
- requirements: lo que pide la tarea troceado en requisitos concretos y comprobables, en el orden en que se piden. \
status: done si los pasos lo cumplen, partial si a medias, missing si ninguno; steps: los números de los pasos que \
lo cumplen; note: qué falta o qué conviene comprobar (vacío si nada).
- chapters: el cambio contado en capítulos, en el orden en que conviene entenderlo: siguiendo el funcionamiento, \
de la entrada (endpoint, pantalla, comando, evento) a la lógica, la persistencia y los efectos, cruzando \
repositorios si uno llama a otro; los tests y la configuración, al final. Cada capítulo es una intención: title \
corto; intent, 1 o 2 frases con qué hace y para qué; narration, 2 a 5 frases que lleven de un paso al siguiente; \
requirements, los números de los requisitos que cubre; steps, los números de sus pasos en orden de lectura. Cada \
paso va en un solo capítulo y deben estar todos.
- outside: pasos que la tarea no pide ni se deducen de ella (step y note con qué hacen). Los tests, la configuración \
y los imports que lo pedido necesita no cuentan.
- risks: riesgos concretos que se vean en el diff (errores sin manejar, casos límite, seguridad, datos).

Juzga solo por el diff, la descripción y lo declarado; lo declarado es la versión del agente, contrástala. Si algo \
no se ve en el diff, es partial o missing. Escribe en el idioma de la descripción.";

/// Presupuesto aproximado de líneas de diff para toda la petición.
const DIFF_BUDGET: usize = 2400;
/// Líneas de diff por paso, como mucho y como poco.
const STEP_LINES: (usize, usize) = (60, 6);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelOutput {
    summary: String,
    requirements: Vec<ModelRequirement>,
    chapters: Vec<ModelChapter>,
    outside: Vec<ModelNote>,
    risks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelRequirement {
    text: String,
    status: String,
    steps: Vec<u32>,
    note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelChapter {
    title: String,
    intent: String,
    narration: String,
    requirements: Vec<u32>,
    steps: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ModelNote {
    step: u32,
    note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Requirement {
    pub text: String,
    pub status: RequirementStatus,
    /// Índices en el plan.
    pub steps: Vec<usize>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chapter {
    pub title: String,
    pub intent: String,
    pub narration: String,
    /// Índices en `requirements`.
    pub requirements: Vec<usize>,
    /// Índices en el plan, en orden de lectura.
    pub steps: Vec<usize>,
}

/// El guion, referido al plan actual.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Storyline {
    /// Modelo que lo escribió; `None` en el guion automático.
    pub model: Option<String>,
    pub summary: String,
    pub requirements: Vec<Requirement>,
    pub chapters: Vec<Chapter>,
    /// Pasos fuera de lo que se pidió, con qué hacen.
    pub outside: Vec<(usize, String)>,
    pub risks: Vec<String>,
    /// El cambio no es el mismo que cuando se generó.
    pub stale: bool,
}

/// Lo que se sabe de la revisión además de sus pasos.
#[derive(Debug, Clone, Copy)]
pub struct StoryInput<'a> {
    pub title: Option<&'a str>,
    pub description: &'a str,
    /// Raíces de los repositorios (una en una sesión).
    pub repos: &'a [String],
    pub triage: &'a [StepTriage],
    pub declared: &'a BTreeMap<usize, Vec<DeclaredView>>,
}

fn output_schema() -> Value {
    let numbers = json!({"type": "array", "items": {"type": "integer"}});
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "requirements": {"type": "array", "items": {
                "type": "object",
                "properties": {"text": {"type": "string"}, "status": {"type": "string", "enum": ["done", "partial", "missing"]},
                               "steps": numbers, "note": {"type": "string"}},
                "required": ["text", "status", "steps", "note"], "additionalProperties": false}},
            "chapters": {"type": "array", "items": {
                "type": "object",
                "properties": {"title": {"type": "string"}, "intent": {"type": "string"}, "narration": {"type": "string"},
                               "requirements": numbers, "steps": numbers},
                "required": ["title", "intent", "narration", "requirements", "steps"], "additionalProperties": false}},
            "outside": {"type": "array", "items": {
                "type": "object",
                "properties": {"step": {"type": "integer"}, "note": {"type": "string"}},
                "required": ["step", "note"], "additionalProperties": false}},
            "risks": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["summary", "requirements", "chapters", "outside", "risks"],
        "additionalProperties": false
    })
}

/// Repositorio de un fichero (por prefijo de ruta) y su ruta dentro de él.
fn place<'a>(repos: &'a [String], path: &'a str) -> (Option<&'a str>, &'a str) {
    repos
        .iter()
        .find_map(|repo| {
            let inside = path.strip_prefix(repo.as_str())?.strip_prefix('/')?;
            Some((Some(repo.rsplit('/').next().unwrap_or(repo)), inside))
        })
        .unwrap_or((None, path))
}

/// Lo que se envía al modelo.
#[must_use]
pub fn request(review: &SessionReview, plan: &[FlowStep], input: StoryInput<'_>) -> String {
    let mut out = String::new();
    let description = input.description.trim();
    if let Some(title) = input.title.filter(|title| !title.trim().is_empty()) {
        let _ = writeln!(out, "Tarea: {title}");
    }
    let _ = writeln!(
        out,
        "Lo que pide la tarea:\n{}\n",
        if description.is_empty() { "(sin descripción: deduce la intención del código)" } else { description }
    );
    let per_step = (DIFF_BUDGET / plan.len().max(1)).clamp(STEP_LINES.1, STEP_LINES.0);
    out.push_str("Pasos del cambio, en el orden del flujo dentro de cada repositorio:\n");
    for (index, step) in plan.iter().enumerate() {
        let (repo, path) = place(input.repos, &review.files[step.file].path);
        let name = step.symbol.as_ref().map_or("(fuera de funciones)", |symbol| symbol.qualified_name.as_str());
        let role = match step.role {
            StepRole::Flow => "código",
            StepRole::Test => "test",
            StepRole::Other => "otros cambios",
        };
        let level = match input.triage.get(index).map(|triaged| triaged.level) {
            Some(Level::Must) => "obligatorio",
            Some(Level::Acknowledge) => "vistazo",
            Some(Level::Skip) => "saltable",
            None => "",
        };
        let tags: Vec<&str> = input
            .triage
            .get(index)
            .map(|triaged| triaged.findings.iter().map(|finding| finding.tag.label()).collect())
            .unwrap_or_default();
        let _ = write!(out, "\n[{}] ", index + 1);
        if let Some(repo) = repo {
            let _ = write!(out, "{repo} · ");
        }
        let _ = write!(out, "{path} · {name} · {role} · +{} −{} · {level}", step.added(), step.removed());
        if !tags.is_empty() {
            let _ = write!(out, " ({})", tags.join(", "));
        }
        let calls: Vec<String> = step.calls.iter().map(|call| format!("[{}]", call + 1)).collect();
        if !calls.is_empty() {
            let _ = write!(out, " · llama a {}", calls.join(", "));
        }
        out.push('\n');
        for entry in input.declared.get(&index).into_iter().flatten().take(2) {
            let _ = writeln!(out, "  Declaró el agente: {}", entry.declaration.summary);
        }
        out.push_str("```diff\n");
        let changed: Vec<&DiffLine> = step.lines.iter().filter(|line| !matches!(line, DiffLine::Context(_))).collect();
        for line in changed.iter().take(per_step) {
            let _ = match line {
                DiffLine::Added { text, .. } => writeln!(out, "+{text}"),
                DiffLine::Removed(text) => writeln!(out, "-{text}"),
                DiffLine::Context(_) => Ok(()),
            };
        }
        if changed.len() > per_step {
            let _ = writeln!(out, "… ({} líneas más)", changed.len() - per_step);
        }
        out.push_str("```\n");
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

/// Escribe el guion si no hay uno vigente. Devuelve `false` si ya lo había.
///
/// # Errors
/// Si falla la base de datos o el modelo.
pub fn generate(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    input: StoryInput<'_>,
    model: &(impl JsonModel + ?Sized),
) -> Result<bool> {
    let model_id = model.model_id();
    let request = request(review, plan, input);
    let hash = input_hash(&model_id, &request);
    if store.storyline(&review.session_id)?.is_some_and(|stored| stored.input_hash == hash) {
        return Ok(false);
    }
    let text = model
        .complete_json(SYSTEM_PROMPT, &request, &output_schema())
        .and_then(validate)
        .map_err(|error| EngineError::Model(error.to_string()))?;
    store
        .save_storyline(&review.session_id, &StoredStoryline { input_hash: hash, model: model_id, storyline: text })?;
    Ok(true)
}

/// Un guion sin capítulos no sirve.
fn validate(text: String) -> std::result::Result<String, ExtractError> {
    let output: ModelOutput =
        serde_json::from_str(&text).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
    if output.chapters.is_empty() {
        return Err(ExtractError::BadResponse("el modelo no escribió ningún capítulo".into()));
    }
    Ok(text)
}

/// El guion guardado, referido al plan actual, o `None` si no hay.
///
/// # Errors
/// Si falla la base de datos.
pub fn storyline(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    input: StoryInput<'_>,
) -> Result<Option<Storyline>> {
    let Some(stored) = store.storyline(&review.session_id)? else { return Ok(None) };
    let Ok(output) = serde_json::from_str::<ModelOutput>(&stored.storyline) else { return Ok(None) };
    let current = input_hash(&stored.model, &request(review, plan, input));
    let to_plan = |number: u32| usize::try_from(number).ok().and_then(|n| n.checked_sub(1)).filter(|&n| n < plan.len());
    let steps_of = |numbers: &[u32]| -> Vec<usize> { numbers.iter().filter_map(|&n| to_plan(n)).collect() };
    let requirements: Vec<Requirement> = output
        .requirements
        .into_iter()
        .map(|requirement| Requirement {
            status: match requirement.status.as_str() {
                "done" => RequirementStatus::Done,
                "partial" => RequirementStatus::Partial,
                _ => RequirementStatus::Missing,
            },
            steps: steps_of(&requirement.steps),
            note: Some(requirement.note).filter(|note| !note.trim().is_empty()),
            text: requirement.text,
        })
        .collect();
    let mut placed = BTreeSet::new();
    let mut chapters: Vec<Chapter> = output
        .chapters
        .into_iter()
        .map(|chapter| Chapter {
            // Cada paso, en el primer capítulo que lo nombra.
            steps: steps_of(&chapter.steps).into_iter().filter(|&step| placed.insert(step)).collect(),
            requirements: chapter
                .requirements
                .iter()
                .filter_map(|&n| usize::try_from(n).ok()?.checked_sub(1))
                .filter(|&n| n < requirements.len())
                .collect(),
            title: chapter.title,
            intent: chapter.intent,
            narration: chapter.narration,
        })
        .filter(|chapter| !chapter.steps.is_empty())
        .collect();
    // Lo que el modelo no colocó (o es nuevo) no se pierde: va al final.
    let rest: Vec<usize> = (0..plan.len()).filter(|step| !placed.contains(step)).collect();
    if !rest.is_empty() {
        chapters.push(Chapter {
            title: "Otros cambios".into(),
            intent: "Pasos que el guion no colocó en ningún capítulo.".into(),
            narration: String::new(),
            requirements: Vec::new(),
            steps: rest,
        });
    }
    Ok(Some(Storyline {
        model: Some(stored.model.clone()),
        summary: output.summary,
        requirements,
        chapters,
        outside: output.outside.into_iter().filter_map(|note| Some((to_plan(note.step)?, note.note))).collect(),
        risks: output.risks,
        stale: current != stored.input_hash,
    }))
}

/// Guion sin modelo: un capítulo por repositorio, en el orden del plan.
#[must_use]
pub fn fallback(review: &SessionReview, plan: &[FlowStep], repos: &[String]) -> Storyline {
    let mut chapters: Vec<Chapter> = Vec::new();
    for (index, step) in plan.iter().enumerate() {
        let (repo, _) = place(repos, &review.files[step.file].path);
        let title = repo.unwrap_or("Cambios").to_owned();
        match chapters.iter_mut().find(|chapter| chapter.title == title) {
            Some(chapter) => chapter.steps.push(index),
            None => chapters.push(Chapter {
                title,
                intent: String::new(),
                narration: String::new(),
                requirements: Vec::new(),
                steps: vec![index],
            }),
        }
    }
    Storyline {
        model: None,
        summary: String::new(),
        requirements: Vec::new(),
        chapters,
        outside: Vec::new(),
        risks: Vec::new(),
        stale: false,
    }
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

        fn complete_json(&self, system: &str, user: &str, schema: &Value) -> std::result::Result<String, ExtractError> {
            assert_eq!(system, SYSTEM_PROMPT);
            assert_eq!(schema["required"][2], "chapters");
            assert!(user.contains("Lo que pide la tarea:\nSumar los precios"), "{user}");
            *self.calls.borrow_mut() += 1;
            Ok(self.answer.clone())
        }
    }

    fn review_of(files: &[(&str, &str)]) -> (SessionReview, Vec<FlowStep>) {
        let edits: Vec<FileEdit> = files
            .iter()
            .enumerate()
            .map(|(n, (path, content))| FileEdit {
                origin: EditOrigin {
                    agent: "t",
                    session_id: "task:A-1".into(),
                    event_id: format!("e{n}"),
                    timestamp: None,
                    cwd: None,
                },
                path: PathBuf::from(path),
                kind: EditKind::Create,
                before: None,
                after: AfterState::Known((*content).into()),
                user_modified: false,
                context: EditContext::default(),
            })
            .collect();
        let review = huella_review::build("task:A-1", &edits, |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    /// Respuesta del modelo: `total` es el paso número `total`, `other` el `other`.
    fn answer(total: usize, other: usize) -> String {
        format!(
            r#"{{"summary":"Suma precios.","requirements":[{{"text":"Sumar los precios","status":"done","steps":[{total}],"note":""}}],
            "chapters":[{{"title":"El cálculo","intent":"Suma","narration":"Primero…","requirements":[1,7],"steps":[{total},{total},99]}}],
            "outside":[{{"step":{other},"note":"cambia otro módulo"}}],"risks":["precios negativos"]}}"#
        )
    }

    #[test]
    fn the_storyline_is_cached_and_every_step_ends_up_in_a_chapter() {
        let store = Store::open_in_memory().unwrap();
        let (review, plan) = review_of(&[
            (
                "/w/billing/total.ts",
                "export function total(items) {\n  return items.reduce((s, i) => s + i.price, 0);\n}\n",
            ),
            ("/w/api/other.ts", "export function other() {\n  return 1;\n}\n"),
        ]);
        let repos = vec!["/w/billing".to_owned(), "/w/api".to_owned()];
        let declared = BTreeMap::new();
        let input = StoryInput {
            title: None,
            description: "Sumar los precios",
            repos: &repos,
            triage: &[],
            declared: &declared,
        };
        let text = request(&review, &plan, input);
        assert!(text.contains("billing · total.ts · total"), "{text}");
        let total = plan.iter().position(|s| s.symbol.as_ref().is_some_and(|s| s.qualified_name == "total")).unwrap();
        let other = plan.iter().position(|s| s.symbol.as_ref().is_some_and(|s| s.qualified_name == "other")).unwrap();
        let model = Fake { calls: RefCell::new(0), answer: answer(total + 1, other + 1) };
        assert!(generate(&store, &review, &plan, input, &model).unwrap());
        assert!(!generate(&store, &review, &plan, input, &model).unwrap());
        assert_eq!(*model.calls.borrow(), 1);

        let found = storyline(&store, &review, &plan, input).unwrap().unwrap();
        assert!(!found.stale);
        assert_eq!(found.chapters[0].steps, vec![total], "sin repetidos ni números fuera del plan");
        assert_eq!(found.chapters[0].requirements, vec![0]);
        assert_eq!(found.chapters.last().unwrap().title, "Otros cambios", "lo no colocado no se pierde");
        assert_eq!(found.chapters.iter().map(|c| c.steps.len()).sum::<usize>(), plan.len());
        assert_eq!(found.requirements[0].status, RequirementStatus::Done);
        assert_eq!(found.outside.len(), 1);

        let changed = StoryInput { description: "Sumar los precios con IVA", ..input };
        assert!(storyline(&store, &review, &plan, changed).unwrap().unwrap().stale);
    }

    #[test]
    fn without_a_model_there_is_one_chapter_per_repository() {
        let (review, plan) =
            review_of(&[("/w/billing/a.ts", "export const a = 1;\n"), ("/w/api/b.ts", "export const b = 2;\n")]);
        let repos = vec!["/w/billing".to_owned(), "/w/api".to_owned()];
        let story = fallback(&review, &plan, &repos);
        let titles: BTreeSet<&str> = story.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, BTreeSet::from(["api", "billing"]));
        assert!(story.model.is_none());
    }
}
