//! Lo que se le pide al agente al acabar cada turno para que quien revise no se
//! quede sin explicación: registrar con `huella_record` el código que escribió
//! sin decir nada, y contar con `huella_storyline` las ramas con commits que
//! ningún guion cuenta.
//!
//! Sin esto, registrar dependía de que el agente se acordara: en DUNGEON-1 hubo
//! 155 ediciones y 12 registros. Cada cosa se pide una sola vez por cómo es
//! (el paso por su contenido, la rama por su commit): si el agente responde que
//! no hay nada que contar, no se le insiste en el turno siguiente.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use huella_review::SessionReview;
use huella_review::flow::{FlowStep, StepRole};
use huella_store::Store;

use crate::Result;
use crate::declared::DeclaredView;
use crate::triage::{Level, StepTriage, Tag};

/// Pasos que se nombran en la petición; el resto se cuenta.
const NAMED: usize = 8;

/// Un paso que el agente escribió y no explicó.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepAsk {
    pub key: String,
    /// El contenido del paso: si cambia, se puede volver a pedir.
    pub hash: String,
    pub name: String,
    pub file: String,
    pub tag: Option<Tag>,
}

/// Una rama con commits que ningún guion declarado cuenta todavía.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoryAsk {
    pub root: String,
    pub branch: String,
    /// El commit de la rama al pedirlo.
    pub head: String,
}

/// Lo que habría que pedir en este punto de control, antes de quitar lo ya pedido.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordAsk {
    pub steps: Vec<StepAsk>,
    pub stories: Vec<StoryAsk>,
}

/// Lo que no merece una explicación propia: tests, documentación, configuración
/// o lo que se salta (generado, formato, imports).
fn needs_no_record(step: &FlowStep, triaged: &StepTriage) -> bool {
    step.role == StepRole::Test
        || triaged.level == Level::Skip
        || triaged.findings.iter().any(|finding| matches!(finding.tag, Tag::Tests | Tag::Docs | Tag::Config))
        // «Otros cambios» de un fichero (campos, imports sueltos): solo si son obligatorios.
        || (step.symbol.is_none() && triaged.level != Level::Must)
}

/// Los pasos del último turno con código que no tienen nada registrado que valga
/// para su código de ahora, primero lo obligatorio y lo que tiene etiqueta.
///
/// Solo el último turno: los anteriores ya tuvieron su fin de turno, y en una
/// sesión larga (o al instalar Huella a mitad) pedirlo todo de golpe no sirve.
#[must_use]
pub fn unrecorded(
    review: &SessionReview,
    plan: &[FlowStep],
    triage: &[StepTriage],
    declared: &BTreeMap<usize, Vec<DeclaredView>>,
) -> Vec<StepAsk> {
    let Some(last) = plan.iter().filter_map(|step| step.turn).max() else { return Vec::new() };
    let mut found: Vec<(&FlowStep, &StepTriage, usize)> = plan
        .iter()
        .zip(triage)
        .enumerate()
        .filter(|(_, (step, triaged))| step.turn == Some(last) && !needs_no_record(step, triaged))
        .filter(|(index, _)| !declared.get(index).is_some_and(|entries| entries.iter().any(|entry| !entry.stale)))
        .map(|(index, (step, triaged))| (step, triaged, index))
        .collect();
    // Lo que más importa, delante: es lo que se nombra si la lista es larga.
    found.sort_by_key(|(_, triaged, index)| (triaged.level != Level::Must, triaged.findings.is_empty(), *index));
    found
        .into_iter()
        .map(|(step, triaged, _)| (step, triaged))
        .map(|(step, triaged)| {
            let path = &review.files[step.file].path;
            let file = path.rsplit('/').next().unwrap_or(path).to_owned();
            StepAsk {
                key: step.key(review),
                hash: step.content_hash(),
                name: step.symbol.as_ref().map_or_else(|| file.clone(), |symbol| symbol.qualified_name.clone()),
                file,
                tag: triaged.findings.first().map(|finding| finding.tag),
            }
        })
        .collect()
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Las ramas que tocó la sesión con commits más nuevos que su último guion declarado.
///
/// # Errors
/// Si falla la base de datos.
pub fn stories_due(store: &Store, review: &SessionReview) -> Result<Vec<StoryAsk>> {
    let roots: BTreeSet<PathBuf> = review
        .files
        .iter()
        .filter_map(|file| Path::new(&file.path).parent().and_then(crate::task::repo_root))
        .collect();
    let mut due = Vec::new();
    for root in roots {
        let Some(branch) = crate::task::checked_out(&root) else { continue };
        let Ok(base) = crate::branch::default_base(&root) else { continue };
        let (Some(head), Some(fork)) = (git(&root, &["rev-parse", "HEAD"]), git(&root, &["merge-base", &base, "HEAD"]))
        else {
            continue;
        };
        // Sin commits propios (o en la rama principal) no hay nada que contar todavía.
        if head == fork {
            continue;
        }
        let committed =
            git(&root, &["log", "-1", "--format=%ct", "HEAD"]).and_then(|seconds| seconds.parse::<i64>().ok());
        let told = match crate::task::repo_identity(&root) {
            Some(identity) => store
                .latest_declared_storyline(&identity.display().to_string(), &branch)?
                .and_then(|stored| crate::branch::unix_seconds(&stored.recorded_at)),
            None => None,
        };
        if told.is_none_or(|told| committed.is_some_and(|committed| committed > told)) {
            due.push(StoryAsk { root: root.display().to_string(), branch, head });
        }
    }
    Ok(due)
}

/// La petición al agente con lo que aún no se le pidió, y la anota para no repetirla.
/// `None` si no hay nada nuevo que pedir.
///
/// # Errors
/// Si falla la base de datos.
pub fn request(store: &mut Store, session_id: &str, ask: &RecordAsk) -> Result<Option<String>> {
    let asked: HashMap<String, String> = store.record_requests(session_id)?;
    let fresh = |item: &str, fingerprint: &str| asked.get(item).is_none_or(|before| before != fingerprint);
    let steps: Vec<&StepAsk> = ask.steps.iter().filter(|step| fresh(&step.key, &step.hash)).collect();
    let stories: Vec<&StoryAsk> = ask.stories.iter().filter(|story| fresh(&story_item(story), &story.head)).collect();
    let Some(text) = text(&steps, &stories) else { return Ok(None) };
    let items: Vec<(String, String)> = steps
        .iter()
        .map(|step| (step.key.clone(), step.hash.clone()))
        .chain(stories.iter().map(|story| (story_item(story), story.head.clone())))
        .collect();
    store.save_record_requests(session_id, &items)?;
    Ok(Some(text))
}

fn story_item(story: &StoryAsk) -> String {
    format!("guion:{}", story.root)
}

fn text(steps: &[&StepAsk], stories: &[&StoryAsk]) -> Option<String> {
    if steps.is_empty() && stories.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !steps.is_empty() {
        let named: Vec<String> = steps
            .iter()
            .take(NAMED)
            .map(|step| {
                let place = if step.name == step.file { String::new() } else { format!(" en {}", step.file) };
                match step.tag {
                    Some(tag) => format!("{}{place} ({})", step.name, tag.label()),
                    None => format!("{}{place}", step.name),
                }
            })
            .collect();
        let more = steps.len().saturating_sub(named.len());
        let more = if more > 0 { format!(" y {more} más") } else { String::new() };
        parts.push(format!(
            "registra con huella_record lo que no se entiende solo al leer este código: lo que decidiste y por qué, lo \
             que diste por hecho, lo que no comprobaste y las reglas de negocio (si X → Y, porque Z). Pasos sin nada \
             registrado: {}{more}. Una entrada por cosa, anclada a su símbolo; si en alguno no hay nada que contar, \
             dilo en una frase",
            named.join("; ")
        ));
    }
    if !stories.is_empty() {
        let branches: Vec<String> = stories
            .iter()
            .map(|story| {
                let repo = story.root.rsplit('/').next().unwrap_or(&story.root);
                format!("{} en {repo}", story.branch)
            })
            .collect();
        parts.push(format!(
            "la rama {} tiene commits que ningún guion cuenta: si esa parte está terminada, relee lo registrado con \
             huella_recorded y cuéntala con huella_storyline; si aún no, dilo en una frase y sigue",
            branches.join(" y la rama ")
        ));
    }
    Some(format!(
        "Huella: antes de terminar, {}. Quien revise el cambio no tiene contexto y es lo que leerá. No se te volverá a \
         pedir por esto mismo.",
        parts.join("; y ")
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;
    use crate::triage::{Policy, triage};

    fn review_of(files: &[(&str, &str)]) -> (SessionReview, Vec<FlowStep>) {
        review_in_turns(&files.iter().map(|(path, content)| (*path, *content, "la tarea")).collect::<Vec<_>>())
    }

    /// Cada fichero, escrito en el turno de su petición.
    fn review_in_turns(files: &[(&str, &str, &str)]) -> (SessionReview, Vec<FlowStep>) {
        let edits: Vec<FileEdit> = files
            .iter()
            .enumerate()
            .map(|(n, (path, content, prompt))| FileEdit {
                origin: EditOrigin {
                    agent: "t",
                    session_id: "s1".into(),
                    event_id: format!("e{n}"),
                    timestamp: None,
                    cwd: None,
                },
                path: PathBuf::from(path),
                kind: EditKind::Create,
                before: None,
                after: AfterState::Known((*content).into()),
                user_modified: false,
                context: EditContext { prompt: Some((*prompt).into()), rationale: None },
            })
            .collect();
        let review = huella_review::build("s1", &edits, |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    const LOGIN: &str = "function login(password: string) {\n  return bcrypt.compare(password, hash);\n}\n";
    const TOTAL: &str = "export function total(prices: number[]) {\n  return prices.reduce((a, b) => a + b, 0);\n}\n";
    const SPEC: &str =
        "describe('total', () => {\n  it('suma', () => {\n    expect(total([1, 2])).toBe(3);\n  });\n});\n";

    #[test]
    fn code_without_a_record_is_asked_for_but_not_tests_or_docs() {
        let (review, plan) = review_of(&[
            ("/r/src/auth/login.ts", LOGIN),
            ("/r/src/total.ts", TOTAL),
            ("/r/src/total.spec.ts", SPEC),
            ("/r/README.md", "# Total\n\nSuma precios.\n"),
        ]);
        let triaged = triage(&review, &plan, &Policy::default(), &BTreeSet::new(), &BTreeSet::new());
        let asked: Vec<String> =
            unrecorded(&review, &plan, &triaged, &BTreeMap::new()).into_iter().map(|step| step.name).collect();
        assert_eq!(asked, vec!["login".to_owned(), "total".to_owned()], "ni el test ni el README");
    }

    #[test]
    fn only_the_last_turn_is_asked_for_with_what_matters_first() {
        let (review, plan) = review_in_turns(&[
            ("/r/src/old.ts", "export function old() {\n  return 1;\n}\n", "primera petición"),
            ("/r/src/total.ts", TOTAL, "segunda petición"),
            ("/r/src/auth/login.ts", LOGIN, "segunda petición"),
        ]);
        let triaged = triage(&review, &plan, &Policy::default(), &BTreeSet::new(), &BTreeSet::new());
        let asked: Vec<String> =
            unrecorded(&review, &plan, &triaged, &BTreeMap::new()).into_iter().map(|step| step.name).collect();
        assert_eq!(
            asked,
            vec!["login".to_owned(), "total".to_owned()],
            "lo de seguridad primero y nada del turno anterior"
        );
    }

    fn ask(steps: &[(&str, &str)]) -> RecordAsk {
        RecordAsk {
            steps: steps
                .iter()
                .map(|(key, hash)| StepAsk {
                    key: (*key).into(),
                    hash: (*hash).into(),
                    name: format!("Pedidos.{key}"),
                    file: "Pedidos.java".into(),
                    tag: None,
                })
                .collect(),
            stories: vec![StoryAsk { root: "/w/api".into(), branch: "feature/DENMS-1".into(), head: "c1".into() }],
        }
    }

    #[test]
    fn each_thing_is_asked_once_until_it_changes() {
        let mut store = Store::open_in_memory().unwrap();
        let first = request(&mut store, "s1", &ask(&[("reservar", "h1")])).unwrap().unwrap();
        assert!(first.contains("Pedidos.reservar en Pedidos.java"), "{first}");
        assert!(first.contains("feature/DENMS-1 en api") && first.contains("huella_storyline"), "{first}");
        assert_eq!(request(&mut store, "s1", &ask(&[("reservar", "h1")])).unwrap(), None, "lo mismo no se repite");

        let changed = request(&mut store, "s1", &ask(&[("reservar", "h2"), ("cancelar", "h1")])).unwrap().unwrap();
        assert!(changed.contains("Pedidos.reservar") && changed.contains("Pedidos.cancelar"), "{changed}");
        assert!(!changed.contains("huella_storyline"), "la rama sigue en el mismo commit: {changed}");
        assert!(request(&mut store, "otra", &ask(&[("reservar", "h1")])).unwrap().is_some(), "cada sesión aparte");
    }

    #[test]
    fn long_lists_are_cut() {
        let keys: Vec<(String, String)> = (0..11).map(|n| (format!("m{n}"), "h".to_owned())).collect();
        let refs: Vec<(&str, &str)> = keys.iter().map(|(key, hash)| (key.as_str(), hash.as_str())).collect();
        let mut only_steps = ask(&refs);
        only_steps.stories.clear();
        let text = request(&mut Store::open_in_memory().unwrap(), "s1", &only_steps).unwrap().unwrap();
        assert!(text.contains("y 3 más"), "{text}");
        assert!(!text.contains("huella_storyline"), "{text}");
    }
}
