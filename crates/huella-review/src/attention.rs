//! Puntos de atención de una revisión, sin LLM: lo que un revisor con
//! experiencia miraría dos veces.

use std::collections::BTreeSet;
use std::path::Path;

use huella_anchoring::{Language, extract_symbols, identifiers};
use serde::Serialize;

use crate::flow::{FlowStep, StepRole};
use crate::{DiffLine, SessionReview, Step};

/// Paso con más líneas añadidas que esto se señala como largo.
const LARGE_STEP_LINES: usize = 80;

/// Marcas que en un test son lo normal.
const FINE_IN_TESTS: &[&str] = &[".unwrap()", "console.log(", ": any", "as any"];

/// Nombres tan comunes que encontrarlos en otro fichero no indica nada.
const GENERIC_NAMES: &[&str] = &[
    "tests", "test", "main", "new", "default", "init", "index", "run", "get", "set", "add", "update", "delete",
    "create",
];

/// Marcas en líneas añadidas que suelen merecer una segunda mirada.
const MARKERS: &[(&str, &str)] = &[
    ("TODO", "deja un TODO"),
    ("FIXME", "deja un FIXME"),
    ("console.log(", "deja un console.log"),
    ("debugger", "deja un debugger"),
    ("dbg!(", "deja un dbg!"),
    (".unwrap()", "usa unwrap(): puede entrar en pánico"),
    (": any", "usa el tipo any"),
    ("as any", "convierte a any"),
    ("@ts-ignore", "silencia el compilador con @ts-ignore"),
    ("eslint-disable", "desactiva reglas de eslint"),
    ("#[allow(", "silencia un aviso del compilador"),
    ("# type: ignore", "silencia el comprobador de tipos"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// Líneas que no escribió el agente en una edición registrada.
    Unexplained,
    /// Se borró un símbolo cuyo nombre aún aparece en el código de la sesión.
    RemovedStillMentioned,
    /// El turno cambia código sin añadir ni tocar tests.
    NoTests,
    LargeStep,
    Marker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Attention {
    pub kind: AttentionKind,
    /// Turno al que se refiere; `None` = cambios sin explicación.
    pub turn: Option<usize>,
    /// Paso del plan al que se refiere, si es uno concreto.
    pub step: Option<usize>,
    pub message: String,
}

/// Puntos de atención de la revisión, en el orden del plan.
pub fn attention(review: &SessionReview, plan: &[FlowStep]) -> Vec<Attention> {
    let mut found = Vec::new();
    for (index, step) in plan.iter().enumerate() {
        let at = |kind, message: String| Attention { kind, turn: step.turn, step: Some(index), message };
        let external = step.lines.iter().filter(|line| is_external(review, line)).count();
        if external > 0 {
            found.push(at(AttentionKind::Unexplained, format!("{external} líneas sin explicación del agente")));
        }
        // En documentación o configuración un paso largo es normal.
        if step.role != StepRole::Other && step.added() > LARGE_STEP_LINES {
            found.push(at(AttentionKind::LargeStep, format!("paso largo: {} líneas añadidas", step.added())));
        }
        let mut reported = BTreeSet::new();
        for line in &step.lines {
            let DiffLine::Added { text, .. } = line else { continue };
            for (marker, message) in MARKERS {
                if step.role == StepRole::Test && FINE_IN_TESTS.contains(marker) {
                    continue;
                }
                if text.contains(marker) && reported.insert(*marker) {
                    found.push(at(AttentionKind::Marker, format!("{message}: `{}`", text.trim())));
                }
            }
        }
    }
    found.extend(turns_without_tests(plan));
    found.extend(removed_still_mentioned(review, plan));
    found
}

fn is_external(review: &SessionReview, line: &DiffLine) -> bool {
    matches!(line, DiffLine::Added { source: Some(s), .. } if matches!(review.steps[*s], Step::External { .. }))
}

fn turns_without_tests(plan: &[FlowStep]) -> Vec<Attention> {
    let turns: BTreeSet<usize> = plan.iter().filter(|s| s.role == StepRole::Flow).filter_map(|s| s.turn).collect();
    turns
        .into_iter()
        .filter(|turn| !plan.iter().any(|s| s.role == StepRole::Test && s.turn == Some(*turn)))
        .map(|turn| Attention {
            kind: AttentionKind::NoTests,
            turn: Some(turn),
            step: None,
            message: "cambia código sin añadir ni tocar tests".to_owned(),
        })
        .collect()
}

/// Símbolos que existían al empezar la sesión y ya no existen, pero cuyo
/// nombre sigue apareciendo en algún fichero final: posible referencia rota.
fn removed_still_mentioned(review: &SessionReview, plan: &[FlowStep]) -> Vec<Attention> {
    let mut mentions: Vec<(String, BTreeSet<String>)> = Vec::new();
    for file in &review.files {
        let Some(language) = Language::from_path(Path::new(&file.path)) else { continue };
        let names = identifiers(language, &file.after).unwrap_or_default().into_iter().map(|(_, n)| n).collect();
        mentions.push((file.path.clone(), names));
    }
    let mut found = Vec::new();
    for (file_index, file) in review.files.iter().enumerate() {
        let Some(language) = Language::from_path(Path::new(&file.path)) else { continue };
        let (Ok(before), Ok(after)) = (extract_symbols(language, &file.before), extract_symbols(language, &file.after))
        else {
            continue;
        };
        for removed in before
            .symbols
            .iter()
            .filter(|old| old.depth <= 1 && !after.symbols.iter().any(|new| new.qualified_name == old.qualified_name))
        {
            let short = removed.qualified_name.rsplit(['.', ':']).next().unwrap_or(&removed.qualified_name);
            // Un nombre que se reutiliza en el mismo fichero suele ser un renombrado interno.
            let users: Vec<&str> = mentions
                .iter()
                .filter(|(path, names)| path != &file.path && names.contains(short))
                .map(|(path, _)| path.rsplit('/').next().unwrap_or(path))
                .collect();
            if users.is_empty() || short.len() < 4 || GENERIC_NAMES.contains(&short) {
                continue;
            }
            let step = plan.iter().position(|s| s.file == file_index && s.role != StepRole::Other);
            found.push(Attention {
                kind: AttentionKind::RemovedStillMentioned,
                turn: step.and_then(|s| plan[s].turn),
                step,
                message: format!(
                    "se borró `{}` y su nombre aún aparece en {}",
                    removed.qualified_name,
                    users.join(", ")
                ),
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};

    use super::*;
    use crate::{build, flow::plan};

    fn edit(path: &str, before: Option<&str>, after: &str, prompt: &str) -> FileEdit {
        FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "s".into(),
                event_id: path.into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from(path),
            kind: if before.is_some() { EditKind::Replace } else { EditKind::Create },
            before: before.map(str::to_owned),
            after: AfterState::Known(after.into()),
            user_modified: false,
            context: EditContext { prompt: Some(prompt.into()), rationale: Some("motivo".into()) },
        }
    }

    fn kinds(points: &[Attention]) -> Vec<AttentionKind> {
        let mut kinds: Vec<AttentionKind> = points.iter().map(|p| p.kind).collect();
        kinds.sort();
        kinds
    }

    #[test]
    fn flags_markers_missing_tests_and_removed_names_still_in_use() {
        let old_lib =
            "export function legacyCheck() {\n  return true;\n}\n\nexport function keep() {\n  return 1;\n}\n";
        let new_lib = "export function keep() {\n  // TODO: validar\n  return 1;\n}\n";
        let caller = "import { legacyCheck } from './lib';\nexport function main() {\n  return legacyCheck();\n}\n";
        let edits = [edit("/r/lib.ts", Some(old_lib), new_lib, "limpia"), edit("/r/main.ts", None, caller, "limpia")];
        let review = build("s", &edits, |e| e.path.display().to_string(), |_| None);
        let points = attention(&review, &plan(&review));
        assert_eq!(
            kinds(&points),
            vec![AttentionKind::RemovedStillMentioned, AttentionKind::NoTests, AttentionKind::Marker]
        );
        let removed = points.iter().find(|p| p.kind == AttentionKind::RemovedStillMentioned).unwrap();
        assert_eq!(removed.message, "se borró `legacyCheck` y su nombre aún aparece en main.ts");
    }

    #[test]
    fn a_turn_with_tests_is_not_flagged() {
        let code = "export function f() {\n  return 1;\n}\n";
        let spec = "describe('f', () => {\n  it('devuelve 1', () => {});\n});\n";
        let edits = [edit("/r/f.ts", None, code, "p"), edit("/r/f.spec.ts", None, spec, "p")];
        let review = build("s", &edits, |e| e.path.display().to_string(), |_| None);
        assert_eq!(attention(&review, &plan(&review)), Vec::new());
    }
}
