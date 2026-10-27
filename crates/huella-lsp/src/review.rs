//! Revisión guiada dentro del editor: cada paso pendiente de la revisión
//! activa es una diagnosis informativa en la primera línea de su función, y
//! una acción de código lo marca como revisado. Funciona en cualquier editor
//! con LSP, también en los que no permiten vistas propias (Zed).

use std::collections::BTreeMap;
use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_engine::narration::Narration;
use huella_review::attention::Attention;
use huella_review::flow::FlowStep;
use huella_review::{DiffLine, SessionReview, Step};
use huella_store::{ReviewWorkspace, Store};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use serde_json::json;

pub(crate) const MARK_REVIEWED: &str = "huella.markReviewed";
pub(crate) const SOURCE: &str = "huella";

pub(crate) struct ActiveReview {
    pub session: String,
    review: SessionReview,
    plan: Vec<FlowStep>,
    narrations: BTreeMap<usize, Narration>,
    /// Worktree de `huella review zed`, si la sesión tiene uno.
    workspace: Option<ReviewWorkspace>,
}

impl ActiveReview {
    /// Revisión de la sesión activa, o `None` si no la hay o no se puede cargar.
    pub fn load(store: &Store, projects: &Path) -> Option<Self> {
        let session = store.active_review().ok()??;
        let review = huella_engine::session_review(projects, &session)
            .inspect_err(|error| eprintln!("huella: no se pudo cargar la revisión {session}: {error}"))
            .ok()?;
        let plan = huella_review::flow::plan(&review);
        let points: Vec<Attention> = huella_review::attention::attention(&review, &plan);
        let narrations = huella_engine::narration::narrations(store, &review, &plan, &points).unwrap_or_default();
        let workspace = store.review_workspace(&session).ok().flatten();
        Some(Self { session, review, plan, narrations, workspace })
    }

    /// Pasos pendientes de un fichero como diagnosis, contra su contenido actual.
    pub fn diagnostics(&self, store: &Store, path: &str, source: &str) -> Vec<Diagnostic> {
        let marks = store.review_marks(&self.review.session_id).unwrap_or_default();
        let current = Language::from_path(Path::new(path))
            .and_then(|language| extract_symbols(language, source).ok())
            .map(|extraction| extraction.symbols)
            .unwrap_or_default();
        let total = self.plan.len();
        let session = &self.review.session_id[..8.min(self.review.session_id.len())];
        self.plan
            .iter()
            .enumerate()
            .filter(|(_, step)| self.review.files[step.file].path == path)
            .filter(|(_, step)| marks.get(&step.key(&self.review)) != Some(&step.content_hash()))
            .map(|(index, step)| {
                let line = step
                    .symbol
                    .as_ref()
                    .map_or(1, |s| current.iter().find(|c| c.qualified_name == s.qualified_name).map_or(s.start_line, |c| c.start_line));
                let unexplained = step.lines.iter().any(|line| {
                    matches!(line, DiffLine::Added { source: Some(s), .. } if matches!(self.review.steps[*s], Step::External { .. }))
                });
                let name = step.symbol.as_ref().map_or("(fuera de funciones)", |s| s.qualified_name.as_str());
                let message = format!(
                    "Huella · revisión {session} · paso {} de {total} pendiente: {name}\n{}",
                    index + 1,
                    self.explanation(index, step, unexplained)
                );
                let line = u32::try_from(line.saturating_sub(1)).unwrap_or(0);
                Diagnostic {
                    range: lsp_types::Range::new(lsp_types::Position::new(line, 0), lsp_types::Position::new(line, u32::MAX)),
                    // Aviso y no información: el panel de diagnósticos de Zed solo
                    // lista errores y avisos, y un paso pendiente es tarea por hacer.
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: Some(NumberOrString::String(format!("paso-{}", index + 1))),
                    source: Some(SOURCE.to_owned()),
                    message,
                    data: Some(json!({"key": step.key(&self.review), "hash": step.content_hash()})),
                    ..Diagnostic::default()
                }
            })
            .collect()
    }

    /// Ficheros que tienen pasos en la revisión. Con worktree de revisión, sus
    /// copias en él: es el proyecto abierto en el editor y así su panel de
    /// diagnósticos los lista.
    pub fn file_paths(&self) -> impl Iterator<Item = String> + '_ {
        self.review.files.iter().map(|file| match &self.workspace {
            Some(workspace) => match file.path.strip_prefix(&workspace.repo) {
                Some(relative) if relative.starts_with('/') => format!("{}{relative}", workspace.path),
                _ => file.path.clone(),
            },
            None => file.path.clone(),
        })
    }

    /// Marca como revisados los ficheros preparados (stage) en el worktree de
    /// revisión. Devuelve si marcó alguno.
    pub fn sync_workspace(&self, store: &Store) -> bool {
        let Some(workspace) = &self.workspace else { return false };
        huella_engine::workspace::sync_workspace_marks(store, &self.review, &self.plan, workspace)
            .inspect_err(|error| eprintln!("huella: no se pudo leer el worktree de revisión: {error}"))
            .is_ok_and(|marked| marked > 0)
    }

    /// Lo más útil para decidir: la nota del modelo, la explicación del agente
    /// o el aviso de que no hay explicación.
    fn explanation(&self, index: usize, step: &FlowStep, unexplained: bool) -> String {
        let note = step.turn.and_then(|turn| self.narrations.get(&turn)).and_then(|n| n.step_notes.get(&index));
        let rationale = step.sources.iter().rev().find_map(|&s| match &self.review.steps[s] {
            Step::Agent { rationale, .. } => rationale.as_deref(),
            Step::External { .. } => None,
        });
        let mut text = match (note, rationale) {
            (Some(note), _) => note.clone(),
            (None, Some(rationale)) => clip(rationale, 300),
            (None, None) => "Sin explicación del agente.".to_owned(),
        };
        if unexplained {
            text.push_str("\n⚠ Tiene líneas sin explicación (terminal o a mano).");
        }
        text
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat,
    }
}
