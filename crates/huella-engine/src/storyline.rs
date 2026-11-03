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
use crate::declared::plain_symbol;
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

Quien lo lee no conoce este código ni la tarea: no des nada por sabido y di qué es cada pieza la primera vez que \
la nombres.

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
/// Menos que esto no explica nada a quien no conoce el código.
const MIN_EXPLANATION: usize = 80;

// ---------------------------------------------------------------------------
// El formato común: huella.storyline/v1.

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Story {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub summary: String,
    /// Cómo se comportaba el sistema antes del cambio.
    pub before: String,
    /// Cómo se comporta ahora.
    pub after: String,
    /// Un caso concreto, de principio a fin.
    pub example: StoryExample,
    /// Términos del dominio y del código que hay que conocer.
    pub concepts: Vec<StoryConcept>,
    pub requirements: Vec<StoryRequirement>,
    pub chapters: Vec<StoryChapter>,
    pub flow: Vec<StoryFlow>,
    pub outside: Vec<StoryOutside>,
    pub unverified: Vec<String>,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryExample {
    pub scenario: String,
    pub walk: Vec<StoryWalk>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryWalk {
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryConcept {
    pub term: String,
    pub meaning: String,
}

/// Una idea del capítulo: lo que hay que entender y el código que la forma.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoryIdea {
    pub title: String,
    pub explanation: String,
    pub anchors: Vec<Anchor>,
    pub look_at: Vec<String>,
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
    pub ideas: Vec<StoryIdea>,
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
            .chain(self.chapters.iter_mut().flat_map(|chapter| {
                chapter.anchors.iter_mut().chain(chapter.ideas.iter_mut().flat_map(|idea| idea.anchors.iter_mut()))
            }))
            .chain(self.outside.iter_mut().map(|outside| &mut outside.anchor))
            .chain(self.example.walk.iter_mut().filter_map(|walk| walk.anchor.as_mut()))
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
    /// Las ideas del capítulo, en orden de lectura; los pasos que no estén en
    /// ninguna se muestran sueltos.
    pub ideas: Vec<Idea>,
    /// Código del cambio que el guion del agente no menciona.
    pub unnarrated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Idea {
    pub title: String,
    pub explanation: String,
    pub look_at: Vec<String>,
    /// Índices en el plan.
    pub steps: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Example {
    pub scenario: String,
    pub walk: Vec<Walk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Walk {
    pub what: String,
    /// Paso del plan donde ocurre, si se encontró.
    pub step: Option<usize>,
}

impl Chapter {
    pub(crate) fn plain(title: String, intent: String, steps: Vec<usize>, unnarrated: bool) -> Self {
        Self {
            title,
            kind: None,
            intent,
            narration: String::new(),
            requirements: Vec::new(),
            steps,
            look_at: Vec::new(),
            questions: Vec::new(),
            ideas: Vec::new(),
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
    pub before: String,
    pub after: String,
    pub example: Option<Example>,
    pub concepts: Vec<StoryConcept>,
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

/// ¿El símbolo del ancla es justo el del paso?
fn same_symbol(wanted: &str, step: &str) -> bool {
    let (wanted, step) = (plain_symbol(wanted), plain_symbol(step));
    step == wanted || short(&step) == short(&wanted)
}

/// ¿El símbolo del ancla contiene el del paso? Una clase contiene sus métodos.
fn contains_symbol(wanted: &str, step: &str) -> bool {
    let (wanted, step) = (plain_symbol(wanted), plain_symbol(step));
    if step.starts_with(&format!("{wanted}.")) {
        return true;
    }
    // `com.acme.Clase` es `Clase`, pero `Clase.Clase` es su constructor, no la clase.
    let mut last = wanted.rsplit('.');
    let is_constructor = last.next().is_some_and(|name| last.next() == Some(name));
    !is_constructor && step.starts_with(&format!("{}.", short(&wanted)))
}

fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// ¿El ancla menciona el fichero de este paso?
fn mentions_file(anchor: &Anchor, review: &SessionReview, step: &FlowStep, repos: &[RepoRef]) -> bool {
    let Some(file) = anchor.file.as_deref().map(|file| file.trim_start_matches("./")) else { return false };
    let (repo, inside) = place(repos, &review.files[step.file].path);
    let same_repo = match (anchor.repo.as_deref(), repo.and_then(|repo| repo.identity.as_deref())) {
        (Some(wanted), Some(found)) => wanted == found,
        _ => true,
    };
    same_repo && (inside == file || inside.ends_with(&format!("/{file}")) || file.ends_with(&format!("/{inside}")))
}

/// ¿Alguna línea que cambia este paso nombra esta palabra?
fn changes_word(step: &FlowStep, word: &str) -> bool {
    step.lines.iter().any(|line| match line {
        DiffLine::Added { text, .. } | DiffLine::Removed(text) => {
            text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '$').any(|part| part == word)
        }
        DiffLine::Context(_) => false,
    })
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
    let in_file: Vec<usize> =
        (0..plan.len()).filter(|&index| mentions_file(anchor, review, &plan[index], repos)).collect();
    match (&anchor.symbol, anchor.range()) {
        (Some(symbol), _) => {
            let named: Vec<usize> = in_file
                .iter()
                .copied()
                .filter(|&index| {
                    plan[index].symbol.as_ref().is_some_and(|step| {
                        same_symbol(symbol, &step.qualified_name) || contains_symbol(symbol, &step.qualified_name)
                    })
                })
                .collect();
            if !named.is_empty() {
                return named;
            }
            // Un campo o una constante no son un símbolo: están donde cambia esa palabra.
            let name = plain_symbol(symbol);
            let word = short(&name);
            in_file.into_iter().filter(|&index| changes_word(&plan[index], word)).collect()
        }
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
fn current_text(review: &SessionReview, step: &FlowStep, anchor: &Anchor) -> String {
    let lines = anchor.range();
    // Si el ancla es una clase y el paso un método suyo, lo declarado son sus líneas.
    let symbol = step.symbol.as_ref().filter(|symbol| {
        // Sin símbolo, lo declarado son sus líneas (quizá el fichero entero). Y el
        // constructor de Java se llama como su clase, pero no es ella.
        anchor.symbol.as_deref().is_some_and(|wanted| {
            same_symbol(wanted, &symbol.qualified_name) && !contains_symbol(wanted, &symbol.qualified_name)
        })
    });
    let current: Vec<&str> = review.files[step.file].after.lines().collect();
    let (start, end) =
        symbol.map(|symbol| (symbol.start_line, symbol.end_line)).or(lines).unwrap_or((1, current.len()));
    current.get(start.saturating_sub(1)..end.min(current.len())).map(|slice| slice.join("\n")).unwrap_or_default()
}

/// Los capítulos del guion con sus pasos, cada paso en uno solo. Un capítulo
/// cuyo código ya está en otro se suma a ese. Lo que el guion no nombra de un
/// fichero que sí menciona va con el capítulo que lo menciona: imports, campos o
/// un símbolo mal escrito no son código sin contar. Devuelve los capítulos, qué
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
    // Las anclas de cada capítulo de la presentación, para lo que no se nombró.
    let mut mentioned: Vec<(usize, &Anchor)> = Vec::new();
    // Las ideas de cada capítulo de la presentación, que se resuelven al final.
    let mut told_ideas: Vec<(usize, &StoryIdea)> = Vec::new();
    for chapter in story_chapters {
        let all_anchors = || chapter.anchors.iter().chain(chapter.ideas.iter().flat_map(|idea| idea.anchors.iter()));
        let mut steps = Vec::new();
        let mut already = None;
        for anchor in all_anchors() {
            let found = steps_of(anchor, review, plan, repos);
            // Lo declarado desfasa si el código del ancla ya no es el que se declaró.
            if let (Some(snapshot), Some(&first)) = (&anchor.snapshot, found.first()) {
                stale |= squash(&current_text(review, &plan[first], anchor)) != squash(snapshot);
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
        if let (true, Some(index)) = (steps.is_empty(), already) {
            // Su código es un paso que ya está en otro capítulo (otra parte de la
            // misma función): no se pierde lo que cuenta, se suma a ese capítulo.
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
            mentioned.extend(all_anchors().map(|anchor| (index, anchor)));
            told_ideas.extend(chapter.ideas.iter().map(|idea| (index, idea)));
            continue;
        }
        let index = chapters.len();
        mentioned.extend(all_anchors().map(|anchor| (index, anchor)));
        told_ideas.extend(chapter.ideas.iter().map(|idea| (index, idea)));
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
            ideas: Vec::new(),
            unnarrated: false,
        });
    }
    for (step, flow) in plan.iter().enumerate() {
        if placed.contains(&step) {
            continue;
        }
        // Primero un capítulo que aún no tenga código: si no, se perdería su narración.
        let target = mentioned
            .iter()
            .filter(|(_, anchor)| mentions_file(anchor, review, flow, repos))
            .map(|(index, _)| *index)
            .min_by_key(|&index| (!chapters[index].steps.is_empty(), index));
        if let Some(index) = target {
            chapters[index].steps.push(step);
            placed.insert(step);
        }
    }
    place_ideas(&mut chapters, &told_ideas, review, plan, repos);
    // Un capítulo que se queda sin código no se muestra.
    let mut shown = 0;
    let renumbered: Vec<Option<usize>> = chapters
        .iter()
        .map(|chapter| {
            (!chapter.steps.is_empty()).then(|| {
                shown += 1;
                shown - 1
            })
        })
        .collect();
    chapters.retain(|chapter| !chapter.steps.is_empty());
    let chapter_ids = chapter_ids.into_iter().filter_map(|(id, index)| Some((id, renumbered[index]?))).collect();
    (chapters, chapter_ids, stale)
}

/// Reparte los pasos de cada capítulo entre sus ideas, sin repetirlos. Lo que no se
/// nombra de un fichero que una idea menciona va con ella. Una idea sin código no
/// pierde lo que cuenta: pasa a la narración del capítulo.
fn place_ideas(
    chapters: &mut [Chapter],
    told: &[(usize, &StoryIdea)],
    review: &SessionReview,
    plan: &[FlowStep],
    repos: &[RepoRef],
) {
    for (index, chapter) in chapters.iter_mut().enumerate() {
        let ideas: Vec<&StoryIdea> = told.iter().filter(|(at, _)| *at == index).map(|(_, idea)| *idea).collect();
        if ideas.is_empty() {
            continue;
        }
        let mut taken = BTreeSet::new();
        let mut steps: Vec<Vec<usize>> = ideas
            .iter()
            .map(|idea| {
                idea.anchors
                    .iter()
                    .flat_map(|anchor| steps_of(anchor, review, plan, repos))
                    .filter(|step| chapter.steps.contains(step) && taken.insert(*step))
                    .collect()
            })
            .collect();
        for &step in &chapter.steps {
            if taken.contains(&step) {
                continue;
            }
            let holder = ideas
                .iter()
                .position(|idea| idea.anchors.iter().any(|anchor| mentions_file(anchor, review, &plan[step], repos)));
            if let Some(holder) = holder {
                steps[holder].push(step);
                taken.insert(step);
            }
        }
        // Se leen en el orden de las ideas, y al final lo que no está en ninguna.
        let rest: Vec<usize> = chapter.steps.iter().copied().filter(|step| !taken.contains(step)).collect();
        chapter.steps = steps.iter().flatten().copied().chain(rest).collect();
        for (idea, steps) in ideas.into_iter().zip(steps) {
            if steps.is_empty() {
                if !idea.explanation.trim().is_empty() {
                    if !chapter.narration.is_empty() {
                        chapter.narration.push(' ');
                    }
                    chapter.narration.push_str(idea.explanation.trim());
                }
                continue;
            }
            chapter.ideas.push(Idea {
                title: idea.title.clone(),
                explanation: idea.explanation.clone(),
                look_at: idea.look_at.clone(),
                steps,
            });
        }
    }
}

/// Lo que le falta a un guion para que lo entienda alguien sin contexto. Se le
/// devuelve al agente para que lo complete mientras aún lo sabe.
#[must_use]
pub fn gaps(story: &Story) -> Vec<String> {
    let mut found = Vec::new();
    if story.before.trim().is_empty() || story.after.trim().is_empty() {
        found.push("before y after: cómo se comportaba el sistema antes y cómo ahora".to_owned());
    }
    if story.example.scenario.trim().is_empty() || story.example.walk.is_empty() {
        found.push("example: un caso concreto con datos de verdad, de principio a fin".to_owned());
    }
    if story.concepts.is_empty() {
        found.push("concepts: los términos del dominio y del código que hay que conocer".to_owned());
    }
    for chapter in &story.chapters {
        if chapter.narration.trim().is_empty() {
            found.push(format!("narration de «{}»: cómo encaja con lo anterior", chapter.title));
        }
        if chapter.ideas.is_empty() && !matches!(chapter.kind.as_str(), "config" | "test") {
            found.push(format!("ideas de «{}»: explica su código por ideas", chapter.title));
        }
        for idea in &chapter.ideas {
            if idea.explanation.trim().len() < MIN_EXPLANATION {
                found.push(format!(
                    "explanation de «{}»: es demasiado breve para quien no conoce el código",
                    idea.title
                ));
            }
            if idea.anchors.is_empty() {
                found.push(format!("anchors de «{}»: di qué código la forma", idea.title));
            }
        }
    }
    found
}

fn resolve_example(
    example: &StoryExample,
    review: &SessionReview,
    plan: &[FlowStep],
    repos: &[RepoRef],
) -> Option<Example> {
    (!example.scenario.trim().is_empty() || !example.walk.is_empty()).then(|| Example {
        scenario: example.scenario.clone(),
        walk: example
            .walk
            .iter()
            .map(|walk| Walk {
                what: walk.what.clone(),
                step: walk.anchor.as_ref().and_then(|anchor| steps_of(anchor, review, plan, repos).first().copied()),
            })
            .collect(),
    })
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
    // Lo que el guion no colocó no se pierde: va al final, y lo generado aparte.
    let (generated, rest): (Vec<usize>, Vec<usize>) = (0..plan.len())
        .filter(|step| !placed.contains(step))
        .partition(|&step| crate::triage::clearly_generated(&review.files[plan[step].file].path));
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
    if !generated.is_empty() {
        chapters.push(Chapter::plain(
            "Ficheros generados".into(),
            "Los produce una herramienta (compilados, lockfiles…): no hace falta leerlos.".into(),
            generated,
            false,
        ));
    }
    let chapter_index = |id: &str| chapter_ids.iter().find(|(chapter, _)| chapter == id).map(|(_, index)| *index);
    let example = resolve_example(&story.example, review, plan, repos);
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
        before: story.before,
        after: story.after,
        example,
        concepts: story.concepts,
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
    /// Lo que le falta para que lo entienda alguien sin contexto.
    pub gaps: Vec<String>,
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
    let gaps = gaps(&told);
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
    Ok(Recorded { id, chapters: told.chapters.len(), anchors: resolved, unresolved, gaps })
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
        // De varias sesiones o micros: el primer antes y ahora, y todos los términos.
        if merged.before.is_empty() {
            merged.before = story.before;
        }
        if merged.after.is_empty() {
            merged.after = story.after;
        }
        if merged.example.scenario.is_empty() {
            merged.example = story.example;
        }
        for concept in story.concepts {
            if !merged.concepts.iter().any(|known| known.term.eq_ignore_ascii_case(&concept.term)) {
                merged.concepts.push(concept);
            }
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

/// Caracteres que se envían de cada línea de diff: las de un fichero
/// minificado o generado pueden tener cientos de miles.
pub(crate) const LINE_CHARS: usize = 200;
/// Contexto que se supone si no se configura: el de un modelo local habitual.
pub const DEFAULT_CONTEXT_TOKENS: usize = 32_768;
/// Tokens que se reservan para las instrucciones y para la respuesta (el guion entero).
const RESERVED_TOKENS: usize = 10_000;
/// Caracteres por token, a la baja: el código y el español cuentan más tokens que la prosa en inglés.
const CHARS_PER_TOKEN: usize = 3;

/// Caracteres que caben en una petición con un contexto de `context_tokens`
/// (o el de por defecto): lo que no cabe se lee por partes.
#[must_use]
pub fn request_chars(context_tokens: Option<usize>) -> usize {
    let context = context_tokens.unwrap_or(DEFAULT_CONTEXT_TOKENS);
    context.saturating_sub(RESERVED_TOKENS).max(4_000) * CHARS_PER_TOKEN
}

pub const MERGE_PROMPT: &str = "\
Juntas en un único guion (huella.storyline/v1) los guiones parciales de un cambio que, por su tamaño, se leyó en \
varias partes. Recibes lo que pide la tarea y, de cada parte, sus capítulos con los números de sus pasos, sus \
requisitos, lo que queda fuera, lo no comprobado y los riesgos.

Devuelve el guion completo, con { \"step\": número } donde se pide código:
- chapters en el orden en que conviene entender el cambio: de la entrada (endpoint, pantalla, comando, evento) a la \
lógica, los datos y los efectos, cruzando repositorios si uno llama a otro; tests y configuración al final. \
Fusiona los capítulos que son la misma intención sumando sus pasos; mantén los números de paso tal cual, cada paso \
en un solo capítulo. Conserva y mejora sus narraciones para que cada capítulo enlace con el anterior; conserva \
look_at y questions.
- requirements: une los repetidos; done si alguna parte lo cumple del todo, partial si a medias, missing si ninguna.
- summary del conjunto, flow entre los capítulos finales, y outside, unverified y risks sin repetir.

Escribe en el idioma de la descripción.";

/// Una línea para el modelo, recortada.
pub(crate) fn clip_line(text: &str) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(LINE_CHARS) {
        Some((cut, _)) => format!("{}… ({} caracteres más)", &text[..cut], text.chars().count() - LINE_CHARS).into(),
        None => text.into(),
    }
}

/// La tarea: su título y lo que pide.
fn header(input: StoryInput<'_>) -> String {
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
    out
}

/// El bloque de un paso: su cabecera y su diff recortado. Lo saltable
/// (generado, solo formato) va sin diff: no hay nada que entender ahí.
fn step_block(
    review: &SessionReview,
    plan: &[FlowStep],
    index: usize,
    input: StoryInput<'_>,
    per_step: usize,
) -> String {
    let step = &plan[index];
    let mut out = String::new();
    let (repo, path) = place(input.repos, &review.files[step.file].path);
    let name = step.symbol.as_ref().map_or("(fuera de funciones)", |symbol| symbol.qualified_name.as_str());
    let role = match step.role {
        StepRole::Flow => "código",
        StepRole::Test => "test",
        StepRole::Other => "otros cambios",
    };
    let level = input.triage.get(index).map(|triaged| triaged.level);
    let level_name = match level {
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
    let _ = write!(out, "{path} · {name} · {role} · +{} −{} · {level_name}", step.added(), step.removed());
    if !tags.is_empty() {
        let _ = write!(out, " ({})", tags.join(", "));
    }
    let calls: Vec<String> = step.calls.iter().map(|call| format!("[{}]", call + 1)).collect();
    if !calls.is_empty() {
        let _ = write!(out, " · llama a {}", calls.join(", "));
    }
    out.push('\n');
    for entry in input.declared.get(&index).into_iter().flatten().take(2) {
        let _ = writeln!(out, "  Declaró el agente: {}", clip_line(&entry.declaration.summary));
    }
    if level == Some(Level::Skip) {
        return out;
    }
    out.push_str("```diff\n");
    let changed: Vec<&DiffLine> = step.lines.iter().filter(|line| !matches!(line, DiffLine::Context(_))).collect();
    for line in changed.iter().take(per_step) {
        let _ = match line {
            DiffLine::Added { text, .. } => writeln!(out, "+{}", clip_line(text)),
            DiffLine::Removed(text) => writeln!(out, "-{}", clip_line(text)),
            DiffLine::Context(_) => Ok(()),
        };
    }
    if changed.len() > per_step {
        let _ = writeln!(out, "… ({} líneas más)", changed.len() - per_step);
    }
    out.push_str("```\n");
    out
}

fn blocks(review: &SessionReview, plan: &[FlowStep], input: StoryInput<'_>) -> Vec<String> {
    let per_step = (DIFF_BUDGET / plan.len().max(1)).clamp(STEP_LINES.1, STEP_LINES.0);
    (0..plan.len()).map(|index| step_block(review, plan, index, input, per_step)).collect()
}

const STEPS_INTRO: &str = "Pasos del cambio, en el orden del flujo dentro de cada repositorio:\n";

/// Todo lo que se le cuenta al modelo revisor del cambio: si cambia, el guion
/// está desfasado. Si no cabe en una petición, se envía por partes.
#[must_use]
pub fn request(review: &SessionReview, plan: &[FlowStep], input: StoryInput<'_>) -> String {
    let text = format!("{}{STEPS_INTRO}{}", header(input), blocks(review, plan, input).concat());
    huella_core::redact::redact(&text).into_owned()
}

fn input_hash(model: &str, request: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    hasher.update(SYSTEM_PROMPT.as_bytes());
    hasher.update(request.as_bytes());
    hasher.finalize().to_hex()[..16].to_owned()
}

/// El modelo revisor escribe el guion si no hay uno vigente. Devuelve `false`
/// si ya lo había. Si el cambio no cabe en el contexto del modelo
/// (`context_tokens`; sin él, [`DEFAULT_CONTEXT_TOKENS`]), lo lee por partes y
/// después las junta; `progress` recibe la parte (1, 2…) y cuántas hay.
///
/// # Errors
/// Si falla la base de datos o el modelo.
pub fn generate(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    input: StoryInput<'_>,
    model: &(impl JsonModel + ?Sized),
    context_tokens: Option<usize>,
    progress: impl FnMut(usize, usize),
) -> Result<bool> {
    generate_within(store, review, plan, input, model, request_chars(context_tokens), progress)
}

fn generate_within(
    store: &Store,
    review: &SessionReview,
    plan: &[FlowStep],
    input: StoryInput<'_>,
    model: &(impl JsonModel + ?Sized),
    budget: usize,
    mut progress: impl FnMut(usize, usize),
) -> Result<bool> {
    let model_id = model.model_id();
    let request = request(review, plan, input);
    let hash = input_hash(&model_id, &request);
    if store.storyline(&review.session_id)?.is_some_and(|stored| stored.input_hash == hash) {
        return Ok(false);
    }
    let ask = |system: &str, user: &str| {
        model
            .complete_json(system, user, &output_schema())
            .and_then(validate)
            .map_err(|error| EngineError::Model(error.to_string()))
    };
    let text = if request.len() <= budget {
        progress(1, 1);
        ask(SYSTEM_PROMPT, &request)?
    } else {
        let head = header(input);
        let segments = segments(&blocks(review, plan, input), budget.saturating_sub(head.len() + STEPS_INTRO.len()));
        let total = segments.len();
        let mut parts = Vec::with_capacity(total);
        for (number, segment) in segments.iter().enumerate() {
            progress(number + 1, total + 1);
            let user = format!(
                "{head}Por su tamaño, el cambio se lee en {total} partes; esta es la {}. Cuenta solo estos pasos \
                 (la numeración es la de todo el cambio); en requirements, solo lo que se vea en esta parte.\n{STEPS_INTRO}{}",
                number + 1,
                segment.concat()
            );
            let text = ask(SYSTEM_PROMPT, &huella_core::redact::redact(&user))?;
            parts.push(serde_json::from_str::<Story>(&text).map_err(|error| EngineError::Model(error.to_string()))?);
        }
        progress(total + 1, total + 1);
        ask(MERGE_PROMPT, &merge_request(&head, &parts))?
    };
    store
        .save_storyline(&review.session_id, &StoredStoryline { input_hash: hash, model: model_id, storyline: text })?;
    Ok(true)
}

/// Los bloques de los pasos en partes que no pasan de `budget` caracteres, en
/// orden. Un bloque que no cabe ni solo se recorta.
fn segments(blocks: &[String], budget: usize) -> Vec<Vec<String>> {
    let budget = budget.max(1000);
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut size = 0;
    for block in blocks {
        let block = match block.char_indices().nth(budget) {
            Some((cut, _)) => format!("{}…\n```\n", &block[..cut]),
            None => block.clone(),
        };
        if out.is_empty() || size + block.len() > budget {
            out.push(Vec::new());
            size = 0;
        }
        size += block.len();
        if let Some(last) = out.last_mut() {
            last.push(block);
        }
    }
    out
}

/// Lo que se le pasa al modelo para juntar las partes: sus guiones, sin diffs.
fn merge_request(head: &str, parts: &[Story]) -> String {
    let steps = |anchors: &[Anchor]| -> String {
        anchors.iter().filter_map(|anchor| anchor.step).map(|step| format!("[{step}]")).collect::<Vec<_>>().join(", ")
    };
    let mut out = format!("{head}El cambio se leyó en {} partes. Lo que salió de cada una:\n", parts.len());
    for (number, part) in parts.iter().enumerate() {
        let _ = writeln!(out, "\n## Parte {}\nResumen: {}", number + 1, part.summary);
        for chapter in &part.chapters {
            let _ = writeln!(
                out,
                "- Capítulo [{}] {} · pasos {} · {}\n  Narración: {}",
                chapter.kind,
                chapter.title,
                steps(&chapter.anchors),
                chapter.intent,
                chapter.narration
            );
            if !chapter.look_at.is_empty() {
                let _ = writeln!(out, "  Mirar: {}", chapter.look_at.join("; "));
            }
            if !chapter.questions.is_empty() {
                let _ = writeln!(out, "  Preguntas: {}", chapter.questions.join("; "));
            }
        }
        for requirement in &part.requirements {
            let _ = writeln!(
                out,
                "- Requisito ({}) {} · pasos {} · {}",
                requirement.status,
                requirement.text,
                steps(&requirement.places),
                requirement.note
            );
        }
        for outside in &part.outside {
            let _ = writeln!(
                out,
                "- Fuera de la tarea: {} · {}",
                steps(std::slice::from_ref(&outside.anchor)),
                outside.why
            );
        }
        if !part.unverified.is_empty() {
            let _ = writeln!(out, "- Sin comprobar: {}", part.unverified.join("; "));
        }
        if !part.risks.is_empty() {
            let _ = writeln!(out, "- Riesgos: {}", part.risks.join("; "));
        }
    }
    huella_core::redact::redact(&out).into_owned()
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
        before: String::new(),
        after: String::new(),
        example: None,
        concepts: Vec::new(),
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
        assert!(generate(&store, &review, &plan, input, &model, None, |_, _| {}).unwrap());
        assert!(!generate(&store, &review, &plan, input, &model, None, |_, _| {}).unwrap());
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

    /// Un revisor que responde a las partes con un capítulo por parte (sus pasos)
    /// y, al juntarlas, un único capítulo con todos los pasos que se le pasan.
    struct Parts {
        systems: RefCell<Vec<String>>,
        longest: RefCell<usize>,
    }

    impl JsonModel for Parts {
        fn model_id(&self) -> String {
            "fake:partes".into()
        }

        fn complete_json(&self, system: &str, user: &str, _: &Value) -> std::result::Result<String, ExtractError> {
            self.systems.borrow_mut().push(system.to_owned());
            let longest = user.lines().map(|line| line.chars().count()).max().unwrap_or(0);
            let mut current = self.longest.borrow_mut();
            *current = (*current).max(longest);
            let numbers: BTreeSet<u32> =
                user.split('[').skip(1).filter_map(|rest| rest.split(']').next()?.parse().ok()).collect();
            let anchors: Vec<Value> = numbers.iter().map(|n| json!({"step": n})).collect();
            Ok(json!({"summary": "parte", "requirements": [], "chapters": [{"id": "C1", "title": "Todo", "kind": "logic",
                "intent": "i", "narration": "n", "anchors": anchors, "requirements": [], "look_at": [], "questions": []}],
                "flow": [], "outside": [], "unverified": [], "risks": []})
            .to_string())
        }
    }

    #[test]
    fn big_changes_are_read_in_parts_and_long_lines_are_clipped() {
        let minified = format!("export const bundle = \"{}\";\n", "x".repeat(5000));
        let files: Vec<(String, String)> = (0..6)
            .map(|n| (format!("/w/api/f{n}.ts"), format!("export function f{n}() {{\n  return {n};\n}}\n{minified}")))
            .collect();
        let refs: Vec<(&str, &str)> = files.iter().map(|(path, content)| (path.as_str(), content.as_str())).collect();
        let (review, plan) = review_of(&refs);
        let repos = plain_repos(&["/w/api"]);
        let declared = BTreeMap::new();
        let input = StoryInput {
            title: None,
            description: "Varias funciones",
            repos: &repos,
            triage: &[],
            declared: &declared,
        };
        let text = request(&review, &plan, input);
        assert!(
            text.contains("caracteres más") && !text.contains(&"x".repeat(LINE_CHARS + 1)),
            "las líneas largas se recortan"
        );

        let store = Store::open_in_memory().unwrap();
        let model = Parts { systems: RefCell::new(Vec::new()), longest: RefCell::new(0) };
        let mut seen = Vec::new();
        assert!(
            generate_within(&store, &review, &plan, input, &model, 1500, |part, total| seen.push((part, total)))
                .unwrap()
        );
        let systems = model.systems.borrow();
        assert!(systems.len() > 2, "varias partes y la que las junta: {}", systems.len());
        assert_eq!(systems.last().unwrap(), MERGE_PROMPT);
        assert!(systems[..systems.len() - 1].iter().all(|system| system == SYSTEM_PROMPT));
        assert_eq!(seen.last(), Some(&(systems.len(), systems.len())));
        assert!(*model.longest.borrow() < 400, "ninguna línea enorme llega al modelo");

        let found = storyline(&store, &review, &plan, input).unwrap().unwrap();
        assert_eq!(found.chapters[0].steps.len(), plan.len(), "al juntar, todos los pasos siguen ahí");
        assert!(!found.stale);
    }

    #[test]
    fn the_request_size_follows_the_context_of_the_model() {
        assert_eq!(request_chars(None), (DEFAULT_CONTEXT_TOKENS - 10_000) * 3);
        assert_eq!(request_chars(Some(65_536)), (65_536 - 10_000) * 3);
        assert!(request_chars(Some(131_072)) > request_chars(Some(65_536)));
        assert_eq!(request_chars(Some(2_000)), 4_000 * 3, "un contexto mínimo no deja peticiones vacías");
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
    fn a_class_contains_its_methods_but_a_constructor_does_not() {
        assert!(contains_symbol("UserService", "UserService.register"));
        assert!(contains_symbol("com.acme.UserService", "UserService.register"));
        assert!(!contains_symbol("UserService.UserService", "UserService.register"));
        assert!(!contains_symbol("UserService.register", "UserService.count"));
        assert!(same_symbol("UserService#register(String)", "UserService.register"));
        assert!(!same_symbol("UserService.count", "UserService.register"));
    }

    const CART: &str = "import { a } from './a';\nexport const LIMIT = a;\nexport class Cart {\n  add(x) {\n    return x + 1;\n  }\n  remove(x) {\n    return x - 1;\n  }\n}\n";

    /// Un guion del agente sobre `cart.ts` y `other.ts` sin contar, con estos capítulos.
    fn told_about_cart(name: &str, chapters: &Value) -> (Storyline, Vec<FlowStep>, usize) {
        let repo = git_repo(name);
        std::fs::write(repo.join("cart.ts"), CART).unwrap();
        std::fs::write(repo.join("other.ts"), OTHER).unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let mut chapters = chapters.clone();
        for chapter in chapters.as_array_mut().unwrap() {
            for anchor in chapter["anchors"].as_array_mut().unwrap() {
                anchor["file"] = json!(repo.join("cart.ts"));
            }
            for idea in chapter.get_mut("ideas").and_then(Value::as_array_mut).into_iter().flatten() {
                for anchor in idea["anchors"].as_array_mut().unwrap() {
                    anchor["file"] = json!(repo.join("cart.ts"));
                }
            }
        }
        record(&mut store, serde_json::from_value(json!({"summary": "s", "chapters": chapters})).unwrap()).unwrap();
        let paths = [
            repo.join("cart.ts").display().to_string(),
            repo.join("other.ts").display().to_string(),
            repo.join("dist/app.js").display().to_string(),
        ];
        let (review, plan) = review_of(&[(&paths[0], CART), (&paths[1], OTHER), (&paths[2], "var a = 1;\n")]);
        let repos = vec![(RepoRef::new(&repo.display().to_string()), "feature/A-1".to_owned())];
        let found = declared(&store, &review, &plan, &repos).unwrap().unwrap();
        std::fs::remove_dir_all(&repo).unwrap();
        let in_cart = plan.iter().filter(|step| review.files[step.file].path.ends_with("cart.ts")).count();
        (found, plan, in_cart)
    }

    #[test]
    fn an_anchor_to_a_class_covers_its_methods_and_what_is_not_named_stays_in_that_chapter() {
        let chapter = json!([{"id": "C1", "title": "El carrito", "intent": "i", "anchors": [{"symbol": "Cart"}]}]);
        let (found, _, in_cart) = told_about_cart("class", &chapter);
        assert_eq!(found.chapters[0].title, "El carrito");
        assert_eq!(found.chapters[0].steps.len(), in_cart, "{found:?}");
        assert!(found.chapters[0].steps.len() > 1, "métodos e imports: {:?}", found.chapters[0]);
        assert!(!found.stale, "un ancla a la clase no desfasa por compararla con un método");
    }

    #[test]
    fn an_anchor_to_a_whole_file_is_not_stale_because_of_its_first_step() {
        let chapter = json!([{"id": "C1", "title": "El fichero", "intent": "i", "anchors": [{}]}]);
        let (found, _, in_cart) = told_about_cart("file", &chapter);
        assert_eq!(found.chapters[0].steps.len(), in_cart);
        assert!(!found.stale, "{found:?}");
    }

    #[test]
    fn only_files_the_story_does_not_mention_count_as_untold_and_generated_goes_apart() {
        let chapter = json!([{"id": "C1", "title": "El carrito", "intent": "i", "anchors": [{"symbol": "Cart.add"}]}]);
        let (found, plan, _) = told_about_cart("rest", &chapter);
        let titles: Vec<&str> = found.chapters.iter().map(|chapter| chapter.title.as_str()).collect();
        assert_eq!(titles, ["El carrito", "No lo contó el agente", "Ficheros generados"]);
        assert!(found.chapters[1].unnarrated && !found.chapters[2].unnarrated);
        assert_eq!(found.chapters[1].steps, vec![position(&plan, "other")]);
    }

    #[test]
    fn java_style_names_match_and_a_misspelled_symbol_keeps_its_chapter() {
        let chapters = json!([
            {"id": "C1", "title": "Quitar", "intent": "i", "anchors": [{"symbol": "Cart#remove(int)"}]},
            {"id": "C2", "title": "Con errata", "intent": "i", "narration": "Lo que cuenta", "anchors": [{"symbol": "Carro.nada"}]}
        ]);
        let (found, plan, _) = told_about_cart("java", &chapters);
        assert_eq!(found.chapters[0].steps[0], position(&plan, "Cart.remove"));
        assert_eq!(found.chapters[1].title, "Con errata", "{found:?}");
        assert_eq!(found.chapters[1].narration, "Lo que cuenta");
    }

    #[test]
    fn a_java_class_told_as_a_whole_leaves_nothing_untold() {
        const SERVICE: &str = "package com.acme;\n\nimport java.util.List;\n\npublic class UserService {\n  private final List<String> users = List.of();\n\n  public UserService() {}\n\n  public void register(String email) {\n    users.add(email);\n  }\n\n  public int count() {\n    return users.size();\n  }\n}\n";
        let repo = git_repo("java-class");
        let dir = repo.join("src/main/java/com/acme");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("UserService.java");
        std::fs::write(&file, SERVICE).unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let told: Story = serde_json::from_value(json!({"summary": "s", "chapters": [
            {"id": "C1", "title": "Usuarios", "intent": "i", "anchors": [{"file": file, "symbol": "UserService"}]}
        ]}))
        .unwrap();
        record(&mut store, told).unwrap();
        let path = file.display().to_string();
        let (review, plan) = review_of(&[(&path, SERVICE)]);
        let repos = vec![(RepoRef::new(&repo.display().to_string()), "feature/A-1".to_owned())];
        let found = declared(&store, &review, &plan, &repos).unwrap().unwrap();
        std::fs::remove_dir_all(&repo).unwrap();
        assert!(plan.len() > 1, "{plan:?}");
        assert_eq!(found.chapters.len(), 1, "{:?}", found.chapters);
        assert_eq!(found.chapters[0].steps.len(), plan.len());
        assert!(!found.stale);
    }

    #[test]
    fn ideas_group_the_steps_of_a_chapter_and_nothing_is_lost() {
        let chapters = json!([{"id": "C1", "title": "El carrito", "intent": "i", "narration": "Empieza.", "anchors": [{"symbol": "Cart.remove"}], "ideas": [
            {"title": "Quitar", "explanation": "Quita.", "look_at": ["negativos"], "anchors": [{"symbol": "Cart.remove"}]},
            {"title": "Con errata", "explanation": "Esto se cuenta igual.", "anchors": [{"symbol": "Carro.nada"}]},
            {"title": "Añadir", "explanation": "Añade.", "anchors": [{"symbol": "Cart.add"}]}
        ]}]);
        let (found, plan, in_cart) = told_about_cart("ideas", &chapters);
        let chapter = &found.chapters[0];
        assert_eq!(chapter.steps.len(), in_cart, "{chapter:?}");
        let titles: Vec<&str> = chapter.ideas.iter().map(|idea| idea.title.as_str()).collect();
        assert_eq!(titles, ["Quitar", "Añadir"], "{chapter:?}");
        assert!(chapter.narration.ends_with("Esto se cuenta igual."), "{chapter:?}");
        // Lo que nadie nombró de ese fichero (imports, la clase) va con la primera idea que lo menciona.
        assert_eq!(chapter.ideas[0].steps[0], position(&plan, "Cart.remove"));
        assert_eq!(chapter.ideas[1].steps, vec![position(&plan, "Cart.add")]);
        let all: Vec<usize> = chapter.ideas.iter().flat_map(|idea| idea.steps.clone()).collect();
        assert_eq!(all.len(), in_cart, "ningún paso suelto, ni repetido");
        assert_eq!(all.iter().collect::<BTreeSet<_>>().len(), all.len());
        assert_eq!(chapter.steps, all, "se lee en el orden de las ideas");
    }

    #[test]
    fn a_field_is_found_where_that_word_changes() {
        const SERVICE: &str = "public class Limits {\n  private static final int MAX_LENGTH = 254;\n\n  public boolean ok(String email) {\n    return email.length() < MAX_LENGTH;\n  }\n}\n";
        let repo = git_repo("field");
        let file = repo.join("Limits.java");
        std::fs::write(&file, SERVICE).unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let told: Story = serde_json::from_value(
            json!({"summary": "s", "chapters": [{"id": "C1", "title": "T", "intent": "i", "ideas": [
                {"title": "El método", "explanation": "e", "anchors": [{"file": file, "symbol": "Limits.ok"}]},
                {"title": "El límite", "explanation": "e", "anchors": [{"file": file, "symbol": "Limits.MAX_LENGTH"}]}
            ]}]}),
        )
        .unwrap();
        record(&mut store, told).unwrap();
        let path = file.display().to_string();
        let (review, plan) = review_of(&[(&path, SERVICE)]);
        let repos = vec![(RepoRef::new(&repo.display().to_string()), "feature/A-1".to_owned())];
        let found = declared(&store, &review, &plan, &repos).unwrap().unwrap();
        std::fs::remove_dir_all(&repo).unwrap();
        let titles: Vec<&str> = found.chapters[0].ideas.iter().map(|idea| idea.title.as_str()).collect();
        assert_eq!(titles, ["El método", "El límite"], "{:?}", found.chapters[0]);
        assert_eq!(found.chapters[0].ideas[0].steps, vec![position(&plan, "Limits.ok")]);
        assert_eq!(found.chapters[0].ideas[1].steps, vec![position(&plan, "Limits")]);
    }

    #[test]
    fn an_idea_without_code_keeps_what_it_tells_in_the_chapter() {
        let chapters = json!([{"id": "C1", "title": "T", "intent": "i", "narration": "Empieza.", "anchors": [{"symbol": "Cart.add"}], "ideas": [
            {"title": "Con código", "explanation": "Añade.", "anchors": [{"symbol": "Cart"}]},
            {"title": "Sin código", "explanation": "Esto se cuenta igual.", "anchors": []}
        ]}]);
        let (found, _, _) = told_about_cart("idea-empty", &chapters);
        assert_eq!(found.chapters[0].ideas.len(), 1);
        assert!(found.chapters[0].narration.ends_with("Esto se cuenta igual."), "{:?}", found.chapters[0]);
    }

    #[test]
    fn a_story_says_what_is_missing_for_someone_without_context() {
        let thin: Story = serde_json::from_value(json!({"summary": "s", "chapters": [
            {"id": "C1", "title": "Lógica", "intent": "i", "anchors": [{"file": "a.ts"}]},
            {"id": "C2", "title": "Ajustes", "kind": "config", "intent": "i", "narration": "n", "anchors": [{"file": "b.toml"}]}
        ]}))
        .unwrap();
        let found = gaps(&thin);
        for expected in ["before y after", "example", "concepts", "narration de «Lógica»", "ideas de «Lógica»"] {
            assert!(found.iter().any(|gap| gap.starts_with(expected)), "falta «{expected}»: {found:?}");
        }
        assert!(!found.iter().any(|gap| gap.contains("Ajustes")), "la configuración no necesita ideas: {found:?}");

        let full: Story = serde_json::from_value(json!({"summary": "s", "before": "b", "after": "a",
            "example": {"scenario": "x", "walk": [{"what": "y"}]}, "concepts": [{"term": "t", "meaning": "m"}],
            "chapters": [{"id": "C1", "title": "T", "intent": "i", "narration": "n", "ideas": [
                {"title": "I", "explanation": "Una explicación con suficiente detalle para alguien que no conoce nada de este código.", "anchors": [{"file": "a.ts"}]}
            ]}]}))
        .unwrap();
        assert_eq!(gaps(&full), Vec::<String>::new());
    }

    #[test]
    fn the_example_and_the_concepts_reach_the_presentation() {
        let repo = git_repo("example");
        std::fs::write(repo.join("cart.ts"), CART).unwrap();
        let file = repo.join("cart.ts");
        let mut store = Store::open_in_memory().unwrap();
        let told: Story =
            serde_json::from_value(json!({"summary": "s", "before": "Antes fallaba", "after": "Ahora suma",
            "example": {"scenario": "Un carrito con 2 productos", "walk": [
                {"what": "Se añade uno", "anchor": {"file": file, "symbol": "Cart.add"}}, {"what": "Se muestra"}]},
            "concepts": [{"term": "Carrito", "meaning": "Lo que el cliente va a comprar"}],
            "chapters": [{"id": "C1", "title": "T", "intent": "i", "anchors": [{"file": file, "symbol": "Cart"}]}]}))
            .unwrap();
        record(&mut store, told).unwrap();
        let path = file.display().to_string();
        let (review, plan) = review_of(&[(&path, CART)]);
        let repos = vec![(RepoRef::new(&repo.display().to_string()), "feature/A-1".to_owned())];
        let found = declared(&store, &review, &plan, &repos).unwrap().unwrap();
        std::fs::remove_dir_all(&repo).unwrap();
        assert_eq!((found.before.as_str(), found.after.as_str()), ("Antes fallaba", "Ahora suma"));
        let example = found.example.unwrap();
        assert_eq!(example.walk[0].step, Some(position(&plan, "Cart.add")));
        assert_eq!(example.walk[1].step, None);
        assert_eq!(found.concepts[0].term, "Carrito");
    }

    #[test]
    fn the_published_schema_is_valid_json_with_the_common_shape() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(schema["title"], VERSION);
        assert!(schema["properties"]["chapters"]["items"]["properties"]["questions"].is_object());
    }
}
