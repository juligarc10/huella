//! Preguntar sobre cualquier trozo de código: «¿por qué está esto así?»,
//! «¿qué pasa si…?». La respuesta se apoya en la historia de ese código (qué
//! se pidió y qué explicó el agente que lo escribió, sus commits) y se guarda
//! anclada al texto seleccionado, para que siga junto a esas líneas aunque el
//! fichero cambie alrededor.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use huella_anchoring::{Language, extract_symbols};
use huella_decisions::JsonModel;
use huella_store::{CodeQuestion, NewCodeQuestion, Store};
use serde_json::{Value, json};

use crate::paths::normalize_path;
use crate::query::{SymbolMemory, memory_at};
use crate::workspace::git;
use crate::{EngineError, Result};

const MAX_CHARS: usize = 60_000;
/// Líneas de código alrededor de la selección que se envían al modelo.
const AROUND: usize = 30;
const MAX_SELECTION_LINES: usize = 400;
/// Ediciones de la historia de un símbolo que se envían al modelo.
const MAX_HISTORY: usize = 8;
/// De las más recientes, cuántas llevan además lo que pensó el agente.
const WITH_REASONING: usize = 3;

const SYSTEM: &str = "\
Ayudas a una persona a entender código, muchas veces escrito por un agente de IA. Respondes a su pregunta sobre \
las líneas seleccionadas (marcadas con >) usando solo el contexto que se te da: el código, la historia de esas \
líneas (lo que se pidió y lo que explicó el agente que las escribió) y sus commits.

- Sé concreto: cita funciones, variables y líneas (L12).
- Distingue lo que consta en la historia (la petición, la explicación del agente, un commit) de lo que deduces \
  tú leyendo el código. Si es deducción, dilo.
- Si el contexto no basta para responder, dilo y explica qué habría que mirar.
- Si ves un problema real relacionado con la pregunta (un caso límite, un error), señálalo.
- Si se pregunta por líneas quitadas, explica qué hacían, qué las sustituye en el código actual (o si nada) y \
  qué dice la historia sobre quitarlas: la petición, la explicación del agente o el commit que las quitó.
- Responde y termina: no ofrezcas hacer cambios ni acabes con preguntas, desde aquí no se puede editar.
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

/// Como [`locate`], para una selección que salta líneas (un diff esconde los
/// métodos de una clase): de su primera línea a la última que viene después.
fn locate_span(source: &str, quote: &str, near: Option<usize>) -> Option<(usize, usize)> {
    let wanted: Vec<&str> = quote.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let (first, last) = (*wanted.first()?, *wanted.last()?);
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    (0..lines.len())
        .filter(|&start| lines[start] == first)
        .filter_map(|start| {
            let end = (start..lines.len()).find(|&end| lines[end] == last)?;
            Some((start + 1, end + 1))
        })
        .min_by_key(|&(start, _)| near.map_or(0, |near| start.abs_diff(near)))
}

/// Una pregunta sobre un trozo de `path`. Se identifica por su texto (`quote`) y,
/// si no se encuentra o viene vacío, por las líneas `lines`.
#[derive(Debug, Clone, Copy)]
pub struct CodeAsk<'a> {
    pub path: &'a Path,
    pub quote: &'a str,
    pub lines: Option<(usize, usize)>,
    pub question: &'a str,
    /// El texto revisado, que puede no ser el del disco (otra rama en la copia de
    /// trabajo). Sin él se lee el fichero.
    pub reviewed: Option<&'a str>,
    /// Líneas quitadas sobre las que también se pregunta: ya no están en el
    /// fichero, así que `quote` es el código junto al que estaban.
    pub removed: Option<Removed<'a>>,
}

/// Lo quitado sobre lo que se pregunta y dónde estaba respecto a `quote`.
#[derive(Debug, Clone, Copy)]
pub struct Removed<'a> {
    pub text: &'a str,
    pub place: RemovedPlace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovedPlace {
    /// Entre las líneas seleccionadas.
    Within,
    /// Justo antes de las líneas marcadas.
    Before,
    /// Justo después.
    After,
}

impl RemovedPlace {
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "before" => Self::Before,
            "after" => Self::After,
            _ => Self::Within,
        }
    }
}

/// Responde una pregunta sobre código y la guarda con su respuesta.
///
/// # Errors
/// Si el fichero no se puede leer, el trozo no existe, o fallan el modelo o la
/// base de datos.
pub fn ask_code(
    store: &Store,
    sources: Option<&crate::Sources>,
    model: &dyn JsonModel,
    ask: CodeAsk<'_>,
) -> Result<CodeAnswer> {
    let CodeAsk { path, quote, lines, question, reviewed, removed } = ask;
    let removed = removed.filter(|removed| !removed.text.trim().is_empty());
    let source = match reviewed {
        Some(reviewed) => reviewed.to_owned(),
        None => std::fs::read_to_string(path)
            .map_err(|error| EngineError::Model(format!("no se pudo leer {}: {error}", path.display())))?,
    };
    let total = source.lines().count();
    let near = lines.map(|(start, _)| start);
    let (start, end) = locate(&source, quote, near)
        .or_else(|| locate_span(&source, quote, near))
        .or(lines.filter(|&(start, end)| start >= 1 && start <= end && end <= total))
        .ok_or_else(|| EngineError::Model("el código seleccionado ya no está en el fichero".into()))?;
    let end = end.min(start + MAX_SELECTION_LINES - 1);
    let key = normalize_path(path, None);
    let previous = questions_in(store, path, &source)?;

    let mut context = context(store, sources, path, &source, start, end, &previous)?;
    if let Some(removed) = removed {
        context = format!("{}{context}", removed_context(path, removed));
    }
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
        removed: removed.map_or("", |removed| removed.text),
    })?;
    Ok(CodeAnswer { id, answer, model: model_id, start_line: start, end_line: end })
}

/// Preguntas guardadas sobre `path`, con su código vuelto a buscar en el
/// fichero actual.
///
/// # Errors
/// Si falla la base de datos.
pub fn questions_for(store: &Store, path: &Path) -> Result<Vec<LocatedQuestion>> {
    questions_in(store, path, &std::fs::read_to_string(path).unwrap_or_default())
}

/// Como [`questions_for`], pero sobre `source`: el texto que tiene el editor,
/// aunque no esté guardado.
///
/// # Errors
/// Si falla la base de datos.
pub fn questions_in(store: &Store, path: &Path, source: &str) -> Result<Vec<LocatedQuestion>> {
    Ok(store
        .code_questions(&normalize_path(path, None))?
        .into_iter()
        .map(|question| {
            let lines = locate(source, &question.quote, Some(question.start_line));
            LocatedQuestion { question, lines }
        })
        .collect())
}

/// Lo que ve el modelo: el código con la selección marcada, dónde está, su
/// historia en Huella, sus commits y lo que ya se preguntó de esas líneas.
fn context(
    store: &Store,
    sources: Option<&crate::Sources>,
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
        let _ = writeln!(out, "{mark}{number:>5}  {}", crate::storyline::clip_line(lines[number - 1]));
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
    let mut activity: HashMap<String, Vec<huella_core::Activity>> = HashMap::new();
    for line in [start, end] {
        if let Some(memory) = memory_at(store, path, source, line)?
            && !told.contains(&memory.symbol.qualified_name)
        {
            told.push(memory.symbol.qualified_name.clone());
            write_history(&mut out, &memory, sources, &mut activity)?;
        }
    }
    if told.is_empty() {
        out.push_str(
            "Huella no tiene historia de estas líneas: no las escribió ningún agente o no se han indexado.\n\n",
        );
    }

    // Lo que declaró el agente sobre los símbolos de la selección, con si sigue vigente.
    let normalized = crate::normalize_path(&std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()), None);
    let current: Vec<&str> = source.lines().collect();
    let declared: Vec<crate::declared::DeclaredView> = store
        .declarations(&normalized)?
        .into_iter()
        .filter(|declaration| {
            declaration.symbol.as_deref().map_or_else(
                || declaration.start_line <= end && start <= declaration.end_line,
                |symbol| containing.contains(&symbol),
            )
        })
        .take(5)
        .map(|declaration| {
            let now = symbols
                .iter()
                .find(|symbol| declaration.symbol.as_deref() == Some(symbol.qualified_name.as_str()))
                .map_or((declaration.start_line, declaration.end_line), |symbol| (symbol.start_line, symbol.end_line));
            let text = current.get(now.0.saturating_sub(1)..now.1.min(current.len())).map(|lines| lines.join("\n"));
            crate::declared::view(declaration, &text.unwrap_or_default())
        })
        .collect();
    if !declared.is_empty() {
        let _ =
            writeln!(out, "## Lo que declaró el agente sobre este código\n\n{}", crate::declared::render(&declared));
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

/// Lo quitado, dónde estaba y los commits que lo quitaron: va antes del resto
/// porque es de lo que trata la pregunta.
fn removed_context(path: &Path, removed: Removed<'_>) -> String {
    let mut out = String::from("# Líneas quitadas por las que se pregunta\n\n");
    out.push_str(match removed.place {
        RemovedPlace::Within => "Estaban entre las líneas marcadas con > más abajo y ya no están en el fichero.\n\n",
        RemovedPlace::Before => {
            "Estaban justo antes de la línea marcada con > más abajo y ya no están en el fichero.\n\n"
        }
        RemovedPlace::After => {
            "Estaban justo después de la línea marcada con > más abajo y ya no están en el fichero.\n\n"
        }
    });
    out.push_str("```\n");
    for line in removed.text.lines().take(MAX_SELECTION_LINES) {
        let _ = writeln!(out, "-{}", crate::storyline::clip_line(line));
    }
    out.push_str("```\n\n");
    let commits = removing_commits(path, removed.text);
    if !commits.is_empty() {
        let _ = writeln!(out, "## Commits que añadieron o quitaron este código\n\n{}\n", commits.join("\n"));
    }
    out
}

/// Commits que tocaron la línea quitada más reconocible (`git log -S`), de
/// cualquier rama: en una tarea, la que se revisa puede no ser la del disco.
fn removing_commits(path: &Path, removed: &str) -> Vec<String> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    let Some(probe) = removed.lines().map(str::trim).filter(|line| line.len() >= 4).max_by_key(|line| line.len())
    else {
        return Vec::new();
    };
    let probe: String = probe.chars().take(200).collect();
    let pickaxe = format!("-S{probe}");
    let Ok(log) =
        git(dir, &["log", "--all", "-n", "6", "--date=short", "--format=%h · %an · %ad · %s", &pickaxe, "--", name])
    else {
        return Vec::new();
    };
    log.lines().map(str::to_owned).collect()
}

/// Cada edición del símbolo con lo que se pidió y lo que explicó el agente,
/// completos: es lo que responde al «por qué».
fn write_history(
    out: &mut String,
    memory: &SymbolMemory,
    sources: Option<&crate::Sources>,
    activity: &mut HashMap<String, Vec<huella_core::Activity>>,
) -> Result<()> {
    let _ = writeln!(out, "### `{}`\n", memory.symbol.qualified_name);
    if memory.changed_since_last_session {
        out.push_str("El código actual no es el que dejó la última sesión que lo tocó: hubo cambios después.\n\n");
    }
    for (index, entry) in memory.entries.iter().take(MAX_HISTORY).enumerate() {
        let when = entry.at.as_deref().map_or("sin fecha", |at| &at[..16.min(at.len())]);
        let _ = writeln!(out, "- **{when}** · sesión {}", &entry.session_id[..8.min(entry.session_id.len())]);
        if let Some(prompt) = &entry.context.prompt {
            let _ = writeln!(out, "  - Petición: «{}»", clip(&one_line(prompt), 1500));
        }
        if let Some(rationale) = &entry.context.rationale {
            let _ = writeln!(out, "  - Lo que explicó el agente: {}", clip(&one_line(rationale), 2500));
        }
        // La explicación guardada es solo el último texto antes de editar; lo
        // pensado en el turno suele contar mejor el porqué.
        if let Some(sources) = sources.filter(|_| index < WITH_REASONING) {
            if !activity.contains_key(&entry.session_id) {
                let read = crate::evidence::session_activity(sources, &entry.session_id)?;
                activity.insert(entry.session_id.clone(), read);
            }
            for thought in crate::evidence::reasoning_before(&activity[&entry.session_id], &entry.edit_id, 3) {
                if entry.context.rationale.as_deref() != Some(thought.as_str()) {
                    let _ = writeln!(out, "  - Lo que pensó antes de escribirlo: {}", clip(&one_line(&thought), 1500));
                }
            }
        }
    }
    if memory.entries.len() > MAX_HISTORY {
        let _ = writeln!(out, "- … y {} ediciones anteriores", memory.entries.len() - MAX_HISTORY);
    }
    out.push('\n');
    Ok(())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    #[test]
    fn a_question_is_about_the_reviewed_text_even_if_the_file_is_not_on_disk() {
        struct Answers;
        impl huella_decisions::JsonModel for Answers {
            fn model_id(&self) -> String {
                "fake:1".into()
            }
            fn complete_json(
                &self,
                _: &str,
                request: &str,
                _: &serde_json::Value,
            ) -> std::result::Result<String, huella_decisions::ExtractError> {
                assert!(request.contains("solo_en_la_rama"), "{request}");
                Ok(r#"{"answer":"Suma."}"#.to_owned())
            }
        }
        let store = huella_store::Store::open_in_memory().unwrap();
        let ask = super::CodeAsk {
            path: std::path::Path::new("/no/existe/en/disco.ts"),
            quote: "solo_en_la_rama();",
            lines: None,
            question: "¿Qué hace?",
            reviewed: Some("function f() {\n  solo_en_la_rama();\n}\n"),
            removed: None,
        };
        let answer = super::ask_code(&store, None, &Answers, ask).unwrap();
        assert_eq!((answer.start_line, answer.end_line), (2, 2));
    }

    #[test]
    fn a_selection_that_skips_lines_goes_from_its_first_line_to_its_last() {
        let source = "class Cart {\n  add() {}\n  remove() {}\n}\n";
        assert_eq!(super::locate(source, "class Cart {\n}", None), None);
        assert_eq!(super::locate_span(source, "class Cart {\n\n}", None), Some((1, 4)));
        assert_eq!(super::locate_span(source, "otra cosa\n}", None), None);
    }

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
    fn a_question_about_removed_lines_shows_them_with_the_commit_that_removed_them() {
        struct Sees(RefCell<String>);
        impl JsonModel for Sees {
            fn model_id(&self) -> String {
                "fake:1".into()
            }
            fn complete_json(&self, _: &str, user: &str, _: &Value) -> std::result::Result<String, ExtractError> {
                user.clone_into(&mut self.0.borrow_mut());
                Ok(json!({"answer": "Se quitó al pasar a céntimos."}).to_string())
            }
        }
        let dir = std::env::temp_dir().join(format!("huella-code-ask-removed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };
        let path = dir.join("total.ts");
        run(&["init", "-q"]);
        std::fs::write(&path, "function total(p) {\n  const rounded = Math.round(p);\n  return rounded;\n}\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "Redondea el total"]);
        std::fs::write(&path, "function total(p) {\n  return p;\n}\n").unwrap();
        run(&["commit", "-q", "-am", "Quita el redondeo: ya llega en céntimos"]);

        let store = Store::open_in_memory().unwrap();
        let model = Sees(RefCell::new(String::new()));
        let answer = ask_code(
            &store,
            None,
            &model,
            CodeAsk {
                path: &path,
                quote: "  return p;",
                lines: None,
                question: "¿Por qué se quita?",
                reviewed: None,
                removed: Some(Removed {
                    text: "  const rounded = Math.round(p);\n  return rounded;",
                    place: RemovedPlace::Before,
                }),
            },
        )
        .unwrap();
        assert_eq!((answer.start_line, answer.end_line), (2, 2), "se ancla a la línea que sigue");
        let seen = model.0.borrow().clone();
        assert!(seen.contains("-  const rounded = Math.round(p);"), "{seen}");
        assert!(seen.contains("justo antes de la línea marcada"), "{seen}");
        assert!(seen.contains("Quita el redondeo"), "el commit que lo quitó: {seen}");
        let stored = store.code_questions(&normalize_path(&path, None)).unwrap();
        assert_eq!(stored[0].removed, "  const rounded = Math.round(p);\n  return rounded;");
        std::fs::remove_dir_all(&dir).unwrap();
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

        let answer = ask_code(
            &store,
            None,
            &model,
            CodeAsk {
                path: &path,
                quote: "let total = sum();\nsave(total)",
                lines: None,
                question: "¿Qué hace?",
                reviewed: None,
                removed: None,
            },
        )
        .unwrap();
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
