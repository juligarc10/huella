//! Evidencia de cada paso de la revisión: qué miró el agente antes de
//! escribirlo, qué pensó, con qué plan trabajaba, qué ejecutó después para
//! comprobarlo y qué intentó y descartó.
//!
//! Se lee de la transcripción al abrir la revisión, sin guardarla: la salida
//! de las órdenes puede ser larga y no aporta nada fuera de la revisión.

use std::collections::{BTreeSet, HashMap};

use huella_core::{Activity, ActivityKind, PlanItem};
use huella_review::flow::FlowStep;
use huella_review::{SessionReview, Step};
use serde::Serialize;

use crate::Result;

/// Razonamientos previos a un paso que se muestran: los últimos, que son los
/// que explican esa edición.
const MAX_REASONING: usize = 3;
const MAX_LOOKED_AT: usize = 40;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StepEvidence {
    /// El agente guarda su actividad y se encontraron las ediciones del paso.
    pub available: bool,
    /// Lecturas, búsquedas y órdenes de exploración del turno antes de su
    /// primera edición.
    pub looked_at: Vec<Activity>,
    /// Lo último que pensó o explicó en el turno antes de escribirlo.
    pub reasoning: Vec<String>,
    /// Su lista de tareas en ese momento.
    pub plan: Option<Vec<PlanItem>>,
    /// Comprobaciones (tests, compilación, lint…) que ejecutó después de su
    /// última edición, en el mismo turno.
    pub checks: Vec<Activity>,
    /// Líneas que escribió para este código y rehízo o borró después.
    pub discarded: Vec<String>,
}

/// Actividad de una sesión, de todos los ficheros de sus agentes, en orden.
///
/// # Errors
/// Si un fichero de sesión no se puede leer.
pub fn session_activity(sources: &crate::Sources, session_id: &str) -> Result<Vec<Activity>> {
    let mut activity = Vec::new();
    for agent in sources.agents() {
        for file in agent.sessions()? {
            // Leer el fichero es barato; interpretarlo, no: solo los que la mencionan.
            if std::fs::read_to_string(&file).is_ok_and(|text| text.contains(session_id)) {
                activity.extend(agent.activity(&file)?.into_iter().filter(|a| a.session_id == session_id));
            }
        }
    }
    // Una sesión continuada se reparte entre ficheros: orden por tiempo, estable.
    activity.sort_by(|a, b| a.at.cmp(&b.at));
    Ok(activity)
}

/// Lo último que pensó o explicó el agente en su turno antes de la edición
/// `event_id` (como mucho `max` textos). Vacío si la edición no aparece.
pub fn reasoning_before(activity: &[Activity], event_id: &str, max: usize) -> Vec<String> {
    let Some(edit) =
        activity.iter().position(|a| matches!(&a.kind, ActivityKind::Edit { event_id: id, .. } if id == event_id))
    else {
        return Vec::new();
    };
    let start = activity[..edit].iter().rposition(|a| matches!(a.kind, ActivityKind::Prompt { .. })).unwrap_or(0);
    let texts: Vec<String> = activity[start..edit]
        .iter()
        .filter_map(|a| match &a.kind {
            ActivityKind::Reasoning { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    texts[texts.len().saturating_sub(max)..].to_vec()
}

/// Evidencia de un paso del guion a partir de la actividad de su sesión.
pub fn step_evidence(review: &SessionReview, step: &FlowStep, activity: &[Activity]) -> StepEvidence {
    let edit_position: HashMap<&str, usize> = activity
        .iter()
        .enumerate()
        .filter_map(|(index, a)| match &a.kind {
            ActivityKind::Edit { event_id, .. } => Some((event_id.as_str(), index)),
            _ => None,
        })
        .collect();
    let positions: Vec<usize> = step
        .sources
        .iter()
        .filter_map(|&source| match &review.steps[source] {
            Step::Agent { event_id, .. } => edit_position.get(event_id.as_str()).copied(),
            Step::External { .. } => None,
        })
        .collect();
    let discarded = discarded_lines(review, step);
    let (Some(&first), Some(&last)) = (positions.iter().min(), positions.iter().max()) else {
        return StepEvidence { discarded, ..StepEvidence::default() };
    };
    let turn_start = activity[..first].iter().rposition(|a| matches!(a.kind, ActivityKind::Prompt { .. })).unwrap_or(0);
    let turn_end = activity[last..]
        .iter()
        .position(|a| matches!(a.kind, ActivityKind::Prompt { .. }))
        .map_or(activity.len(), |offset| last + offset);
    let before = &activity[turn_start..first];

    let mut seen = BTreeSet::new();
    let looked_at: Vec<Activity> = before
        .iter()
        .filter(|a| match &a.kind {
            ActivityKind::Read { target } => seen.insert(format!("r:{target}")),
            ActivityKind::Search { query, scope } => seen.insert(format!("s:{query}:{scope:?}")),
            // Muchos agentes exploran por la terminal (grep, cat, head).
            ActivityKind::Command { command, .. } => seen.insert(format!("c:{command}")),
            _ => false,
        })
        .take(MAX_LOOKED_AT)
        .cloned()
        .collect();

    let reasoning: Vec<String> = before
        .iter()
        .filter_map(|a| match &a.kind {
            ActivityKind::Reasoning { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let reasoning = reasoning[reasoning.len().saturating_sub(MAX_REASONING)..].to_vec();

    let plan = activity[..first].iter().rev().find_map(|a| match &a.kind {
        ActivityKind::Plan { items } => Some(items.clone()),
        _ => None,
    });
    let checks = activity[last..turn_end].iter().filter(|a| a.kind.is_check()).cloned().collect();
    StepEvidence { available: true, looked_at, reasoning, plan, checks, discarded }
}

/// Descartes de las ediciones del paso, y de las ediciones del mismo fichero y
/// turno que no dejaron ninguna línea (se rehicieron enteras).
fn discarded_lines(review: &SessionReview, step: &FlowStep) -> Vec<String> {
    let path = &review.files[step.file].path;
    let surviving: BTreeSet<usize> =
        review.files.iter().flat_map(|file| &file.hunks).flat_map(|hunk| hunk.sources.iter().copied()).collect();
    review
        .steps
        .iter()
        .enumerate()
        .filter_map(|(index, s)| match s {
            Step::Agent { turn, path: step_path, discarded, .. }
                if step.sources.contains(&index)
                    || (step_path == path && Some(*turn) == step.turn && !surviving.contains(&index)) =>
            {
                Some(discarded)
            }
            _ => None,
        })
        .flatten()
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;

    fn edit(event: &str, before: Option<&str>, after: &str) -> FileEdit {
        FileEdit {
            origin: EditOrigin {
                agent: "test",
                session_id: "s".into(),
                event_id: event.into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from("a.ts"),
            kind: if before.is_some() { EditKind::Replace } else { EditKind::Create },
            before: before.map(str::to_owned),
            after: AfterState::Known(after.into()),
            user_modified: false,
            context: EditContext { prompt: Some("haz a".into()), rationale: None },
        }
    }

    fn act(kind: ActivityKind) -> Activity {
        Activity { session_id: "s".into(), at: None, kind }
    }

    fn command(text: &str, ok: bool) -> ActivityKind {
        ActivityKind::Command { command: text.into(), description: None, ok: Some(ok), output: String::new() }
    }

    #[test]
    fn splits_the_turn_into_what_came_before_and_the_checks_after() {
        let slow = "function a() {\n  return slow();\n}\n";
        let edits = [edit("e1", None, slow), edit("e2", Some(slow), "function a() {\n  return fast();\n}\n")];
        let review = huella_review::build("s", &edits, |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let activity = vec![
            act(ActivityKind::Prompt { text: "otra cosa".into() }),
            act(command("npm run lint", true)),
            act(ActivityKind::Prompt { text: "haz a".into() }),
            act(ActivityKind::Plan {
                items: vec![PlanItem { text: "Escribir a".into(), status: "in_progress".into() }],
            }),
            act(ActivityKind::Read { target: "b.ts".into() }),
            act(ActivityKind::Read { target: "b.ts".into() }),
            act(command("grep -rn slow src", true)),
            act(ActivityKind::Reasoning { text: "Uso slow primero".into() }),
            act(ActivityKind::Edit { event_id: "e1".into(), path: "a.ts".into() }),
            act(ActivityKind::Reasoning { text: "Mejor fast".into() }),
            act(ActivityKind::Edit { event_id: "e2".into(), path: "a.ts".into() }),
            act(command("mkdir -p deploy", true)),
            act(command("npm test", false)),
            act(ActivityKind::Prompt { text: "siguiente".into() }),
            act(command("npm test", true)),
        ];
        let step = plan.iter().find(|s| s.symbol.is_some()).expect("paso de a");
        let evidence = step_evidence(&review, step, &activity);
        assert!(evidence.available);
        assert_eq!(evidence.looked_at.len(), 2, "una lectura (repetida) y una exploración por terminal");
        assert_eq!(evidence.plan.as_ref().map(Vec::len), Some(1));
        assert_eq!(evidence.reasoning, vec!["Uso slow primero".to_owned()]);
        assert_eq!(evidence.checks.len(), 1, "solo comprobaciones, después y en el mismo turno");
        assert!(matches!(&evidence.checks[0].kind, ActivityKind::Command { ok: Some(false), .. }));
        assert_eq!(evidence.discarded, vec!["  return slow();".to_owned()]);
    }

    #[test]
    fn reasoning_before_an_edit_stays_within_its_turn() {
        let activity = vec![
            act(ActivityKind::Reasoning { text: "de otro turno".into() }),
            act(ActivityKind::Prompt { text: "haz a".into() }),
            act(ActivityKind::Reasoning { text: "miro la config".into() }),
            act(ActivityKind::Reasoning { text: "añado la comprobación".into() }),
            act(ActivityKind::Edit { event_id: "e1".into(), path: "a.ts".into() }),
        ];
        assert_eq!(reasoning_before(&activity, "e1", 1), vec!["añado la comprobación".to_owned()]);
        assert_eq!(reasoning_before(&activity, "e1", 5).len(), 2);
        assert_eq!(reasoning_before(&activity, "otra", 3), Vec::<String>::new());
    }

    #[test]
    fn agents_without_activity_say_so() {
        let edits = [edit("e1", None, "function a() {}\n")];
        let review = huella_review::build("s", &edits, |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        let evidence = step_evidence(&review, &plan[0], &[]);
        assert!(!evidence.available);
    }
}
