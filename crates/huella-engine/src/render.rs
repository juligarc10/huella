//! Presentación de la memoria de un símbolo en markdown (hover y terminal).

use std::collections::HashSet;
use std::fmt::Write as _;

use huella_store::SymbolRole;

use crate::query::SymbolMemory;

const PROMPT_CHARS: usize = 220;
const RATIONALE_CHARS: usize = 450;

/// Markdown con las `max_entries` ediciones más recientes del símbolo.
pub fn render_markdown(memory: &SymbolMemory, max_entries: usize) -> String {
    let sessions: HashSet<&str> = memory.entries.iter().map(|e| e.session_id.as_str()).collect();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "**Huella** · `{}` · {} {} en {} {}",
        memory.symbol.qualified_name,
        memory.entries.len(),
        plural(memory.entries.len(), "edición", "ediciones"),
        sessions.len(),
        plural(sessions.len(), "sesión", "sesiones"),
    );
    if memory.changed_since_last_session {
        out.push_str("\n⚠️ El código ha cambiado desde la última sesión que lo tocó.\n");
    }
    for entry in memory.entries.iter().take(max_entries) {
        let when = entry.at.as_deref().map_or_else(|| "sin fecha".to_owned(), short_date);
        let removed = if entry.role == SymbolRole::Removed { " · borrado o renombrado" } else { "" };
        let _ = write!(out, "\n**{when}**{removed}");
        if let Some(prompt) = &entry.context.prompt {
            let _ = write!(out, " — _«{}»_", escape(&clip(&one_line(prompt), PROMPT_CHARS)));
        }
        out.push('\n');
        if let Some(rationale) = &entry.context.rationale {
            for line in clip(rationale, RATIONALE_CHARS).lines() {
                let _ = writeln!(out, "> {line}");
            }
        }
    }
    let hidden = memory.entries.len().saturating_sub(max_entries);
    if hidden > 0 {
        let _ = writeln!(out, "\n…y {hidden} {} más.", plural(hidden, "edición", "ediciones"));
    }
    out
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

/// `2026-10-25T10:03:11.120Z` → `2026-10-25 10:03`.
fn short_date(timestamp: &str) -> String {
    match (timestamp.get(..10), timestamp.get(11..16)) {
        (Some(day), Some(time)) => format!("{day} {time}"),
        _ => timestamp.to_owned(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
        None => text.to_owned(),
    }
}

/// La petición va en cursiva: sus `_` y `*` no deben romper el formato.
fn escape(text: &str) -> String {
    text.replace('_', "\\_").replace('*', "\\*")
}

#[cfg(test)]
mod tests {
    use huella_core::{EditContext, Symbol, SymbolKind};
    use huella_store::HistoryEntry;

    use super::*;

    fn entry(session: &str, at: &str, prompt: &str, rationale: Option<&str>, role: SymbolRole) -> HistoryEntry {
        HistoryEntry {
            edit_id: format!("{session}-{at}"),
            session_id: session.into(),
            at: Some(at.into()),
            role,
            kind: "replace".into(),
            confidence: Some("exact".into()),
            body_hash: "h".into(),
            context: EditContext { prompt: Some(prompt.into()), rationale: rationale.map(str::to_owned) },
        }
    }

    fn memory(entries: Vec<HistoryEntry>, changed: bool) -> SymbolMemory {
        SymbolMemory {
            symbol: Symbol {
                qualified_name: "UserService.register".into(),
                kind: SymbolKind::Method,
                start_line: 2,
                end_line: 4,
                body_hash: "h".into(),
                byte_range: 0..1,
                depth: 1,
            },
            entries,
            changed_since_last_session: changed,
        }
    }

    #[test]
    fn renders_header_entries_and_drift_warning() {
        let text = render_markdown(
            &memory(
                vec![
                    entry(
                        "s2",
                        "2026-10-25T10:03:11.120Z",
                        "recorta *el* email",
                        Some("Uso trim.\nY tests."),
                        SymbolRole::Touched,
                    ),
                    entry("s1", "2026-10-24T09:00:00Z", "crea el\nservicio", None, SymbolRole::Touched),
                ],
                true,
            ),
            5,
        );
        assert!(text.starts_with("**Huella** · `UserService.register` · 2 ediciones en 2 sesiones\n"));
        assert!(text.contains("⚠️ El código ha cambiado"));
        assert!(text.contains("**2026-10-25 10:03** — _«recorta \\*el\\* email»_\n> Uso trim.\n> Y tests.\n"));
        assert!(text.contains("_«crea el servicio»_"));
    }

    #[test]
    fn limits_entries_and_marks_removals() {
        let entries = vec![
            entry("s1", "2026-10-25T10:00:00Z", "a", None, SymbolRole::Removed),
            entry("s1", "2026-10-24T10:00:00Z", "b", None, SymbolRole::Touched),
            entry("s1", "2026-10-23T10:00:00Z", "c", None, SymbolRole::Touched),
        ];
        let text = render_markdown(&memory(entries, false), 2);
        assert!(text.contains("3 ediciones en 1 sesión"));
        assert!(text.contains("borrado o renombrado"));
        assert!(!text.contains("«c»"));
        assert!(text.contains("…y 1 edición más."));
    }

    #[test]
    fn clips_on_char_boundaries() {
        assert_eq!(clip("ñññññ", 3), "ñññ…");
        assert_eq!(clip("abc", 3), "abc");
    }
}
