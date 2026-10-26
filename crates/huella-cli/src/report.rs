//! Agregación de resultados de anclaje y métricas de la F0.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use huella_anchoring::AnchorReport;
use huella_claude_code::ParseStats;
use huella_core::{AnchorOutcome, Confidence, EditKind, FileEdit};
use serde::Serialize;

/// Criterio de salida de la F0: porcentaje de ediciones de código ancladas.
pub const F0_TARGET: f64 = 0.90;

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub transcripts: usize,
    pub lines: usize,
    pub invalid_json: usize,
    pub unknown_edit_shapes: usize,
    pub edits: usize,
    pub created: usize,
    pub rewritten: usize,
    pub replaced: usize,
    pub user_modified: usize,
    /// Ediciones de ficheros sin extracción de símbolos (markdown, json, css…).
    pub file_only: usize,
    pub no_change: usize,
    pub symbols_exact: usize,
    pub symbols_approximate: usize,
    pub module_only_exact: usize,
    pub module_only_approximate: usize,
    /// Símbolos que dejaron de existir (borrados o renombrados).
    pub removed_symbols: usize,
    pub with_syntax_errors: usize,
    pub unresolved: BTreeMap<String, usize>,
    pub by_language: BTreeMap<&'static str, LanguageStats>,
}

#[derive(Debug, Default, Serialize)]
pub struct LanguageStats {
    pub edits: usize,
    pub anchored: usize,
}

impl Summary {
    pub fn add_parse_stats(&mut self, stats: &ParseStats) {
        self.transcripts += 1;
        self.lines += stats.lines;
        self.invalid_json += stats.invalid_json;
        self.unknown_edit_shapes += stats.unknown_edit_shapes;
    }

    pub fn add(&mut self, edit: &FileEdit, report: &AnchorReport) {
        self.edits += 1;
        match edit.kind {
            EditKind::Create => self.created += 1,
            EditKind::Rewrite => self.rewritten += 1,
            EditKind::Replace => self.replaced += 1,
        }
        self.user_modified += usize::from(edit.user_modified);
        self.with_syntax_errors += usize::from(report.has_syntax_errors);
        if let Some(language) = report.language {
            let stats = self.by_language.entry(language.name()).or_default();
            stats.edits += 1;
            stats.anchored += usize::from(report.outcome.is_anchored());
        }
        match &report.outcome {
            AnchorOutcome::Anchored(anchor) => {
                self.removed_symbols += anchor.removed.len();
                let counter = match (anchor.is_module_only(), anchor.confidence) {
                    (false, Confidence::Exact) => &mut self.symbols_exact,
                    (false, Confidence::Approximate) => &mut self.symbols_approximate,
                    (true, Confidence::Exact) => &mut self.module_only_exact,
                    (true, Confidence::Approximate) => &mut self.module_only_approximate,
                };
                *counter += 1;
            }
            AnchorOutcome::FileOnly => self.file_only += 1,
            AnchorOutcome::NoChange => self.no_change += 1,
            AnchorOutcome::Unresolved { reason } => *self.unresolved.entry(reason.clone()).or_default() += 1,
        }
    }

    pub fn anchored(&self) -> usize {
        self.symbols_exact + self.symbols_approximate + self.module_only_exact + self.module_only_approximate
    }

    pub fn unresolved_total(&self) -> usize {
        self.unresolved.values().sum()
    }

    /// Ediciones de código que deberían anclarse: lenguaje soportado y con cambios.
    pub fn anchorable(&self) -> usize {
        self.anchored() + self.unresolved_total()
    }

    pub fn anchoring_rate(&self) -> Option<f64> {
        ratio(self.anchored(), self.anchorable())
    }

    /// Tasa contando solo los anclajes exactos (sin recurrir al disco).
    pub fn exact_rate(&self) -> Option<f64> {
        ratio(self.symbols_exact + self.module_only_exact, self.anchorable())
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let pct = |rate: Option<f64>| rate.map_or_else(|| "n/d".to_owned(), |r| format!("{:.1} %", r * 100.0));
        let _ = writeln!(
            out,
            "Transcripciones: {} · líneas: {} (JSON inválido: {})",
            self.transcripts, self.lines, self.invalid_json
        );
        let _ = writeln!(
            out,
            "Ediciones: {} (nuevas {}, reescritas {}, reemplazos {}; modificadas por el usuario {})",
            self.edits, self.created, self.rewritten, self.replaced, self.user_modified
        );
        if self.unknown_edit_shapes > 0 {
            let _ = writeln!(out, "Resultados de edición con forma desconocida: {}", self.unknown_edit_shapes);
        }
        let _ = writeln!(out, "\nSin símbolos (markdown, json, css…), anclaje a fichero: {}", self.file_only);
        let _ = writeln!(out, "Código sin cambios reales: {}", self.no_change);
        let _ = writeln!(out, "Código a anclar: {}", self.anchorable());
        let _ = writeln!(
            out,
            "  a símbolos:        {} exactas + {} aproximadas",
            self.symbols_exact, self.symbols_approximate
        );
        let _ = writeln!(
            out,
            "  solo nivel módulo: {} exactas + {} aproximadas",
            self.module_only_exact, self.module_only_approximate
        );
        let _ = writeln!(out, "  símbolos borrados o renombrados: {}", self.removed_symbols);
        let _ = writeln!(out, "  sin resolver:      {}", self.unresolved_total());
        for (reason, count) in &self.unresolved {
            let _ = writeln!(out, "    · {reason}: {count}");
        }
        let _ = writeln!(out, "  con errores de sintaxis (anclados igualmente): {}", self.with_syntax_errors);
        let _ = writeln!(out, "\nPor lenguaje:");
        for (language, stats) in &self.by_language {
            let _ = writeln!(
                out,
                "  {language:<11} {:>4} ediciones · {} ancladas",
                stats.edits,
                pct(ratio(stats.anchored, stats.edits))
            );
        }
        let verdict = match self.anchoring_rate() {
            Some(rate) if rate >= F0_TARGET => "cumple",
            Some(_) => "NO cumple",
            None => "sin datos",
        };
        let _ = writeln!(
            out,
            "\nTasa de anclaje: {} (solo exactas: {}) · criterio F0 > {:.0} %: {verdict}",
            pct(self.anchoring_rate()),
            pct(self.exact_rate()),
            F0_TARGET * 100.0
        );
        out
    }
}

#[allow(clippy::cast_precision_loss, reason = "recuentos muy por debajo de 2^52")]
fn ratio(part: usize, total: usize) -> Option<f64> {
    (total > 0).then(|| part as f64 / total as f64)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use huella_anchoring::anchor_edit;
    use huella_core::{AfterState, EditOrigin};

    use super::*;

    fn edit(path: &str, before: Option<&str>, after: AfterState) -> FileEdit {
        FileEdit {
            origin: EditOrigin {
                agent: "test",
                session_id: "s".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from(path),
            kind: if before.is_some() { EditKind::Replace } else { EditKind::Create },
            before: before.map(str::to_owned),
            after,
            user_modified: false,
        }
    }

    #[test]
    fn rate_counts_only_code_that_should_anchor() {
        let edits = [
            edit("a.ts", Some("let a = 1;\n"), AfterState::Known("let a = 2;\n".into())),
            edit("b.ts", None, AfterState::Known("import x from 'x';\n".into())),
            edit("c.ts", Some("let c = 1;\n"), AfterState::Known("let c = 1;\n".into())),
            edit("README.md", None, AfterState::Known("# Hola\n".into())),
            edit("d.ts", None, AfterState::FragmentOnly("gone()".into())),
        ];
        let mut summary = Summary::default();
        for edit in &edits {
            summary.add(edit, &anchor_edit(edit, |_| None));
        }
        assert_eq!((summary.symbols_exact, summary.module_only_exact), (1, 1));
        assert_eq!((summary.no_change, summary.file_only, summary.unresolved_total()), (1, 1, 1));
        assert_eq!(summary.anchorable(), 3);
        let rate = summary.anchoring_rate().unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 1e-9);
        assert!(summary.render().contains("NO cumple"));
        assert_eq!(summary.by_language["typescript"].edits, 4);
    }

    #[test]
    fn empty_summary_has_no_rate() {
        assert_eq!(Summary::default().anchoring_rate(), None);
        assert!(Summary::default().render().contains("sin datos"));
    }
}
