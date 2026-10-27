//! Modelo de la revisión de una sesión: qué cambió en cada fichero y por qué.
//!
//! A partir de las ediciones de una sesión se reconstruye la historia de cada
//! fichero y se calcula su diff neto (estado inicial frente a final). Cada
//! bloque del diff se atribuye a las ediciones que produjeron sus líneas. Si el
//! fichero cambió entre dos ediciones del agente (un script, un formateador, la
//! persona), ese cambio aparece como **externo**: sin explicación, y por eso
//! mismo lo que más conviene revisar.

pub mod flow;

use std::collections::BTreeMap;

use huella_core::{AfterState, FileEdit};
use serde::Serialize;
use similar::{DiffOp, DiffTag, TextDiff};

/// Líneas de contexto alrededor de cada bloque.
const CONTEXT_LINES: usize = 3;

/// Revisión de una sesión completa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionReview {
    pub session_id: String,
    pub turns: Vec<Turn>,
    /// Cada paso que cambió algún fichero, en orden: ediciones del agente y
    /// cambios externos detectados entre ellas.
    pub steps: Vec<Step>,
    pub files: Vec<FileReview>,
    /// Ediciones que no se pudieron reconstruir (la transcripción no guardó el
    /// fichero original). Sus cambios aparecen como externos.
    pub unreconstructable: usize,
}

/// Un turno de la conversación: lo que pidió la persona.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Turn {
    pub index: usize,
    pub prompt: Option<String>,
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    /// Edición del agente, con su explicación.
    Agent { turn: usize, at: Option<String>, rationale: Option<String> },
    /// El fichero cambió fuera de las ediciones del agente.
    External { reason: ExternalReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalReason {
    /// Cambio entre dos ediciones: terminal, formateador, otra herramienta o la persona.
    BetweenEdits,
    /// Edición cuyo fichero completo no quedó en la transcripción.
    Unreconstructable,
    /// El fichero actual difiere del que dejó la última edición del agente:
    /// cambios hechos después en la misma sesión por terminal, o a mano.
    AfterLastEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Created,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileReview {
    pub path: String,
    pub status: FileStatus,
    pub before: String,
    pub after: String,
    pub hunks: Vec<Hunk>,
}

/// Bloque del diff neto, con sus líneas de contexto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hunk {
    /// Primera línea en el fichero inicial y en el final (1-based).
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
    /// Índices en [`SessionReview::steps`] de los pasos que produjeron el bloque.
    pub sources: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub enum DiffLine {
    Context(String),
    /// Línea nueva y el paso que la escribió (`None` = estaba al empezar).
    Added {
        text: String,
        source: Option<usize>,
    },
    Removed(String),
}

impl Hunk {
    pub fn added(&self) -> usize {
        self.lines.iter().filter(|line| matches!(line, DiffLine::Added { .. })).count()
    }

    pub fn removed(&self) -> usize {
        self.lines.iter().filter(|line| matches!(line, DiffLine::Removed(_))).count()
    }
}

impl SessionReview {
    /// Bloques cuyo origen incluye algún cambio externo.
    pub fn external_hunks(&self) -> usize {
        self.files
            .iter()
            .flat_map(|file| &file.hunks)
            .filter(|hunk| hunk.sources.iter().any(|&s| matches!(self.steps[s], Step::External { .. })))
            .count()
    }
}

/// Construye la revisión. `edits` son las ediciones de la sesión en orden y
/// `key` da la ruta con la que se agrupa cada fichero (la normalizada).
/// `current` da el contenido actual de un fichero, si procede compararlo:
/// lo que difiera del último estado del agente es un cambio externo.
pub fn build(
    session_id: &str,
    edits: &[FileEdit],
    key: impl Fn(&FileEdit) -> String,
    current: impl Fn(&str) -> Option<String>,
) -> SessionReview {
    let turns = turns(edits);
    let turn_of = |index: usize| {
        let prompt = &edits[index].context.prompt;
        turns.iter().rev().find(|turn| &turn.prompt == prompt).map_or(0, |turn| turn.index)
    };

    let mut steps: Vec<Step> = Vec::new();
    let mut histories: BTreeMap<String, History> = BTreeMap::new();
    let mut unreconstructable = 0;
    for (index, edit) in edits.iter().enumerate() {
        let history = histories.entry(key(edit)).or_insert_with(|| History::start(edit));
        let AfterState::Known(after) = &edit.after else {
            unreconstructable += 1;
            history.pending_unknown = true;
            continue;
        };
        let expected = edit.before.as_deref().unwrap_or_default();
        if history.current != expected {
            let reason = if std::mem::take(&mut history.pending_unknown) {
                ExternalReason::Unreconstructable
            } else {
                ExternalReason::BetweenEdits
            };
            steps.push(Step::External { reason });
            history.apply(expected, steps.len() - 1);
        }
        steps.push(Step::Agent {
            turn: turn_of(index),
            at: edit.origin.timestamp.clone(),
            rationale: edit.context.rationale.clone(),
        });
        history.apply(after, steps.len() - 1);
    }

    for (path, history) in &mut histories {
        if let Some(now) = current(path)
            && now != history.current
        {
            steps.push(Step::External { reason: ExternalReason::AfterLastEdit });
            history.apply(&now, steps.len() - 1);
        }
    }

    let files = histories.into_iter().filter_map(|(path, history)| history.finish(path)).collect();
    SessionReview { session_id: session_id.to_owned(), turns, steps, files, unreconstructable }
}

/// Un turno nuevo empieza cada vez que cambia la petición de la persona.
fn turns(edits: &[FileEdit]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for edit in edits {
        if turns.last().is_none_or(|turn| turn.prompt != edit.context.prompt) {
            turns.push(Turn {
                index: turns.len(),
                prompt: edit.context.prompt.clone(),
                started_at: edit.origin.timestamp.clone(),
            });
        }
    }
    turns
}

/// Historia de un fichero durante la sesión, con el paso que escribió cada
/// línea del estado actual y los borrados pendientes de atribuir.
struct History {
    initial: String,
    created: bool,
    current: String,
    /// Paso que escribió cada línea actual; `None` = línea original.
    provenance: Vec<Option<usize>>,
    /// Borrados: (línea del estado actual delante de la que se borró, paso).
    deletions: Vec<(usize, usize)>,
    pending_unknown: bool,
}

impl History {
    fn start(edit: &FileEdit) -> Self {
        let initial = edit.before.clone().unwrap_or_default();
        let provenance = vec![None; lines(&initial).len()];
        Self {
            created: edit.before.is_none(),
            current: initial.clone(),
            initial,
            provenance,
            deletions: Vec::new(),
            pending_unknown: false,
        }
    }

    /// Pasa al estado `next`; las líneas nuevas o cambiadas se atribuyen a `step`.
    fn apply(&mut self, next: &str, step: usize) {
        let (old, new) = (lines(&self.current), lines(next));
        let diff = TextDiff::from_slices(&old, &new);
        let mut provenance = Vec::with_capacity(new.len());
        let mut deletions = Vec::new();
        for op in diff.ops() {
            let (tag, old_range, new_range) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => provenance.extend_from_slice(&self.provenance[old_range]),
                DiffTag::Insert | DiffTag::Replace => provenance.extend(new_range.map(|_| Some(step))),
                DiffTag::Delete => deletions.push((new_range.start, step)),
            }
        }
        for &(position, source) in &self.deletions {
            deletions.push((map_position(diff.ops(), position), source));
        }
        self.provenance = provenance;
        self.deletions = deletions;
        next.clone_into(&mut self.current);
    }

    fn finish(self, path: String) -> Option<FileReview> {
        let (old, new) = (lines(&self.initial), lines(&self.current));
        let diff = TextDiff::from_slices(&old, &new);
        let hunks: Vec<Hunk> =
            diff.grouped_ops(CONTEXT_LINES).iter().map(|group| self.hunk(group, &old, &new)).collect();
        if hunks.is_empty() {
            return None;
        }
        let status = if self.created { FileStatus::Created } else { FileStatus::Modified };
        Some(FileReview { path, status, before: self.initial, after: self.current, hunks })
    }

    fn hunk(&self, group: &[DiffOp], old: &[&str], new: &[&str]) -> Hunk {
        let mut lines = Vec::new();
        let mut sources = Vec::new();
        let mut changed_new = (usize::MAX, 0);
        for op in group {
            let (tag, old_range, new_range) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => lines.extend(new[new_range].iter().map(|l| DiffLine::Context(trim(l)))),
                DiffTag::Delete | DiffTag::Insert | DiffTag::Replace => {
                    lines.extend(old[old_range].iter().map(|l| DiffLine::Removed(trim(l))));
                    lines.extend(
                        new[new_range.clone()]
                            .iter()
                            .zip(&self.provenance[new_range.clone()])
                            .map(|(l, source)| DiffLine::Added { text: trim(l), source: *source }),
                    );
                    sources.extend(self.provenance[new_range.clone()].iter().flatten().copied());
                    changed_new = (changed_new.0.min(new_range.start), changed_new.1.max(new_range.end));
                }
            }
        }
        // Los borrados se atribuyen por la posición donde estaba el texto.
        let (from, to) = changed_new;
        sources.extend(self.deletions.iter().filter(|(at, _)| *at >= from && *at <= to).map(|(_, step)| *step));
        sources.sort_unstable();
        sources.dedup();
        let (first_old, first_new) = group.first().map_or((0, 0), |op| (op.old_range().start, op.new_range().start));
        Hunk { old_start: first_old + 1, new_start: first_new + 1, lines, sources }
    }
}

/// Posición de una línea del texto viejo en el texto nuevo. Las inserciones no
/// desplazan a la línea a la que preceden: se salta a la operación que la cubre.
fn map_position(ops: &[DiffOp], position: usize) -> usize {
    for op in ops {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        if old_range.contains(&position) {
            return match tag {
                DiffTag::Equal => new_range.start + (position - old_range.start),
                _ => new_range.start,
            };
        }
    }
    ops.last().map_or(position, |op| op.new_range().end)
}

fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn trim(line: &str) -> String {
    line.trim_end_matches(['\n', '\r']).to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_core::{EditContext, EditKind, EditOrigin};

    use super::*;

    fn edit(path: &str, before: Option<&str>, after: &str, prompt: &str, rationale: &str) -> FileEdit {
        FileEdit {
            origin: EditOrigin {
                agent: "test",
                session_id: "s".into(),
                event_id: rationale.into(),
                timestamp: Some("2026-10-27T10:00:00Z".into()),
                cwd: None,
            },
            path: PathBuf::from(path),
            kind: if before.is_some() { EditKind::Replace } else { EditKind::Create },
            before: before.map(str::to_owned),
            after: AfterState::Known(after.into()),
            user_modified: false,
            context: EditContext { prompt: Some(prompt.into()), rationale: Some(rationale.into()) },
        }
    }

    fn review(edits: &[FileEdit]) -> SessionReview {
        build("s", edits, |edit| edit.path.display().to_string(), |_| None)
    }

    fn rationales(review: &SessionReview, hunk: &Hunk) -> Vec<String> {
        hunk.sources
            .iter()
            .map(|&s| match &review.steps[s] {
                Step::Agent { rationale, .. } => rationale.clone().unwrap_or_default(),
                Step::External { .. } => "externo".into(),
            })
            .collect()
    }

    const BASE: &str = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";

    #[test]
    fn net_diff_attributes_each_hunk_to_the_edit_that_wrote_it() {
        let first = BASE.replace("b\n", "B\n");
        let second = first.replace("k\n", "K\n");
        let r = review(&[
            edit("x.ts", Some(BASE), &first, "cambia b", "pongo b en mayúscula"),
            edit("x.ts", Some(&first), &second, "cambia k", "pongo k en mayúscula"),
        ]);
        let file = &r.files[0];
        assert_eq!(file.status, FileStatus::Modified);
        assert_eq!(file.hunks.len(), 2);
        assert_eq!(rationales(&r, &file.hunks[0]), vec!["pongo b en mayúscula"]);
        assert_eq!(rationales(&r, &file.hunks[1]), vec!["pongo k en mayúscula"]);
        assert_eq!(r.turns.len(), 2);
        assert_eq!(r.external_hunks(), 0);
    }

    #[test]
    fn a_line_rewritten_twice_keeps_only_the_last_author() {
        let first = BASE.replace("b\n", "B\n");
        let second = first.replace("B\n", "Bb\n");
        let r = review(&[
            edit("x.ts", Some(BASE), &first, "p", "primera versión"),
            edit("x.ts", Some(&first), &second, "p", "versión final"),
        ]);
        assert_eq!(rationales(&r, &r.files[0].hunks[0]), vec!["versión final"]);
        assert_eq!(r.turns.len(), 1, "misma petición, mismo turno");
    }

    #[test]
    fn changes_between_edits_are_external_and_unexplained() {
        // Cambios separados por más de 6 líneas para que el diff no los fusione.
        let base: String = (0..40).map(|n| format!("línea {n}\n")).collect::<Vec<_>>().concat();
        let agent = base.replace("línea 2\n", "LÍNEA 2\n");
        let formatted = agent.replace("línea 20\n", "línea 20;\n");
        let next = formatted.replace("línea 35\n", "LÍNEA 35\n");
        let r = review(&[
            edit("x.ts", Some(&base), &agent, "p", "cambio b"),
            edit("x.ts", Some(&formatted), &next, "p", "cambio e"),
        ]);
        assert!(matches!(r.steps[1], Step::External { reason: ExternalReason::BetweenEdits }));
        let all: Vec<Vec<String>> = r.files[0].hunks.iter().map(|h| rationales(&r, h)).collect();
        assert_eq!(all, vec![vec!["cambio b".to_owned()], vec!["externo".to_owned()], vec!["cambio e".to_owned()]]);
        assert_eq!(r.external_hunks(), 1);
    }

    #[test]
    fn nearby_changes_merge_into_one_hunk_with_every_source() {
        let agent = BASE.replace("b\n", "B\n");
        let formatted = agent.replace("k\n", "k;\n");
        let next = formatted.replace("e\n", "E\n");
        let r = review(&[
            edit("x.ts", Some(BASE), &agent, "p", "cambio b"),
            edit("x.ts", Some(&formatted), &next, "p", "cambio e"),
        ]);
        let hunks = &r.files[0].hunks;
        assert_eq!(hunks.len(), 1);
        assert_eq!(rationales(&r, &hunks[0]), vec!["cambio b", "externo", "cambio e"]);
        assert_eq!(r.external_hunks(), 1, "basta un origen externo para señalar el bloque");
    }

    #[test]
    fn changes_after_the_last_edit_are_compared_with_the_current_file() {
        let base: String = (0..40).map(|n| format!("línea {n}\n")).collect::<Vec<_>>().concat();
        let agent = base.replace("línea 2\n", "LÍNEA 2\n");
        let disk = agent.replace("línea 30\n", "línea 30 // parche por terminal\n");
        let edits = [edit("x.ts", Some(&base), &agent, "p", "cambio 2")];
        let r = build("s", &edits, |e| e.path.display().to_string(), |_| Some(disk.clone()));
        assert!(matches!(r.steps.last(), Some(Step::External { reason: ExternalReason::AfterLastEdit })));
        assert_eq!(r.files[0].after, disk);
        let sources: Vec<Vec<String>> = r.files[0].hunks.iter().map(|h| rationales(&r, h)).collect();
        assert_eq!(sources, vec![vec!["cambio 2".to_owned()], vec!["externo".to_owned()]]);

        let unchanged = build("s", &edits, |e| e.path.display().to_string(), |_| Some(agent.clone()));
        assert_eq!(unchanged.external_hunks(), 0);
    }

    #[test]
    fn deletions_are_attributed_by_position() {
        let without = BASE.replace("f\ng\n", "");
        let r = review(&[edit("x.ts", Some(BASE), &without, "p", "borro f y g")]);
        let hunk = &r.files[0].hunks[0];
        assert_eq!((hunk.added(), hunk.removed()), (0, 2));
        assert_eq!(rationales(&r, hunk), vec!["borro f y g"]);
    }

    #[test]
    fn deletions_survive_later_edits_above_them() {
        let without = BASE.replace("j\n", "");
        let more = format!("cabecera\n{without}");
        let r = review(&[
            edit("x.ts", Some(BASE), &without, "p", "borro j"),
            edit("x.ts", Some(&without), &more, "p", "añado cabecera"),
        ]);
        let hunks = &r.files[0].hunks;
        assert_eq!(hunks.len(), 2);
        assert_eq!(rationales(&r, &hunks[1]), vec!["borro j"]);
    }

    #[test]
    fn created_files_and_untouched_files() {
        let r = review(&[
            edit("nuevo.ts", None, "export const x = 1;\n", "p", "creo el fichero"),
            edit("igual.ts", Some("a\n"), "b\n", "p", "cambio"),
            edit("igual.ts", Some("b\n"), "a\n", "p", "lo deshago"),
        ]);
        assert_eq!(r.files.len(), 1, "un fichero que vuelve a su estado inicial no se revisa");
        assert_eq!(r.files[0].status, FileStatus::Created);
        assert_eq!(rationales(&r, &r.files[0].hunks[0]), vec!["creo el fichero"]);
    }

    #[test]
    fn unreconstructable_edits_become_external_changes() {
        let middle = BASE.replace("c\n", "C\n");
        let end = middle.replace("i\n", "I\n");
        let mut lost = edit("x.ts", None, "", "p", "perdida");
        lost.after = AfterState::FragmentOnly("C".into());
        let r = review(&[
            edit("x.ts", Some(BASE), BASE, "p", "nada"),
            lost,
            edit("x.ts", Some(&middle), &end, "p", "cambio i"),
        ]);
        assert_eq!(r.unreconstructable, 1);
        assert!(r.steps.iter().any(|s| matches!(s, Step::External { reason: ExternalReason::Unreconstructable })));
    }

    #[test]
    fn positions_map_through_inserts_and_deletes() {
        let old = lines("a\nb\nc\n");
        let new = lines("x\na\nc\n");
        let diff = TextDiff::from_slices(&old, &new);
        assert_eq!(map_position(diff.ops(), 0), 1);
        assert_eq!(map_position(diff.ops(), 2), 2);
        assert_eq!(map_position(diff.ops(), 3), 3);
    }
}
