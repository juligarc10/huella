//! `huella review`: revisión en la terminal de lo que cambió una sesión.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::IsTerminal;

use huella_engine::SessionListing;
use huella_engine::narration::Narration;
use huella_review::attention::{Attention, AttentionKind};
use huella_review::flow::{FlowStep, StepRole};
use huella_review::{DiffLine, ExternalReason, FileStatus, Hunk, SessionReview, Step};

/// Colores ANSI solo si la salida es una terminal.
struct Style {
    on: bool,
}

impl Style {
    fn detect() -> Self {
        Self { on: std::io::stdout().is_terminal() }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.on { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_owned() }
    }
}

pub fn render_sessions(sessions: &[SessionListing]) -> String {
    let mut out = String::new();
    for listing in sessions {
        let summary = &listing.summary;
        let _ = writeln!(
            out,
            "{}  {}  {:<11}  {:>4} ediciones · {:>3} ficheros  {}",
            &summary.id[..8.min(summary.id.len())],
            summary.last_at.as_deref().map_or("sin fecha".to_owned(), short_date),
            summary.agent,
            summary.edits,
            summary.files,
            listing.project.as_deref().unwrap_or("(sin proyecto)")
        );
    }
    out
}

pub fn render_review(review: &SessionReview, file_filter: Option<&str>) -> String {
    let style = Style::detect();
    let mut out = String::new();
    let hunks: usize = review.files.iter().map(|file| file.hunks.len()).sum();
    let _ = writeln!(
        out,
        "{} · {} turnos · {} ficheros · {} bloques ({} con cambios externos sin explicación)",
        style.paint("1", &format!("Sesión {}", &review.session_id[..8.min(review.session_id.len())])),
        review.turns.len(),
        review.files.len(),
        hunks,
        review.external_hunks()
    );
    if review.unreconstructable > 0 {
        let _ = writeln!(
            out,
            "{} ediciones no se pudieron reconstruir; sus cambios cuentan como externos",
            review.unreconstructable
        );
    }
    let root = common_root(review.files.iter().map(|file| file.path.as_str()));
    for file in review.files.iter().filter(|file| file_filter.is_none_or(|filter| file.path.contains(filter))) {
        let status = match file.status {
            FileStatus::Created => "nuevo",
            FileStatus::Modified => "modificado",
        };
        let name = file.path.strip_prefix(&root).unwrap_or(&file.path);
        let _ =
            writeln!(out, "\n{} ({status} · {} bloques)", style.paint("1;36", &format!("━━ {name}")), file.hunks.len());
        for hunk in &file.hunks {
            render_hunk(&mut out, &style, review, hunk);
        }
    }
    out
}

fn render_hunk(out: &mut String, style: &Style, review: &SessionReview, hunk: &Hunk) {
    let _ = writeln!(
        out,
        "\n{}",
        style.paint(
            "35",
            &format!("@@ -{} +{} @@  +{} −{}", hunk.old_start, hunk.new_start, hunk.added(), hunk.removed())
        )
    );
    render_sources(out, style, review, &hunk.sources, "  ", None);
    render_lines(out, style, review, &hunk.lines, "");
}

/// Turno, explicación del agente o aviso de cambio externo de cada origen.
/// `under_turn`: el turno cuya cabecera ya se mostró; sus peticiones no se repiten.
fn render_sources(
    out: &mut String,
    style: &Style,
    review: &SessionReview,
    sources: &[usize],
    indent: &str,
    under_turn: Option<usize>,
) {
    let mut seen_turns: BTreeSet<usize> = under_turn.into_iter().collect();
    for &source in sources {
        match &review.steps[source] {
            Step::Agent { turn, at, rationale, .. } => {
                if seen_turns.insert(*turn) {
                    let prompt = review.turns.get(*turn).and_then(|t| t.prompt.as_deref()).map(|p| clip(p, 110));
                    let when = at.as_deref().map(short_date).unwrap_or_default();
                    let _ = writeln!(
                        out,
                        "{indent}{} {when} · «{}»",
                        style.paint("1;34", &format!("turno {}", turn + 1)),
                        prompt.unwrap_or_default()
                    );
                }
                if let Some(rationale) = rationale {
                    let _ = writeln!(out, "{indent}  ▸ {}", clip(rationale, 220));
                }
            }
            Step::External { reason } => {
                let text = match reason {
                    ExternalReason::BetweenEdits => {
                        "cambio externo entre ediciones (terminal, formateador o a mano): sin explicación"
                    }
                    ExternalReason::Unreconstructable => "edición no reconstruible: revisa este bloque sin explicación",
                    ExternalReason::AfterLastEdit => {
                        "cambio posterior a la última edición del agente (terminal o a mano): sin explicación"
                    }
                };
                let _ = writeln!(out, "{indent}{}", style.paint("1;33", &format!("⚠ {text} (líneas marcadas con !)")));
            }
        }
    }
}

fn render_lines(out: &mut String, style: &Style, review: &SessionReview, lines: &[DiffLine], indent: &str) {
    for line in lines {
        let rendered = match line {
            DiffLine::Context(text) => format!("  {text}"),
            // Las líneas sin explicación se marcan aparte dentro del bloque.
            DiffLine::Added { text, source } => match source.map(|s| &review.steps[s]) {
                Some(Step::External { .. }) => style.paint("33", &format!("! {text}")),
                _ => style.paint("32", &format!("+ {text}")),
            },
            DiffLine::Removed(text) => style.paint("31", &format!("- {text}")),
        };
        let _ = writeln!(out, "{indent}{rendered}");
    }
}

/// Revisión en el orden del flujo: puntos de entrada, lo que van llamando,
/// tests y otros cambios. Cada línea del diff aparece en un solo paso.
pub fn render_flow(
    review: &SessionReview,
    plan: &[FlowStep],
    points: &[Attention],
    narrations: &BTreeMap<usize, Narration>,
    file_filter: Option<&str>,
) -> String {
    let style = Style::detect();
    let mut out = String::new();
    let unexplained = plan
        .iter()
        .filter(|step| step.sources.iter().any(|&s| matches!(review.steps[s], Step::External { .. })))
        .count();
    let _ = writeln!(
        out,
        "{} · {} turnos · {} ficheros · {} pasos ({} con cambios sin explicación)",
        style.paint("1", &format!("Sesión {}", &review.session_id[..8.min(review.session_id.len())])),
        review.turns.len(),
        review.files.len(),
        plan.len(),
        unexplained
    );
    let root = common_root(review.files.iter().map(|file| file.path.as_str()));
    let mut section = None;
    for (number, step) in plan.iter().enumerate() {
        let path = &review.files[step.file].path;
        if file_filter.is_some_and(|filter| !path.contains(filter)) {
            continue;
        }
        if section.is_none_or(|(turn, _)| turn != step.turn) {
            let header = match step.turn.and_then(|turn| review.turns.get(turn)) {
                Some(turn) => format!(
                    "━━ Turno {} · {} · «{}»",
                    turn.index + 1,
                    turn.started_at.as_deref().map(short_date).unwrap_or_default(),
                    turn.prompt.as_deref().map(|p| clip(p, 120)).unwrap_or_default()
                ),
                None => "━━ Cambios sin explicación (terminal, formateadores o a mano)".to_owned(),
            };
            let _ = writeln!(out, "\n{}", style.paint("1;36", &header));
            if let Some(narration) = step.turn.and_then(|turn| narrations.get(&turn)) {
                let outdated = if narration.stale { ", desfasado: el turno ha cambiado" } else { "" };
                let _ = writeln!(out, "   💬 {}", clip(&narration.summary, 600));
                let _ =
                    writeln!(out, "      {}", style.paint("2", &format!("generado por {}{outdated}", narration.model)));
                for (_, risk) in narration.risks.iter().filter(|(step, _)| step.is_none()) {
                    let _ = writeln!(out, "   {}", style.paint("1;33", &format!("⚑ {risk} (modelo)")));
                }
            }
            for point in points.iter().filter(|p| p.turn == step.turn && p.step.is_none()) {
                let _ = writeln!(out, "   {}", style.paint("1;33", &format!("⚑ {}", point.message)));
            }
        }
        if section != Some((step.turn, step.role)) {
            section = Some((step.turn, step.role));
            let title = match step.role {
                StepRole::Flow => "Flujo",
                StepRole::Test => "Tests",
                StepRole::Other => "Otros cambios",
            };
            let _ = writeln!(out, "{}", style.paint("2", &format!("   {title}")));
        }
        let indent = "  ".repeat(step.depth.min(6));
        let name = path.strip_prefix(&root).unwrap_or(path);
        let (label, location) = match &step.symbol {
            Some(symbol) => (symbol.qualified_name.clone(), format!("{name}:{}", symbol.start_line)),
            None => ("(fuera de funciones)".to_owned(), name.to_owned()),
        };
        let _ = writeln!(
            out,
            "\n{indent}{} {}  {}  +{} −{}",
            style.paint("1", &format!("{}.", number + 1)),
            style.paint("1", &label),
            style.paint("2", &location),
            step.added(),
            step.removed()
        );
        for point in points.iter().filter(|p| p.step == Some(number) && p.kind != AttentionKind::Unexplained) {
            let _ = writeln!(out, "{indent}   {}", style.paint("33", &format!("⚑ {}", point.message)));
        }
        if let Some(narration) = step.turn.and_then(|turn| narrations.get(&turn)) {
            if let Some(note) = narration.step_notes.get(&number) {
                let _ = writeln!(out, "{indent}   💬 {}", clip(note, 300));
            }
            for (_, risk) in narration.risks.iter().filter(|(step, _)| *step == Some(number)) {
                let _ = writeln!(out, "{indent}   {}", style.paint("33", &format!("⚑ {risk} (modelo)")));
            }
        }
        render_sources(&mut out, &style, review, &step.sources, &format!("{indent}   "), step.turn);
        render_lines(&mut out, &style, review, &step.lines, &format!("{indent}   "));
    }
    out
}

/// Prefijo de directorio común, para mostrar rutas cortas.
pub(crate) fn common_root<'a>(paths: impl Iterator<Item = &'a str>) -> String {
    let mut root: Option<String> = None;
    for path in paths {
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        root = Some(match root {
            None => dir.to_owned(),
            Some(current) => {
                let shared: Vec<&str> =
                    current.split('/').zip(dir.split('/')).take_while(|(a, b)| a == b).map(|(a, _)| a).collect();
                shared.join("/")
            }
        });
    }
    root.map(|root| if root.is_empty() { root } else { format!("{root}/") }).unwrap_or_default()
}

fn short_date(timestamp: &str) -> String {
    match (timestamp.get(..10), timestamp.get(11..16)) {
        (Some(day), Some(time)) => format!("{day} {time}"),
        _ => timestamp.to_owned(),
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_root_is_the_shared_directory() {
        assert_eq!(common_root(["/r/crates/a/src/x.rs", "/r/crates/b/y.rs"].into_iter()), "/r/crates/");
        assert_eq!(common_root(["/r/x.rs"].into_iter()), "/r/");
        assert_eq!(common_root(std::iter::empty()), "");
    }

    #[test]
    fn clip_flattens_and_cuts_on_char_boundaries() {
        assert_eq!(clip("ñ ñ\nñ", 3), "ñ ñ…");
        assert_eq!(clip("hola", 10), "hola");
    }
}
