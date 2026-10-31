//! Preguntar sobre cualquier trozo de código: «¿por qué está esto así?»,
//! «¿qué pasa si…?». La respuesta se apoya en la historia de ese código (qué
//! se pidió y qué explicó el agente que lo escribió, sus commits) y se guarda
//! anclada al texto seleccionado, para que siga junto a esas líneas aunque el
//! fichero cambie alrededor.

use std::fmt::Write as _;
use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_decisions::JsonModel;
use huella_store::{CodeQuestion, NewCodeQuestion, Store};
use serde_json::{Value, json};

use crate::paths::normalize_path;
use crate::query::memory_at;
use crate::render::render_markdown;
use crate::workspace::git;
use crate::{EngineError, Result};

const MAX_CHARS: usize = 60_000;
/// Líneas de código alrededor de la selección que se envían al modelo.
const AROUND: usize = 30;
const MAX_SELECTION_LINES: usize = 400;

const SYSTEM: &str = "\
Ayudas a una persona a entender código, muchas veces escrito por un agente de IA. Respondes a su pregunta sobre \
las líneas seleccionadas (marcadas con >) usando solo el contexto que se te da: el código, la historia de esas \
líneas (lo que se pidió y lo que explicó el agente que las escribió) y sus commits.

- Sé concreto: cita funciones, variables y líneas (L12).
- Distingue lo que consta en la historia (la petición, la explicación del agente, un commit) de lo que deduces \
  tú leyendo el código. Si es deducción, dilo.
- Si el contexto no basta para responder, dilo y explica qué habría que mirar.
- Si ves un problema real relacionado con la pregunta (un caso límite, un error), señálalo.
- Escribe en el idioma de la pregunta, en markdown breve.";

/// Respuesta a una pregunta sobre código, ya guardada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAnswer {
    pub id: i64,
    pub answer: String,
    pub model: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// Una pregunta guardada y dónde está ahora su código.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedQuestion {
    pub question: CodeQuestion,
    /// Líneas actuales del código preguntado; `None` si ya no está en el fichero.
    pub lines: Option<(usize, usize)>,
}

/// Líneas (1-based, inclusivas) donde está `quote` en `source`. Se compara sin
/// sangría ni espacios finales; si aparece varias veces, gana la más cercana a
/// `near`.
pub fn locate(source: &str, quote: &str, near: Option<usize>) -> Option<(usize, usize)> {
    let wanted: Vec<&str> = quote.lines().map(str::trim).collect();
    let first = wanted.iter().position(|line| !line.is_empty())?;
    let last = wanted.iter().rposition(|line| !line.is_empty())?;
    let wanted = &wanted[first..=last];
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    (0..lines.len().saturating_sub(wanted.len() - 1))
        .filter(|&start| lines[start..start + wanted.len()] == *wanted)
        .min_by_key(|&start| near.map_or(0, |near| (start + 1).abs_diff(near)))
        .map(|start| (start + 1, start + wanted.len()))
}

/// Pregunta sobre un trozo de `path`. Se identifica por su texto (`quote`) y,
/// si no se encuentra o viene vacío, por las líneas `lines`. La pregunta y la
/// respuesta se guardan.
///
/// # Errors
/// Si el fichero no se puede leer, el trozo no existe, o fallan el modelo o la
/// base de datos.
pub fn ask_code(
    store: &Store,
    model: &dyn JsonModel,
    path: &Path,
    quote: &str,
    lines: Option<(usize, usize)>,
    question: &str,
) -> Result<CodeAnswer> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| EngineError::Model(format!("no se pudo leer {}: {error}", path.display())))?;
    let total = source.lines().count();
    let (start, end) = locate(&source, quote, lines.map(|(start, _)| start))
        .or(lines.filter(|&(start, end)| start >= 1 && start <= end && end <= total))
        .ok_or_else(|| EngineError::Model("el código seleccionado ya no está en el fichero".into()))?;
    let end = end.min(start + MAX_SELECTION_LINES - 1);
    let key = normalize_path(path, None);
    let previous = questions_for(store, path)?;

    let context = context(store, path, &source, start, end, &previous)?;
    let request = huella_core::redact::redact(&format!("{context}\n## Pregunta\n\n{}", question.trim())).into_owned();
    let schema = json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
        "additionalProperties": false
    });
    let text = model.complete_json(SYSTEM, &request, &schema).map_err(|error| EngineError::Model(error.to_string()))?;
    let parsed: Value = serde_json::from_str(&text).map_err(|error| EngineError::Model(error.to_string()))?;
    let answer = parsed["answer"].as_str().unwrap_or_default().trim().to_owned();
    if answer.is_empty() {
        return Err(EngineError::Model("el modelo devolvió una respuesta vacía".into()));
    }
    let selected: Vec<&str> = source.lines().skip(start - 1).take(end - start + 1).collect();
    let model_id = model.model_id();
    let id = store.add_code_question(&NewCodeQuestion {
        path: &key,
        start_line: start,
        end_line: end,
        quote: &selected.join("\n"),
        question: question.trim(),
        answer: &answer,
        model: &model_id,
    })?;
    Ok(CodeAnswer { id, answer, model: model_id, start_line: start, end_line: end })
}

/// Preguntas guardadas sobre `path`, con su código vuelto a buscar en el
/// fichero actual.
///
/// # Errors
/// Si falla la base de datos.
pub fn questions_for(store: &Store, path: &Path) -> Result<Vec<LocatedQuestion>> {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    Ok(store
        .code_questions(&normalize_path(path, None))?
        .into_iter()
        .map(|question| {
            let lines = locate(&source, &question.quote, Some(question.start_line));
            LocatedQuestion { question, lines }
        })
        .collect())
}

/// Lo que ve el modelo: el código con la selección marcada, dónde está, su
/// historia en Huella, sus commits y lo que ya se preguntó de esas líneas.
fn context(
    store: &Store,
    path: &Path,
    source: &str,
    start: usize,
    end: usize,
    previous: &[LocatedQuestion],
) -> Result<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::new();
    let _ = writeln!(out, "# Código seleccionado: `{}`, líneas {start}–{end}\n", path.display());
    out.push_str("```\n");
    for number in start.saturating_sub(AROUND).max(1)..=(end + AROUND).min(lines.len()) {
        let mark = if (start..=end).contains(&number) { '>' } else { ' ' };
        let _ = writeln!(out, "{mark}{number:>5}  {}", lines[number - 1]);
    }
    out.push_str("```\n\n");

    let symbols = Language::from_path(path)
        .and_then(|language| extract_symbols(language, source).ok())
        .map(|extraction| extraction.symbols)
        .unwrap_or_default();
    let containing: Vec<&str> = symbols
        .iter()
        .filter(|s| s.start_line <= end && start <= s.end_line)
        .map(|s| s.qualified_name.as_str())
        .collect();
    if !containing.is_empty() {
        let _ = writeln!(out, "Símbolos: {}\n", containing.join(", "));
    }

    out.push_str("## Historia de estas líneas en Huella\n\n");
    let mut told = Vec::new();
    for line in [start, end] {
        if let Some(memory) = memory_at(store, path, source, line)?
            && !told.contains(&memory.symbol.qualified_name)
        {
            told.push(memory.symbol.qualified_name.clone());
            let _ = writeln!(out, "{}\n", render_markdown(&memory, 8));
        }
    }
    if told.is_empty() {
        out.push_str(
            "Huella no tiene historia de estas líneas: no las escribió ningún agente o no se han indexado.\n\n",
        );
    }

    let commits = blame(path, start, end);
    if !commits.is_empty() {
        let _ = writeln!(out, "## Commits de estas líneas\n\n{}\n", commits.join("\n"));
    }

    let overlapping: Vec<&LocatedQuestion> =
        previous.iter().filter(|q| q.lines.is_some_and(|(s, e)| s <= end && start <= e)).collect();
    if !overlapping.is_empty() {
        out.push_str("## Preguntas anteriores sobre estas líneas\n\n");
        for located in overlapping.iter().rev().take(3).rev() {
            let _ = writeln!(out, "P: {}\nR: {}\n", located.question.question, clip(&located.question.answer, 1500));
        }
    }
    Ok(clip(&out, MAX_CHARS))
}

/// Commits que dejaron las líneas como están: `abc1234 · autor · fecha · resumen`.
/// Sin repositorio o sin commits, nada.
fn blame(path: &Path, start: usize, end: usize) -> Vec<String> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    let range = format!("{start},{end}");
    let Ok(porcelain) = git(dir, &["blame", "--porcelain", "-L", &range, "--", name]) else { return Vec::new() };
    let mut commits: Vec<String> = Vec::new();
    let (mut sha, mut author, mut time) = (String::new(), String::new(), String::new());
    for line in porcelain.lines() {
        if let Some(rest) = line.strip_prefix("author ") {
            rest.clone_into(&mut author);
        } else if let Some(rest) = line.strip_prefix("author-time ") {
            rest.clone_into(&mut time);
        } else if let Some(summary) = line.strip_prefix("summary ") {
            let entry = if sha.chars().all(|c| c == '0') {
                "(cambios sin commit)".to_owned()
            } else {
                format!("{} · {author} · {} · {summary}", &sha[..7.min(sha.len())], date(&time))
            };
            if !commits.contains(&entry) {
                commits.push(entry);
            }
        } else if line.len() > 40 && line.as_bytes()[..40].iter().all(u8::is_ascii_hexdigit) {
            line[..40].clone_into(&mut sha);
        }
    }
    commits
}

/// `1730000000` → `2024-10-27`.
fn date(unix: &str) -> String {
    let Ok(seconds) = unix.parse::<i64>() else { return String::new() };
    let days = seconds.div_euclid(86_400);
    // Fecha civil desde días (algoritmo de Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use huella_decisions::ExtractError;

    use super::*;

    #[test]
    fn quotes_are_found_again_after_the_code_moves() {
        let source = "fn a() {}\n\nfn charge() {\n    let total = sum();\n    save(total)\n}\n";
        assert_eq!(locate(source, "    let total = sum();\n    save(total)", None), Some((4, 5)));
        // Sin sangría y con líneas en blanco alrededor, igual.
        assert_eq!(locate(source, "\nlet total = sum();\nsave(total)\n", None), Some((4, 5)));
        let moved = format!("// cabecera nueva\n\n{source}");
        assert_eq!(locate(&moved, "let total = sum();\nsave(total)", Some(4)), Some((6, 7)));
        assert_eq!(locate(source, "no está", None), None);
        assert_eq!(locate(source, "   \n", None), None);
    }

    #[test]
    fn repeated_snippets_resolve_to_the_nearest_copy() {
        let source = "x = 1\ny\nx = 1\ny\nx = 1\n";
        assert_eq!(locate(source, "x = 1", Some(5)), Some((5, 5)));
        assert_eq!(locate(source, "x = 1", Some(2)), Some((1, 1)));
    }

    #[test]
    fn dates_from_unix_seconds() {
        assert_eq!(date("0"), "1970-01-01");
        assert_eq!(date("1730000000"), "2024-10-27");
    }

    struct Fake {
        seen: RefCell<String>,
    }

    impl JsonModel for Fake {
        fn model_id(&self) -> String {
            "fake:1".into()
        }

        fn complete_json(&self, system: &str, user: &str, _: &Value) -> std::result::Result<String, ExtractError> {
            assert_eq!(system, SYSTEM);
            user.clone_into(&mut self.seen.borrow_mut());
            Ok(json!({"answer": "Suma y guarda el total (L4–L5)."}).to_string())
        }
    }

    #[test]
    fn answers_are_stored_with_the_selected_code_and_found_after_edits() {
        let dir = std::env::temp_dir().join(format!("huella-code-ask-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pago.rs");
        std::fs::write(&path, "fn a() {}\n\nfn charge() {\n    let total = sum();\n    save(total)\n}\n").unwrap();
        let store = Store::open_in_memory().unwrap();
        let model = Fake { seen: RefCell::new(String::new()) };

        let answer = ask_code(&store, &model, &path, "let total = sum();\nsave(total)", None, "¿Qué hace?").unwrap();
        assert_eq!((answer.start_line, answer.end_line), (4, 5));
        let seen = model.seen.borrow().clone();
        assert!(seen.contains(">    4      let total = sum();"), "{seen}");
        assert!(seen.contains("Símbolos: charge"), "{seen}");
        assert!(seen.contains("¿Qué hace?"));

        // El código se mueve dos líneas: la pregunta lo sigue encontrando.
        std::fs::write(&path, "// a\n// b\nfn a() {}\n\nfn charge() {\n    let total = sum();\n    save(total)\n}\n")
            .unwrap();
        let found = questions_for(&store, &path).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lines, Some((6, 7)));

        // Si desaparece, se sabe.
        std::fs::write(&path, "fn a() {}\n").unwrap();
        assert_eq!(questions_for(&store, &path).unwrap()[0].lines, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
