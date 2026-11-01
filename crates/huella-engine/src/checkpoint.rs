//! Puntos de control mientras el agente trabaja: al acabar cada turno se cuenta
//! lo obligatorio que sigue sin revisar y se avisa; si la política lo pide, el
//! agente no puede seguir editando con demasiado pendiente. Así la revisión va
//! al paso del agente en lugar de acumularse para el final.

use std::collections::HashMap;
use std::hash::BuildHasher;

use huella_review::SessionReview;
use huella_review::flow::FlowStep;
use huella_store::{StepVerdict, Store, StoredCheckpoint};

use crate::Result;
use crate::triage::{Level, Policy, StepTriage, Tag};

/// Pasos pendientes que se nombran en el aviso.
const NAMED: usize = 3;

/// Un paso obligatorio sin veredicto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub name: String,
    pub file: String,
    /// La etiqueta más exigente, la que lo hizo obligatorio.
    pub tag: Option<Tag>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub session_id: String,
    pub must_total: usize,
    pub pending: Vec<Pending>,
    /// Pendientes de más respecto al punto de control anterior.
    pub new_pending: usize,
    pub policy: Policy,
}

/// Lo obligatorio de la revisión y lo que sigue sin un veredicto que valga
/// para el contenido actual del paso.
#[must_use]
pub fn count<S: BuildHasher>(
    review: &SessionReview,
    plan: &[FlowStep],
    triage: &[StepTriage],
    verdicts: &HashMap<String, StepVerdict, S>,
) -> (usize, Vec<Pending>) {
    let mut total = 0;
    let mut pending = Vec::new();
    for (step, triaged) in plan.iter().zip(triage) {
        if triaged.level != Level::Must {
            continue;
        }
        total += 1;
        let decided =
            verdicts.get(&step.key(review)).is_some_and(|verdict| verdict.content_hash == step.content_hash());
        if !decided {
            let path = &review.files[step.file].path;
            pending.push(Pending {
                name: step.symbol.as_ref().map_or_else(|| file_name(path), |symbol| symbol.qualified_name.clone()),
                file: file_name(path),
                tag: triaged.findings.first().map(|finding| finding.tag),
            });
        }
    }
    (total, pending)
}

/// Punto de control de la sesión: lo cuenta y lo guarda para la próxima vez.
/// `None` si la sesión no tiene ediciones.
///
/// # Errors
/// Si falla la base de datos o no se pueden leer las sesiones.
pub fn check(
    store: &Store,
    sources: &crate::Sources,
    session_id: &str,
    project: Option<&str>,
) -> Result<Option<Checkpoint>> {
    let Ok(review) = crate::session_review(sources, session_id) else { return Ok(None) };
    let plan = huella_review::flow::plan(&review);
    if plan.is_empty() {
        return Ok(None);
    }
    let points = huella_review::attention::attention(&review, &plan);
    let activity = crate::evidence::session_activity(sources, &review.session_id)?;
    let project = match project {
        Some(project) => Some(project.to_owned()),
        None => crate::session_project(store, &review.session_id)?,
    };
    let assessment = crate::triage::assess(store, &review, &plan, &points, &activity, project.as_deref())?;
    let verdicts = store.review_verdicts(&review.session_id)?;
    let (must_total, pending) = count(&review, &plan, &assessment.triage, &verdicts);
    let previous = store.checkpoint(&review.session_id)?.map_or(0, |stored| stored.must_pending);
    store.save_checkpoint(&review.session_id, StoredCheckpoint { must_total, must_pending: pending.len() })?;
    Ok(Some(Checkpoint {
        new_pending: pending.len().saturating_sub(previous),
        session_id: review.session_id,
        must_total,
        pending,
        policy: assessment.policy,
    }))
}

/// Si el agente debe parar antes de editar, por qué. Lee el recuento del último
/// punto de control y solo lo recalcula cuando bloquearía: la persona puede
/// haber revisado desde entonces.
///
/// # Errors
/// Si falla la base de datos o no se pueden leer las sesiones.
pub fn gate(
    store: &Store,
    sources: &crate::Sources,
    session_id: &str,
    project: Option<&str>,
    policy: &Policy,
) -> Result<Option<String>> {
    let Some(limit) = policy.checkpoints.block_after else { return Ok(None) };
    let cached = store.checkpoint(session_id)?.map_or(0, |stored| stored.must_pending);
    if cached < limit.max(1) {
        return Ok(None);
    }
    let Some(checkpoint) = check(store, sources, session_id, project)? else { return Ok(None) };
    Ok((checkpoint.pending.len() >= limit.max(1)).then(|| block_reason(&checkpoint, limit)))
}

/// El aviso para la persona al acabar un turno, si hay algo pendiente.
#[must_use]
pub fn notice(checkpoint: &Checkpoint) -> Option<String> {
    let pending = checkpoint.pending.len();
    if pending == 0 || !checkpoint.policy.checkpoints.notify {
        return None;
    }
    let steps = if pending == 1 { "1 paso obligatorio".to_owned() } else { format!("{pending} pasos obligatorios") };
    let new = match checkpoint.new_pending {
        0 => String::new(),
        n if n == pending => String::new(),
        n => format!(", {n} de este turno"),
    };
    let named: Vec<String> = checkpoint
        .pending
        .iter()
        .rev()
        .take(NAMED)
        .map(|step| {
            // Entre paréntesis, el fichero (si no es el propio paso) y la etiqueta.
            let detail: Vec<&str> = [(step.name != step.file).then_some(step.file.as_str()), step.tag.map(Tag::label)]
                .into_iter()
                .flatten()
                .collect();
            if detail.is_empty() { step.name.clone() } else { format!("{} ({})", step.name, detail.join(", ")) }
        })
        .collect();
    let more = if pending > NAMED { format!(" y {} más", pending - NAMED) } else { String::new() };
    Some(format!(
        "Huella: {steps} sin revisar{new}: {}{more}. Revísalos en Huella (`huella ui`, sesión {}).",
        named.join(", "),
        short(&checkpoint.session_id)
    ))
}

/// Por qué no puede seguir editando, dicho al agente.
#[must_use]
pub fn block_reason(checkpoint: &Checkpoint, limit: usize) -> String {
    format!(
        "Huella: hay {} pasos obligatorios sin revisar en esta sesión y la política del repositorio \
         (.huella.toml, block_after = {limit}) no deja seguir editando así. No intentes otra edición: \
         termina el turno pidiendo a la persona que los revise en Huella (`huella ui`, sesión {}) y \
         espera a que te diga que puedes continuar.",
        checkpoint.pending.len(),
        short(&checkpoint.session_id)
    )
}

fn short(session_id: &str) -> &str {
    &session_id[..8.min(session_id.len())]
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};
    use huella_store::Verdict;

    use super::*;
    use crate::triage::triage;

    fn review_of(files: &[(&str, &str)]) -> (SessionReview, Vec<FlowStep>) {
        let edits: Vec<FileEdit> = files
            .iter()
            .enumerate()
            .map(|(n, (path, content))| FileEdit {
                origin: EditOrigin {
                    agent: "t",
                    session_id: "s1234567890".into(),
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
        let review = huella_review::build("s1234567890", &edits, |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    const LOGIN: &str = "function login(password: string) {\n  return bcrypt.compare(password, hash);\n}\n";
    const TWICE: &str = "function twice(n: number) {\n  return n * 2;\n}\n";

    #[test]
    fn only_must_steps_without_a_current_verdict_are_pending() {
        let (review, plan) = review_of(&[("/r/src/auth/login.ts", LOGIN), ("/r/src/math.ts", TWICE)]);
        let triaged = triage(&review, &plan, &Policy::default(), &BTreeSet::new(), &BTreeSet::new());
        let (total, pending) = count(&review, &plan, &triaged, &HashMap::new());
        assert_eq!(total, 1);
        assert_eq!(pending, vec![Pending { name: "login".into(), file: "login.ts".into(), tag: Some(Tag::Security) }]);

        let login =
            plan.iter().position(|step| step.symbol.as_ref().is_some_and(|s| s.qualified_name == "login")).unwrap();
        let mut verdicts = HashMap::new();
        verdicts.insert(
            plan[login].key(&review),
            StepVerdict { content_hash: "otro contenido".into(), verdict: Verdict::Ok },
        );
        assert_eq!(count(&review, &plan, &triaged, &verdicts).1.len(), 1, "un veredicto de otro contenido no vale");
        verdicts.insert(
            plan[login].key(&review),
            StepVerdict { content_hash: plan[login].content_hash(), verdict: Verdict::Ok },
        );
        assert_eq!(count(&review, &plan, &triaged, &verdicts).1.len(), 0);
    }

    fn checkpoint(pending: usize, new_pending: usize) -> Checkpoint {
        Checkpoint {
            session_id: "84e33d8e-aaaa".into(),
            must_total: pending + 1,
            pending: (0..pending)
                .map(|n| Pending { name: format!("paso{n}"), file: "a.ts".into(), tag: Some(Tag::Security) })
                .collect(),
            new_pending,
            policy: Policy::default(),
        }
    }

    #[test]
    fn the_notice_names_the_latest_steps_and_what_is_new() {
        assert_eq!(notice(&checkpoint(0, 0)), None);
        let text = notice(&checkpoint(5, 2)).unwrap();
        assert_eq!(
            text,
            "Huella: 5 pasos obligatorios sin revisar, 2 de este turno: paso4 (a.ts, seguridad), \
             paso3 (a.ts, seguridad), paso2 (a.ts, seguridad) y 2 más. Revísalos en Huella (`huella ui`, sesión 84e33d8e)."
        );
        let mut quiet = checkpoint(2, 2);
        quiet.policy.checkpoints.notify = false;
        assert_eq!(notice(&quiet), None);
    }

    #[test]
    fn the_block_reason_tells_the_agent_to_stop_and_ask() {
        let reason = block_reason(&checkpoint(6, 0), 5);
        assert!(reason.contains("hay 6 pasos obligatorios sin revisar"), "{reason}");
        assert!(reason.contains("block_after = 5"), "{reason}");
        assert!(reason.contains("espera a que te diga que puedes continuar"), "{reason}");
    }

    #[test]
    fn without_a_limit_the_gate_never_reads_the_session() {
        let store = Store::open_in_memory().unwrap();
        let sources = crate::Sources::claude_code(std::path::Path::new("/no-existe"));
        store.save_checkpoint("s1", StoredCheckpoint { must_total: 9, must_pending: 9 }).unwrap();
        assert_eq!(gate(&store, &sources, "s1", None, &Policy::default()).unwrap(), None);
        let mut strict = Policy::default();
        strict.checkpoints.block_after = Some(10);
        assert_eq!(gate(&store, &sources, "s1", None, &strict).unwrap(), None, "por debajo del límite no recalcula");
    }
}
