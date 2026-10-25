//! Muestra de anclajes en markdown para validarlos a mano.
//!
//! La tasa de anclaje mide cobertura; si el símbolo elegido es el correcto
//! solo lo puede juzgar una persona leyendo el cambio.

use std::fmt::Write as _;

use huella_anchoring::AnchorReport;
use huella_core::{AfterState, AnchorOutcome, FileEdit};

use crate::display_path;

/// Elige `size` anclajes repartidos uniformemente: determinista y sin sesgo
/// hacia una sola sesión. Con distintos `offset` salen muestras disjuntas, lo
/// que permite validar con una muestra que no se usó para corregir.
pub fn select(items: &[(FileEdit, AnchorReport)], size: usize, offset: usize) -> Vec<&(FileEdit, AnchorReport)> {
    let anchored: Vec<_> = items.iter().filter(|(_, report)| report.outcome.is_anchored()).collect();
    if size == 0 || anchored.is_empty() {
        return Vec::new();
    }
    let step = (anchored.len() / size).max(1);
    anchored.into_iter().skip(offset % step).step_by(step).take(size).collect()
}

pub fn render(
    sample: &[&(FileEdit, AnchorReport)],
    read_current: impl Fn(&std::path::Path) -> Option<String>,
) -> String {
    let mut out = String::from(
        "# Huella F0 · validación manual del anclaje\n\nMarca cada caso como correcto si los símbolos elegidos son los que de verdad cambió la edición.\n",
    );
    for (index, (edit, report)) in sample.iter().enumerate() {
        let _ = writeln!(out, "\n## {}. `{}`\n", index + 1, display_path(edit));
        let _ = writeln!(
            out,
            "Sesión `{}` · {} · {:?}",
            edit.origin.session_id,
            edit.origin.timestamp.as_deref().unwrap_or("sin fecha"),
            edit.kind
        );
        let _ = writeln!(out, "\n**Anclaje:** {}\n", describe(&report.outcome));
        let text = match &edit.after {
            AfterState::Known(text) => Some(text.clone()),
            AfterState::FragmentOnly(_) => read_current(&edit.path),
        };
        if let Some(text) = text {
            let _ = writeln!(out, "```\n{}```", excerpt(&text, &report.changed_lines, 12));
        }
        out.push_str("\n- [ ] Correcto\n");
    }
    out
}

fn describe(outcome: &AnchorOutcome) -> String {
    match outcome {
        AnchorOutcome::Symbols { symbols, also_module, confidence } => {
            let names: Vec<String> = symbols
                .iter()
                .map(|s| format!("`{}` (l. {}–{})", s.qualified_name, s.start_line, s.end_line))
                .collect();
            let module = if *also_module { " + nivel módulo" } else { "" };
            format!("{}{module} · {confidence:?}", names.join(", "))
        }
        AnchorOutcome::ModuleOnly { confidence } => format!("solo nivel módulo · {confidence:?}"),
        other => format!("{other:?}"),
    }
}

/// Líneas cambiadas con su número, hasta `max_lines`.
fn excerpt(text: &str, ranges: &[(usize, usize)], max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut shown = 0;
    for &(start, end) in ranges {
        for number in start..=end.min(lines.len()) {
            if shown == max_lines {
                let _ = writeln!(out, "…");
                return out;
            }
            let _ = writeln!(out, "{number:>5} │ {}", lines[number - 1]);
            shown += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_numbers_lines_and_truncates() {
        let text = "a\nb\nc\nd\n";
        assert_eq!(excerpt(text, &[(2, 3)], 10), "    2 │ b\n    3 │ c\n");
        assert_eq!(excerpt(text, &[(1, 4)], 2), "    1 │ a\n    2 │ b\n…\n");
        assert_eq!(excerpt(text, &[(4, 9)], 10), "    4 │ d\n");
    }
}
