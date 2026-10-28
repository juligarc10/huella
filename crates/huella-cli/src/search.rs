//! Resultados de `huella search` (terminal y MCP).

use std::fmt::Write as _;
use std::path::Path;

use huella_store::SearchHit;

const MAX_LISTED: usize = 4;

/// Lista numerada de resultados en markdown.
pub fn render_hits(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return "Nada en la memoria coincide con esa búsqueda.".to_owned();
    }
    let mut out = String::new();
    for (number, hit) in (1..).zip(hits) {
        match hit {
            SearchHit::Request { session_id, at, prompt, snippet, paths, symbols, edits, .. } => {
                let _ = writeln!(
                    out,
                    "{number}. **Petición** · {} · sesión {} · {edits} {}",
                    at.as_deref().map_or_else(|| "sin fecha".to_owned(), short_date),
                    &session_id[..8.min(session_id.len())],
                    if *edits == 1 { "edición" } else { "ediciones" }
                );
                let request = prompt.as_deref().map(one_line).unwrap_or_default();
                let _ = writeln!(out, "   «{}»", clip(&request, 160));
                if !snippet.contains(&request.chars().take(40).collect::<String>()) {
                    let _ = writeln!(out, "   {}", one_line(snippet));
                }
                let files: Vec<&str> =
                    paths.iter().map(|p| Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p)).collect();
                let _ = writeln!(out, "   ficheros: {}", listed(&files));
                if !symbols.is_empty() {
                    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
                    let _ = writeln!(out, "   símbolos: {}", listed(&names));
                }
            }
            SearchHit::Decision { decision, snippet, .. } => {
                let _ = writeln!(
                    out,
                    "{number}. **Decisión** ({}) · {} · {}",
                    status_label(&decision.status),
                    decision.title,
                    decision.decided_at.as_deref().map_or_else(|| "sin fecha".to_owned(), short_date)
                );
                let _ = writeln!(out, "   {}", one_line(snippet));
                if let Some(project) = &decision.project {
                    let _ = writeln!(out, "   proyecto: {project}");
                }
            }
        }
    }
    out
}

fn listed(items: &[&str]) -> String {
    let shown = items.iter().take(MAX_LISTED).copied().collect::<Vec<_>>().join(", ");
    match items.len().saturating_sub(MAX_LISTED) {
        0 => shown,
        more => format!("{shown} (+{more})"),
    }
}

/// `2026-10-25T10:03:11.120Z` → `2026-10-25 10:03`.
fn short_date(timestamp: &str) -> String {
    timestamp.get(..16).unwrap_or(timestamp).replacen('T', " ", 1)
}

pub fn status_label(status: &str) -> &str {
    match status {
        "accepted" => "vigente",
        "proposed" => "propuesta",
        "rejected" => "descartada",
        "superseded" => "reemplazada",
        other => other,
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
