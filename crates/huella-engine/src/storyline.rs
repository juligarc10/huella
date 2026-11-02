//! El guion de una revisión: el cambio contado como una presentación, en el
//! formato común `huella.storyline/v1` (`docs/huella-storyline-v1.schema.json`).
//!
//! Hay tres fuentes, por este orden:
//! 1. **El agente que hizo el cambio** lo declara al terminar (`huella_storyline`):
//!    es quien mejor sabe el porqué y el orden. Se ancla al código y se marca
//!    como desfasado si cambia; lo que no cuenta va a «No lo contó el agente».
//! 2. **Un modelo revisor** lo escribe a partir del diff y de lo que pide la tarea.
//! 3. **Sin modelo**, un capítulo por repositorio en el orden del plan.
//!
//! Las tres se leen con la misma estructura y se resuelven igual contra los
//! pasos de la revisión.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

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

/// El esquema del guion, tal como se publica y se da al agente.
pub const SCHEMA: &str = include_str!("../../../docs/huella-storyline-v1.schema.json");
pub const VERSION: &str = "huella.storyline/v1";

pub const SYSTEM_PROMPT: &str = "\
Preparas la presentación de un cambio para que una persona desarrolladora lo entienda antes de aceptarlo. Recibes lo \
que pide la tarea y los pasos del cambio, numerados, en todos los repositorios que toca: su diff, su nivel de \
riesgo y lo que declaró el agente que los escribió. Devuelves el guion en el formato huella.storyline/v1; donde \
se pide código (anchors, where, outside), usa { \"step\": número del paso }.

- summary: 2 a 4 frases que cuenten qué hace el cambio en conjunto y por qué, como un flujo.
- requirements: lo que pide la tarea troceado en requisitos comprobables, en su orden, con id R1, R2…; status \
done, partial o missing (si no se ve en el diff, no es done); where, los pasos que lo cumplen; note, qué falta.
- chapters: el cambio en capítulos en el orden en que conviene ENTENDERLO: de la entrada (endpoint, pantalla, \
comando, evento) a la lógica, los datos y los efectos, cruzando repositorios si uno llama a otro; tests y \
configuración al final. Cada uno: id C1, C2…; title corto; kind (entry, logic, data, effect, integration, test, \
config u other); intent (qué y para qué); narration (2 a 5 frases que lo enlacen con el anterior); anchors (sus \
pasos en orden de lectura; cada paso en un solo capítulo, y todos los pasos en alguno); requirements (ids que \
cubre); look_at (lo que no hay que pasar por alto); questions (lo que quien revisa debería saber responder).
- flow: cómo se conectan los capítulos al ejecutarse (from, to, label con qué pasa entre ellos).
- outside: pasos que la tarea no pide ni se deducen de ella, con why. Los tests, la configuración y los imports \
que lo pedido necesita no cuentan.
- unverified y risks: lo que no se ve probado y los riesgos concretos.

Juzga solo por el diff, la descripción y lo declarado; lo declarado es la versión del agente, contrástala. Escribe \
en el idioma de la descripción.";

/// Presupuesto aproximado de líneas de diff para toda la petición.
const DIFF_BUDGET: usize = 2400;
/// Líneas de diff por paso, como mucho y como poco.
const STEP_LINES: (usize, usize) = (60, 6);

// ---------------------------------------------------------------------------
// El formato común: huella.storyline/v1.

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Story {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub summary: String,
    pub requirements: Vec<StoryRequirement>,
    pub chapters: Vec<StoryChapter>,
    pub flow: Vec<StoryFlow>,
    pub outside: Vec<StoryOutside>,
    pub unverified: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryRequirement {
    pub id: String,
    pub text: String,
    pub status: String,
    #[serde(rename = "where")]
    pub places: Vec<Anchor>,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryChapter {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub intent: String,
    pub narration: String,
    pub anchors: Vec<Anchor>,
    pub requirements: Vec<String>,
    pub look_at: Vec<String>,
    pub questions: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryFlow {
    pub from: String,
    pub to: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryOutside {
    pub anchor: Anchor,
    pub why: String,
}

/// Un trozo de código del guion. `repo` y `snapshot` los pone Huella al
/// guardarlo: el repositorio (su directorio git común) y el texto de entonces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Anchor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

impl Anchor {
    fn range(&self) -> Option<(usize, usize)> {
        self.lines.as_deref().and_then(|lines| Some((*lines.first()?, *lines.get(1)?)))
    }
}

impl Story {
    fn anchors_mut(&mut self) -> impl Iterator<Item = &mut Anchor> {
        self.requirements
            .iter_mut()
            .flat_map(|requirement| requirement.places.iter_mut())
            .chain(self.chapters.iter_mut().flat_map(|chapter| chapter.anchors.iter_mut()))
            .chain(self.outside.iter_mut().map(|outside| &mut outside.anchor))
    }
}

// ---------------------------------------------------------------------------
// El guion resuelto contra los pasos de la revisión: lo que pinta la aplicación.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Lo contó el agente que hizo el cambio.
    Declared,
    /// Lo escribió un modelo revisor.
    Reviewer,
    /// Un capítulo por repositorio, sin modelo.
    Automatic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Requirement {
    pub id: String,
    pub text: String,
    pub status: RequirementStatus,
    /// Índices en el plan.
    pub steps: Vec<usize>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chapter {
    pub title: String,
    pub kind: Option<String>,
    pub intent: String,
    pub narration: String,
    /// Índices en `requirements`.
    pub requirements: Vec<usize>,
    /// Índices en el plan, en orden de lectura.
    pub steps: Vec<usize>,
    pub look_at: Vec<String>,
    pub questions: Vec<String>,
    /// Código del cambio que el guion del agente no menciona.
    pub unnarrated: bool,
}

impl Chapter {
    fn plain(title: String, intent: String, steps: Vec<usize>, unnarrated: bool) -> Self {
        Self {
            title,
            kind: None,
            intent,
            narration: String::new(),
            requirements: Vec::new(),
            steps,
            look_at: Vec::new(),
            questions: Vec::new(),
            unnarrated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Flow {
    /// Índices en `chapters`.
    pub from: usize,
    pub to: usize,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Storyline {
    pub source: Source,
    /// Modelo que lo escribió (revisor); `None` si lo declaró el agente o es automático.
    pub model: Option<String>,
    pub summary: String,
    pub requirements: Vec<Requirement>,
    pub chapters: Vec<Chapter>,
    pub flow: Vec<Flow>,
    /// Pasos fuera de lo que se pidió, con qué hacen.
    pub outside: Vec<(usize, String)>,
    pub unverified: Vec<String>,
    pub risks: Vec<String>,
    /// El cambio no es el mismo que cuando se escribió.
    pub stale: bool,
    /// Cuándo lo declaró el agente.
    pub recorded_at: Option<String>,
}

/// Un repositorio de la revisión: su raíz y su identidad (directorio git común).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub root: String,
    pub identity: Option<String>,
}

impl RepoRef {
    #[must_use]
    pub fn new(root: &str) -> Self {
        Self { root: root.to_owned(), identity: identity(Path::new(root)) }
    }
}

fn identity(dir: &Path) -> Option<String> {
    crate::task::repo_identity(dir).map(|identity| identity.display().to_string())
}

/// Repositorio de un fichero (por prefijo de ruta) y su ruta dentro de él.
fn place<'a>(repos: &'a [RepoRef], path: &'a str) -> (Option<&'a RepoRef>, &'a str) {
    repos
        .iter()
        .find_map(|repo| Some((Some(repo), path.strip_prefix(repo.root.as_str())?.strip_prefix('/')?)))
        .unwrap_or((None, path))
}

fn repo_name(repo: &RepoRef) -> &str {
    repo.root.rsplit('/').next().unwrap_or(&repo.root)
}

fn short(name: &str) -> &str {
    name.rsplit(['.', ':']).next().unwrap_or(name)
}

fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Los pasos a los que se refiere un ancla.
fn steps_of(anchor: &Anchor, review: &SessionReview, plan: &[FlowStep], repos: &[RepoRef]) -> Vec<usize> {
    if let Some(step) = anchor.step {
        return usize::try_from(step)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&n| n < plan.len())
            .into_iter()
            .collect();
    }
    let Some(file) = anchor.file.as_deref().map(|file| file.trim_start_matches("./")) else { return Vec::new() };
    let in_file: Vec<usize> = (0..plan.len())
        .filter(|&index| {
            let (repo, inside) = place(repos, &review.files[plan[index].file].path);
            let same_repo = match (anchor.repo.as_deref(), repo.and_then(|repo| repo.identity.as_deref())) {
                (Some(wanted), Some(found)) => wanted == found,
                _ => true,
            };
            same_repo
                && (inside == file || inside.ends_with(&format!("/{file}")) || file.ends_with(&format!("/{inside}")))
        })
        .collect();
    match (&anchor.symbol, anchor.range()) {
        (Some(symbol), _) => in_file
            .into_iter()
            .filter(|&index| {
                plan[index]
                    .symbol
                    .as_ref()
                    .is_some_and(|step| step.qualified_name == *symbol || short(&step.qualified_name) == short(symbol))
            })
            .collect(),
        (None, Some((start, end))) => in_file
            .into_iter()
            .filter(|&index| {
                plan[index].symbol.as_ref().is_none_or(|symbol| symbol.start_line <= end && start <= symbol.end_line)
            })
            .collect(),
        (None, None) => in_file,
    }
}

/// El texto actual del código de un paso (su símbolo) o, sin símbolo, de esas líneas.
fn current_text(review: &SessionReview, step: &FlowStep, lines: Option<(usize, usize)>) -> String {
    let current: Vec<&str> = review.files[step.file].after.lines().collect();
    let (start, end) =
        step.symbol.as_ref().map(|symbol| (symbol.start_line, symbol.end_line)).or(lines).unwrap_or((1, current.len()));
    current.get(start.saturating_sub(1)..end.min(current.len())).map(|slice| slice.join("\n")).unwrap_or_default()
}

/// Los capítulos del guion con sus pasos, cada paso en uno solo. Un capítulo
/// cuyo código ya está en otro se suma a ese. Devuelve los capítulos, qué
/// capítulo muestra cada id del guion y si algo está desfasado.
fn place_chapters(
    story_chapters: &[StoryChapter],
    requirement_index: impl Fn(&str) -> Option<usize>,
    placed: &mut BTreeSet<usize>,
    review: &SessionReview,
    plan: &[FlowStep],
    repos: &[RepoRef],
) -> (Vec<Chapter>, Vec<(String, usize)>, bool) {
    let mut stale = false;
    // Id de cada capítulo del guion → capítulo de la presentación que lo muestra.
    let mut chapter_ids: Vec<(String, usize)> = Vec::new();
    let mut chapters: Vec<Chapter> = Vec::new();
    // Capítulo de la presentación que tiene cada paso.
    let mut holder: BTreeMap<usize, usize> = BTreeMap::new();
    for chapter in story_chapters {
        let mut steps = Vec::new();
        let mut already = None;
        for anchor in &chapter.anchors {
            let found = steps_of(anchor, review, plan, repos);
            // Lo declarado desfasa si el código del ancla ya no es el que se declaró.
            if let (Some(snapshot), Some(&first)) = (&anchor.snapshot, found.first()) {
                stale |= squash(&current_text(review, &plan[first], anchor.range())) != squash(snapshot);
            }
            for step in found {
                if placed.insert(step) {
                    steps.push(step);
                } else if already.is_none() {
                    already = holder.get(&step).copied();
                }
            }
        }
        let requirements: Vec<usize> = chapter.requirements.iter().filter_map(|id| requirement_index(id)).collect();
        if steps.is_empty() {
            // Su código es un paso que ya está en otro capítulo (otra parte de la
            // misma función): no se pierde lo que cuenta, se suma a ese capítulo.
            if let Some(index) = already {
                let target = &mut chapters[index];
                if !chapter.narration.trim().is_empty() {
                    if !target.narration.is_empty() {
                        target.narration.push(' ');
                    }
                    target.narration.push_str(chapter.narration.trim());
                }
                target.look_at.extend(chapter.look_at.iter().cloned());
                target.questions.extend(chapter.questions.iter().cloned());
                for requirement in requirements {
                    if !target.requirements.contains(&requirement) {
                        target.requirements.push(requirement);
                    }
                }
                chapter_ids.push((chapter.id.clone(), index));
            }
            continue;
        }
        let index = chapters.len();
        for &step in &steps {
            holder.insert(step, index);
        }
        chapter_ids.push((chapter.id.clone(), index));
        chapters.push(Chapter {
            title: chapter.title.clone(),
            kind: Some(chapter.kind.clone()).filter(|kind| !kind.is_empty()),
            intent: chapter.intent.clone(),
            narration: chapter.narration.clone(),
            requirements,
            steps,
            look_at: chapter.look_at.clone(),
            questions: chapter.questions.clone(),
            unnarrated: false,
        });
    }
    (chapters, chapter_ids, stale)
}

/// Convierte un guion (de cualquier fuente) en la presentación de esta revisión.
#[must_use]
pub fn resolve(
    story: Story,
    source: Source,
    review: &SessionReview,
    plan: &[FlowStep],
    repos: &[RepoRef],
) -> Storyline {
    let mut stale = false;
    let mut placed = BTreeSet::new();
    let requirements: Vec<Requirement> = story
        .requirements
        .iter()
        .map(|requirement| Requirement {
            id: requirement.id.clone(),
            text: requirement.text.clone(),
            status: match requirement.status.as_str() {
                "done" => RequirementStatus::Done,
                "partial" => RequirementStatus::Partial,
                _ => RequirementStatus::Missing,
            },
            steps: requirement
                .places
                .iter()
                .flat_map(|anchor| steps_of(anchor, review, plan, repos))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            note: Some(requirement.note.clone()).filter(|note| !note.trim().is_empty()),
        })
        .collect();
    let requirement_index = |id: &str| requirements.iter().position(|requirement| requirement.id == id);
    let (mut chapters, chapter_ids, chapters_stale) =
        place_chapters(&story.chapters, requirement_index, &mut placed, review, plan, repos);
    stale |= chapters_stale;
    // Lo que el guion no colocó no se pierde: va al final.
    let rest: Vec<usize> = (0..plan.len()).filter(|step| !placed.contains(step)).collect();
    if !rest.is_empty() {
        chapters.push(if source == Source::Declared {
            Chapter::plain(
                "No lo contó el agente".into(),
                "Código del cambio que el guion del agente no menciona: revísalo con más cuidado.".into(),
                rest,
                true,
            )
        } else {
            Chapter::plain(
                "Otros cambios".into(),
                "Pasos que el guion no colocó en ningún capítulo.".into(),
                rest,
                false,
            )
        });
    }
    let chapter_index = |id: &str| chapter_ids.iter().find(|(chapter, _)| chapter == id).map(|(_, index)| *index);
    let flow = story
        .flow
        .iter()
        .filter_map(|edge| {
            Some(Flow { from: chapter_index(&edge.from)?, to: chapter_index(&edge.to)?, label: edge.label.clone() })
        })
        // Dos capítulos que acabaron juntos no son una flecha.
        .filter(|edge| edge.from != edge.to)
        .collect();
    let outside = story
        .outside
        .iter()
        .flat_map(|outside| {
            steps_of(&outside.anchor, review, plan, repos).into_iter().map(|step| (step, outside.why.clone()))
        })
        .collect();
    Storyline {
        source,
        model: None,
        summary: story.summary,
        requirements,
        chapters,
        flow,
        outside,
        unverified: story.unverified,
        risks: story.risks,
        stale,
        recorded_at: None,
    }
}

// ---------------------------------------------------------------------------
// 1. El guion que declara el agente.

/// Lo que se guardó de un guion declarado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub id: i64,
    pub chapters: usize,
    pub anchors: usize,
    /// Anclas que no se pudieron resolver (fichero inexistente), para que el agente las corrija.
    pub unresolved: Vec<String>,
}

/// Guarda el guion que declara el agente: resuelve cada ancla en disco (con su
/// símbolo y su texto) y lo asocia a cada repositorio y rama que toca.
///
/// # Errors
/// Si el guion no tiene capítulos, ninguna ancla se puede resolver o falla la base de datos.
pub fn record(store: &mut Store, mut told: Story) -> Result<Recorded> {
    if told.chapters.is_empty() {
        return Err(EngineError::Invalid("el guion no tiene capítulos".into()));
    }
    told.version = Some(VERSION.to_owned());
    let mut repos: BTreeSet<(String, String)> = BTreeSet::new();
    let mut unresolved = Vec::new();
    let mut resolved = 0;
    for anchor in told.anchors_mut() {
        let Some(file) = anchor.file.clone() else { continue };
        let Some((path, root)) = std::fs::canonicalize(&file).ok().and_then(|path| {
            let root = path.parent().and_then(crate::task::repo_root)?;
            Some((path, std::fs::canonicalize(&root).unwrap_or(root)))
        }) else {
            unresolved.push(file);
            continue;
        };
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        let (symbol, (start, end)) = crate::declared::anchor(&path, &source, anchor.symbol.as_deref(), anchor.range());
        let text: Vec<&str> = source.lines().collect();
        anchor.snapshot = text.get(start.saturating_sub(1)..end.min(text.len())).map(|slice| slice.join("\n"));
        anchor.symbol = symbol.or_else(|| anchor.symbol.clone());
        anchor.lines = Some(vec![start, end]);
        anchor.file = Some(path.strip_prefix(&root).map_or(file, |inside| inside.display().to_string()));
        anchor.repo = identity(&root);
        if let (Some(repo), Some(branch)) = (anchor.repo.clone(), crate::task::checked_out(&root)) {
            repos.insert((repo, branch));
        }
        resolved += 1;
    }
    if resolved == 0 {
        return Err(EngineError::Invalid(format!(
            "ninguna ancla apunta a un fichero existente ({}): usa rutas absolutas del código que escribiste",
            unresolved.join(", ")
        )));
    }
    let text = serde_json::to_string(&told).map_err(|error| EngineError::Invalid(error.to_string()))?;
    let id = store.add_declared_storyline(&text, &repos.into_iter().collect::<Vec<_>>())?;
    Ok(Recorded { id, chapters: told.chapters.len(), anchors: resolved, unresolved })
}

/// El guion declarado de una revisión: el más reciente de cada repositorio y
/// rama, juntos si son varios (una tarea con varias sesiones o micros).
///
/// # Errors
/// Si falla la base de datos.
pub fn declared(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    repos: &[(RepoRef, String)],
) -> Result<Option<Storyline>> {
    let mut found: BTreeMap<i64, (String, Story)> = BTreeMap::new();
    for (repo, branch) in repos {
        let Some(identity) = &repo.identity else { continue };
        if let Some(stored) = store.latest_declared_storyline(identity, branch)?
            && let Ok(story) = serde_json::from_str::<Story>(&stored.storyline)
        {
            found.insert(stored.id, (stored.recorded_at, story));
        }
    }
    let Some(recorded_at) = found.values().map(|(at, _)| at.clone()).max() else { return Ok(None) };
    let stories: Vec<Story> = found.into_values().map(|(_, story)| story).collect();
    let refs: Vec<RepoRef> = repos.iter().map(|(repo, _)| repo.clone()).collect();
    let mut storyline = resolve(merge(stories), Source::Declared, review, plan, &refs);
    storyline.recorded_at = Some(recorded_at);
    Ok(Some(storyline))
}

/// Varios guiones en uno, en el orden en que se declararon; los ids se
/// prefijan para que no choquen.
fn merge(mut stories: Vec<Story>) -> Story {
    if stories.len() == 1 {
        return stories.remove(0);
    }
    let mut merged = Story::default();
    for (index, story) in stories.into_iter().enumerate() {
        let prefix = |id: &str| format!("S{}:{id}", index + 1);
        if !story.summary.is_empty() {
            if !merged.summary.is_empty() {
                merged.summary.push(' ');
            }
            merged.summary.push_str(story.summary.trim());
        }
        merged.requirements.extend(
            story
                .requirements
                .into_iter()
                .map(|requirement| StoryRequirement { id: prefix(&requirement.id), ..requirement }),
        );
        merged.chapters.extend(story.chapters.into_iter().map(|chapter| StoryChapter {
            id: prefix(&chapter.id),
            requirements: chapter.requirements.iter().map(|id| prefix(id)).collect(),
            ..chapter
        }));
        merged.flow.extend(story.flow.into_iter().map(|edge| StoryFlow {
            from: prefix(&edge.from),
            to: prefix(&edge.to),
            label: edge.label,
        }));
        merged.outside.extend(story.outside);
        merged.unverified.extend(story.unverified);
        merged.risks.extend(story.risks);
    }
    merged
}

// ---------------------------------------------------------------------------
// 2. El guion del modelo revisor.

/// Lo que se sabe de la revisión además de sus pasos.
#[derive(Debug, Clone, Copy)]
pub struct StoryInput<'a> {
    pub title: Option<&'a str>,
    pub description: &'a str,
    pub repos: &'a [RepoRef],
    pub triage: &'a [StepTriage],
    pub declared: &'a BTreeMap<usize, Vec<DeclaredView>>,
}

/// El formato común con las anclas como número de paso y todo obligatorio: lo
/// que piden las salidas estructuradas estrictas.
fn output_schema() -> Value {
    let strings = json!({"type": "array", "items": {"type": "string"}});
    let anchor = json!({"type": "object", "properties": {"step": {"type": "integer"}}, "required": ["step"], "additionalProperties": false});
    let anchors = json!({"type": "array", "items": anchor});
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "requirements": {"type": "array", "items": {
                "type": "object",
                "properties": {"id": {"type": "string"}, "text": {"type": "string"},
                               "status": {"type": "string", "enum": ["done", "partial", "missing"]},
                               "where": anchors, "note": {"type": "string"}},
                "required": ["id", "text", "status", "where", "note"], "additionalProperties": false}},
            "chapters": {"type": "array", "items": {
                "type": "object",
                "properties": {"id": {"type": "string"}, "title": {"type": "string"},
                               "kind": {"type": "string", "enum": ["entry", "logic", "data", "effect", "integration", "test", "config", "other"]},
                               "intent": {"type": "string"}, "narration": {"type": "string"}, "anchors": anchors,
                               "requirements": strings, "look_at": strings, "questions": strings},
                "required": ["id", "title", "kind", "intent", "narration", "anchors", "requirements", "look_at", "questions"],
                "additionalProperties": false}},
            "flow": {"type": "array", "items": {
                "type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}, "label": {"type": "string"}},
                "required": ["from", "to", "label"], "additionalProperties": false}},
            "outside": {"type": "array", "items": {
                "type": "object", "properties": {"anchor": anchor, "why": {"type": "string"}},
                "required": ["anchor", "why"], "additionalProperties": false}},
            "unverified": strings,
            "risks": strings
        },
        "required": ["summary", "requirements", "chapters", "flow", "outside", "unverified", "risks"],
        "additionalProperties": false
    })
}

/// Lo que se envía al modelo revisor.
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
            let _ = write!(out, "{} · ", repo_name(repo));
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

/// El modelo revisor escribe el guion si no hay uno vigente. Devuelve `false` si ya lo había.
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
    let story: Story = serde_json::from_str(&text).map_err(|error| ExtractError::BadResponse(error.to_string()))?;
    if story.chapters.is_empty() {
        return Err(ExtractError::BadResponse("el modelo no escribió ningún capítulo".into()));
    }
    Ok(text)
}

/// El guion del revisor guardado, resuelto contra el plan actual, o `None` si no hay.
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
    let Ok(parsed) = serde_json::from_str::<Story>(&stored.storyline) else { return Ok(None) };
    let current = input_hash(&stored.model, &request(review, plan, input));
    let mut storyline = resolve(parsed, Source::Reviewer, review, plan, input.repos);
    storyline.stale = current != stored.input_hash;
    storyline.model = Some(stored.model);
    Ok(Some(storyline))
}

// ---------------------------------------------------------------------------
// 3. Sin modelo.

/// Guion automático: un capítulo por repositorio, en el orden del plan.
#[must_use]
pub fn fallback(review: &SessionReview, plan: &[FlowStep], repos: &[RepoRef]) -> Storyline {
    let mut chapters: Vec<Chapter> = Vec::new();
    for (index, step) in plan.iter().enumerate() {
        let (repo, _) = place(repos, &review.files[step.file].path);
        let title = repo.map_or("Cambios", repo_name).to_owned();
        match chapters.iter_mut().find(|chapter| chapter.title == title) {
            Some(chapter) => chapter.steps.push(index),
            None => chapters.push(Chapter::plain(title, String::new(), vec![index], false)),
        }
    }
    Storyline {
        source: Source::Automatic,
        model: None,
        summary: String::new(),
        requirements: Vec::new(),
        chapters,
        flow: Vec::new(),
        outside: Vec::new(),
        unverified: Vec::new(),
        risks: Vec::new(),
        stale: false,
        recorded_at: None,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::process::Command;

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

    fn position(plan: &[FlowStep], name: &str) -> usize {
        plan.iter().position(|s| s.symbol.as_ref().is_some_and(|s| s.qualified_name == name)).unwrap()
    }

    /// Respuesta del revisor: `total` y `other` son los números de esos pasos.
    fn answer(total: usize, other: usize) -> String {
        format!(
            r#"{{"summary":"Suma precios.","requirements":[{{"id":"R1","text":"Sumar los precios","status":"done","where":[{{"step":{total}}}],"note":""}}],
            "chapters":[{{"id":"C1","title":"El cálculo","kind":"logic","intent":"Suma","narration":"Primero…","anchors":[{{"step":{total}}},{{"step":{total}}},{{"step":99}}],"requirements":["R1","R7"],"look_at":["precios negativos"],"questions":["¿Qué pasa con una lista vacía?"]}}],
            "flow":[],"outside":[{{"anchor":{{"step":{other}}},"why":"cambia otro módulo"}}],"unverified":[],"risks":["precios negativos"]}}"#
        )
    }

    const TOTAL: &str = "export function total(items) {\n  return items.reduce((s, i) => s + i.price, 0);\n}\n";
    const OTHER: &str = "export function other() {\n  return 1;\n}\n";

    fn plain_repos(roots: &[&str]) -> Vec<RepoRef> {
        roots.iter().map(|root| RepoRef { root: (*root).to_owned(), identity: None }).collect()
    }

    #[test]
    fn the_reviewer_storyline_is_cached_and_every_step_ends_up_in_a_chapter() {
        let store = Store::open_in_memory().unwrap();
        let (review, plan) = review_of(&[("/w/billing/total.ts", TOTAL), ("/w/api/other.ts", OTHER)]);
        let repos = plain_repos(&["/w/billing", "/w/api"]);
        let declared = BTreeMap::new();
        let input = StoryInput {
            title: None,
            description: "Sumar los precios",
            repos: &repos,
            triage: &[],
            declared: &declared,
        };
        assert!(request(&review, &plan, input).contains("billing · total.ts · total"));
        let (total, other) = (position(&plan, "total"), position(&plan, "other"));
        let model = Fake { calls: RefCell::new(0), answer: answer(total + 1, other + 1) };
        assert!(generate(&store, &review, &plan, input, &model).unwrap());
        assert!(!generate(&store, &review, &plan, input, &model).unwrap());
        assert_eq!(*model.calls.borrow(), 1);

        let found = storyline(&store, &review, &plan, input).unwrap().unwrap();
        assert_eq!((found.source, found.stale), (Source::Reviewer, false));
        assert_eq!(found.chapters[0].steps, vec![total], "sin repetidos ni números fuera del plan");
        assert_eq!(found.chapters[0].requirements, vec![0]);
        assert_eq!(found.chapters[0].questions, vec!["¿Qué pasa con una lista vacía?".to_owned()]);
        assert_eq!(found.chapters.last().unwrap().title, "Otros cambios", "lo no colocado no se pierde");
        assert_eq!(found.chapters.iter().map(|c| c.steps.len()).sum::<usize>(), plan.len());
        assert_eq!(found.requirements[0].steps, vec![total]);
        assert_eq!(found.outside, vec![(other, "cambia otro módulo".to_owned())]);

        let changed = StoryInput { description: "Sumar los precios con IVA", ..input };
        assert!(storyline(&store, &review, &plan, changed).unwrap().unwrap().stale);
    }

    #[test]
    fn without_a_model_there_is_one_chapter_per_repository() {
        let (review, plan) =
            review_of(&[("/w/billing/a.ts", "export const a = 1;\n"), ("/w/api/b.ts", "export const b = 2;\n")]);
        let found = fallback(&review, &plan, &plain_repos(&["/w/billing", "/w/api"]));
        let titles: BTreeSet<&str> = found.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, BTreeSet::from(["api", "billing"]));
        assert_eq!(found.source, Source::Automatic);
    }

    fn git_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-story-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let status =
            Command::new("git").arg("-C").arg(&dir).args(["init", "-q", "-b", "feature/A-1"]).status().unwrap();
        assert!(status.success());
        std::fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn the_agent_storyline_is_anchored_resolved_and_flags_what_it_did_not_tell() {
        let repo = git_repo("declared");
        std::fs::write(repo.join("total.ts"), TOTAL).unwrap();
        std::fs::write(repo.join("other.ts"), OTHER).unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let told: Story = serde_json::from_value(json!({
            "summary": "Suma los precios del carrito.",
            "requirements": [{"id": "R1", "text": "Sumar los precios", "status": "done",
                              "where": [{"file": repo.join("total.ts"), "symbol": "total"}]}],
            "chapters": [{"id": "C1", "title": "El total", "kind": "logic", "intent": "Suma los precios",
                          "anchors": [{"file": repo.join("total.ts"), "symbol": "total"}], "requirements": ["R1"],
                          "look_at": ["precios negativos"], "questions": ["¿Y con descuentos?"]},
                         {"id": "C2", "title": "La suma", "intent": "El reduce", "narration": "Después suma.",
                          "anchors": [{"file": repo.join("total.ts"), "lines": [2, 2]}],
                          "questions": ["¿Qué devuelve con una lista vacía?"]}],
            "flow": [{"from": "C1", "to": "C2", "label": "suma"}], "risks": ["sin redondeo"]
        }))
        .unwrap();
        let recorded = record(&mut store, told).unwrap();
        assert_eq!((recorded.chapters, recorded.anchors, recorded.unresolved.len()), (2, 3, 0));

        let paths = [repo.join("total.ts").display().to_string(), repo.join("other.ts").display().to_string()];
        let (review, plan) = review_of(&[(&paths[0], TOTAL), (&paths[1], OTHER)]);
        let repos = vec![(RepoRef::new(&repo.display().to_string()), "feature/A-1".to_owned())];
        let found = declared(&store, &review, &plan, &repos).unwrap().unwrap();
        assert_eq!((found.source, found.stale), (Source::Declared, false));
        assert_eq!(found.chapters[0].steps, vec![position(&plan, "total")]);
        assert_eq!(found.requirements[0].steps, vec![position(&plan, "total")]);
        // «La suma» es otra parte de la misma función: se suma al capítulo que la tiene, sin perderse.
        assert_eq!(found.chapters.len(), 2, "{:?}", found.chapters);
        assert_eq!(found.chapters[0].questions.len(), 2);
        assert!(found.chapters[0].narration.contains("Después suma."));
        assert!(found.flow.is_empty(), "una flecha de un capítulo a sí mismo no se dibuja");
        let untold = found.chapters.last().unwrap();
        assert!(untold.unnarrated && untold.title == "No lo contó el agente", "{untold:?}");
        assert_eq!(untold.steps, vec![position(&plan, "other")]);

        // Si el código cambia después de contarlo, el guion queda desfasado.
        let changed = TOTAL.replace("i.price", "i.price * 2");
        let (review, plan) = review_of(&[(&paths[0], &changed), (&paths[1], OTHER)]);
        assert!(declared(&store, &review, &plan, &repos).unwrap().unwrap().stale);

        let missing: Story = serde_json::from_value(json!({"summary": "x", "chapters": [{"id": "C1", "title": "t",
            "intent": "i", "anchors": [{"file": "/no/existe.ts"}]}]}))
        .unwrap();
        assert!(record(&mut store, missing).unwrap_err().to_string().contains("ninguna ancla"));
        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn the_published_schema_is_valid_json_with_the_common_shape() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(schema["title"], VERSION);
        assert!(schema["properties"]["chapters"]["items"]["properties"]["questions"].is_object());
    }
}
