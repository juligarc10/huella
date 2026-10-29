//! El bucle de la revisión: lo que la persona pide corregir vuelve al agente
//! como instrucciones, se ve qué se hizo después y se puede preguntar a la
//! sesión por cualquier paso.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use huella_decisions::JsonModel;
use huella_review::flow::FlowStep;
use huella_review::{DiffLine, SessionReview, Step};
use huella_store::{ReviewComment, Store, Verdict};
use serde_json::{Value, json};

/// Líneas del cambio que acompañan a cada comentario en las instrucciones.
const FEEDBACK_LINES: usize = 30;
/// Contexto máximo que se envía para responder una pregunta.
const ASK_MAX_CHARS: usize = 60_000;

/// Instrucciones de corrección para el agente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    pub markdown: String,
    /// Comentarios abiertos y pasos marcados para cambiar o deshacer que incluye.
    pub items: usize,
}

/// Lo que la persona pide cambiar, en texto para el agente: cada comentario
/// abierto con su fichero, su función, la línea a la que se refiere y el
/// código del cambio, y los pasos marcados para cambiar o deshacer.
///
/// # Errors
/// Si falla la base de datos.
pub fn feedback(store: &Store, review: &SessionReview, plan: &[FlowStep]) -> crate::Result<Feedback> {
    let comments: Vec<ReviewComment> =
        store.comments(&review.session_id)?.into_iter().filter(|comment| comment.status == "open").collect();
    let verdicts = store.review_verdicts(&review.session_id)?;
    let commented: BTreeSet<&str> = comments.iter().map(|comment| comment.step_key.as_str()).collect();
    let flagged: Vec<(usize, &FlowStep, Verdict)> = plan
        .iter()
        .enumerate()
        .filter_map(|(index, step)| {
            let key = step.key(review);
            let verdict = verdicts.get(&key)?;
            let current = verdict.content_hash == step.content_hash();
            (current && verdict.verdict != Verdict::Ok && !commented.contains(key.as_str())).then_some((
                index,
                step,
                verdict.verdict,
            ))
        })
        .collect();

    let short = &review.session_id[..8.min(review.session_id.len())];
    let mut out = String::new();
    let _ = writeln!(out, "# Correcciones de la revisión de la sesión {short}\n");
    if comments.is_empty() && flagged.is_empty() {
        out.push_str("No hay nada pendiente de corregir.\n");
        return Ok(Feedback { markdown: out, items: 0 });
    }
    out.push_str(
        "He revisado los cambios de esta sesión. Corrige lo siguiente, explica brevemente cada corrección y no \
         cambies nada más.\n",
    );
    let mut number = 0;
    for comment in &comments {
        number += 1;
        let step = plan.iter().enumerate().find(|(_, step)| step.key(review) == comment.step_key);
        let verdict = verdicts.get(&comment.step_key).map(|v| v.verdict);
        let place = comment
            .symbol
            .as_deref()
            .map_or_else(|| format!("`{}`", comment.path), |s| format!("`{s}` en `{}`", comment.path));
        let _ = writeln!(out, "\n## {number}. {place}\n");
        if let Some(prompt) = step.and_then(|(_, s)| turn_prompt(review, s)) {
            let _ = writeln!(out, "Parte de la petición: «{}»\n", clip(&one_line(&prompt), 200));
        }
        if let Some(quote) = comment.quote.as_deref().filter(|q| !q.trim().is_empty()) {
            let _ = writeln!(out, "Línea: `{}`\n", quote.trim());
        }
        if verdict == Some(Verdict::Discard) {
            out.push_str("Deshaz este cambio.\n\n");
        }
        let _ = writeln!(out, "{}", comment.body.trim());
        if let Some((_, step)) = step {
            write_code(&mut out, &comment.path, step, comment.quote.as_deref());
        }
    }
    for (_, step, verdict) in &flagged {
        number += 1;
        let path = &review.files[step.file].path;
        let place =
            step.symbol.as_ref().map_or_else(|| format!("`{path}`"), |s| format!("`{}` en `{path}`", s.qualified_name));
        let _ = writeln!(out, "\n## {number}. {place}\n");
        out.push_str(match verdict {
            Verdict::Discard => "Deshaz este cambio.\n",
            _ => "Este cambio no es correcto: revísalo y corrígelo.\n",
        });
        write_code(&mut out, path, step, None);
    }
    let _ = writeln!(out, "\nCuando termines, revisaré solo lo que hayas cambiado (Huella, sesión {short}).");
    Ok(Feedback { markdown: out, items: number })
}

/// El código del paso en un bloque, centrado en la línea comentada.
fn write_code(out: &mut String, path: &str, step: &FlowStep, quote: Option<&str>) {
    let lines: Vec<String> = step
        .lines
        .iter()
        .map(|line| match line {
            DiffLine::Added { text, .. } => format!("+ {text}"),
            DiffLine::Removed(text) => format!("- {text}"),
            DiffLine::Context(text) => format!("  {text}"),
        })
        .collect();
    let center = quote
        .and_then(|quote| lines.iter().position(|line| line.trim_start_matches(['+', '-', ' ']).trim() == quote.trim()))
        .unwrap_or(0);
    let start = center.saturating_sub(FEEDBACK_LINES / 3);
    let shown: Vec<&String> = lines.iter().skip(start).take(FEEDBACK_LINES).collect();
    if shown.is_empty() {
        return;
    }
    let language = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or_default();
    let _ = writeln!(out, "\n```diff {language}");
    for line in shown {
        let _ = writeln!(out, "{line}");
    }
    out.push_str("```\n");
}

/// Una sesión que tocó los ficheros de la revisión después de comentarla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUp {
    pub session_id: String,
    pub paths: Vec<String>,
    pub last_at: String,
}

/// Pistas sobre si un comentario abierto ya se atendió.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentHint {
    pub id: i64,
    /// La línea comentada ya no está en el fichero actual.
    pub quote_gone: bool,
    /// Alguna sesión editó el fichero después del comentario.
    pub edited_after: bool,
}

/// Qué pasó después de comentar: las sesiones que tocaron los ficheros de la
/// revisión tras el primer comentario abierto, y una pista por comentario.
///
/// # Errors
/// Si falla la base de datos.
pub fn follow_up(store: &Store, review: &SessionReview) -> crate::Result<(Vec<FollowUp>, Vec<CommentHint>)> {
    let comments = store.comments(&review.session_id)?;
    let Some(since) = comments.iter().filter(|c| c.status == "open").map(|c| c.created_at.clone()).min() else {
        return Ok((Vec::new(), Vec::new()));
    };
    let paths: Vec<String> = review.files.iter().map(|file| file.path.clone()).collect();
    let after = store.edits_after(&since, &paths)?;
    let follow_ups: Vec<FollowUp> =
        after.into_iter().map(|(session_id, paths, last_at)| FollowUp { session_id, paths, last_at }).collect();
    let hints = comments
        .iter()
        .filter(|comment| comment.status == "open")
        .map(|comment| {
            let current = std::fs::read_to_string(&comment.path).unwrap_or_default();
            let quote_gone = comment
                .quote
                .as_deref()
                .map(str::trim)
                .filter(|quote| !quote.is_empty())
                .is_some_and(|quote| !current.lines().any(|line| line.trim() == quote));
            let edited_after =
                follow_ups.iter().any(|f| f.last_at > comment.created_at && f.paths.contains(&comment.path));
            CommentHint { id: comment.id, quote_gone, edited_after }
        })
        .collect();
    Ok((follow_ups, hints))
}

/// Respuesta a una pregunta sobre la sesión.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub id: i64,
    pub answer: String,
    pub model: String,
}

const ASK_SYSTEM: &str = "\
Ayudas a una persona a revisar código que escribió un agente de IA. Respondes a su pregunta sobre un cambio \
usando solo el contexto que se te da: lo que pidió la persona, lo que explicó el agente, el diff y el código \
actual.

- Sé concreto: cita funciones, variables y líneas.
- Distingue lo que dijo el agente de lo que deduces tú del código.
- Si el contexto no basta para responder, dilo y explica qué habría que mirar.
- Si ves un problema real relacionado con la pregunta (un caso límite, un error), señálalo.
- Escribe en el idioma de la pregunta, en markdown breve.";

/// Pregunta a la sesión, sobre un paso (`step`, su índice en el plan) o
/// sobre la sesión entera. La pregunta y la respuesta se guardan.
///
/// # Errors
/// Si falla el modelo o la base de datos.
pub fn ask(
    store: &Store,
    model: &dyn JsonModel,
    review: &SessionReview,
    plan: &[FlowStep],
    step: Option<usize>,
    question: &str,
) -> crate::Result<Answer> {
    let context = ask_context(store, review, plan, step)?;
    let request = huella_core::redact::redact(&format!("{context}\n## Pregunta\n\n{}", question.trim())).into_owned();
    let schema = json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
        "additionalProperties": false
    });
    let text = model
        .complete_json(ASK_SYSTEM, &request, &schema)
        .map_err(|error| crate::EngineError::Model(error.to_string()))?;
    let parsed: Value = serde_json::from_str(&text).map_err(|error| crate::EngineError::Model(error.to_string()))?;
    let answer = parsed["answer"].as_str().unwrap_or_default().trim().to_owned();
    if answer.is_empty() {
        return Err(crate::EngineError::Model("el modelo devolvió una respuesta vacía".into()));
    }
    let key = step.and_then(|index| plan.get(index)).map(|s| s.key(review));
    let model_id = model.model_id();
    let id = store.add_question(&review.session_id, key.as_deref(), question.trim(), &answer, &model_id)?;
    Ok(Answer { id, answer, model: model_id })
}

/// Contexto de una pregunta: la petición, el paso con su diff, sus
/// explicaciones y el código actual, y las preguntas anteriores. Sin paso,
/// el resumen de la sesión entera.
fn ask_context(store: &Store, review: &SessionReview, plan: &[FlowStep], step: Option<usize>) -> crate::Result<String> {
    let mut out = String::new();
    let _ = writeln!(out, "# Sesión {}\n", &review.session_id[..8.min(review.session_id.len())]);
    if let Some((index, step)) = step.and_then(|index| plan.get(index).map(|s| (index, s))) {
        write_step_context(&mut out, review, plan, index, step);
    } else {
        write_session_context(&mut out, review, plan);
    }
    let previous = store.questions(&review.session_id)?;
    if !previous.is_empty() {
        out.push_str("\n## Preguntas anteriores\n\n");
        for question in previous.iter().rev().take(3).rev() {
            let _ = writeln!(out, "P: {}\nR: {}\n", question.question, clip(&question.answer, 1500));
        }
    }
    Ok(clip(&out, ASK_MAX_CHARS))
}

/// Un paso: su petición, lo que explicó el agente, su diff, el código actual
/// y a qué llama.
fn write_step_context(out: &mut String, review: &SessionReview, plan: &[FlowStep], index: usize, step: &FlowStep) {
    let path = &review.files[step.file].path;
    if let Some(prompt) = turn_prompt(review, step) {
        let _ = writeln!(out, "## Petición\n\n«{}»\n", clip(&prompt, 4000));
    }
    let name = step.symbol.as_ref().map_or("(cambios fuera de funciones)", |s| s.qualified_name.as_str());
    let _ = writeln!(out, "## Paso {} de {}: `{name}` en `{path}`\n", index + 1, plan.len());
    let rationales = rationales(review, step);
    if !rationales.is_empty() {
        out.push_str("### Lo que explicó el agente\n\n");
        for rationale in rationales {
            let _ = writeln!(out, "> {}\n", clip(&one_line(rationale), 1500));
        }
    }
    out.push_str("### Diff del paso (+ añadido, - quitado, ! sin explicación del agente)\n\n```\n");
    for line in &step.lines {
        let rendered = match line {
            DiffLine::Added { text, source } => {
                let external = source.is_some_and(|s| matches!(review.steps[s], Step::External { .. }));
                format!("{} {text}", if external { '!' } else { '+' })
            }
            DiffLine::Removed(text) => format!("- {text}"),
            DiffLine::Context(text) => format!("  {text}"),
        };
        let _ = writeln!(out, "{rendered}");
    }
    out.push_str("```\n\n");
    if let Some(code) = current_code(path, step) {
        let _ = writeln!(out, "### Código actual del fichero\n\n```\n{code}\n```\n");
    }
    let calls: Vec<&str> = step
        .calls
        .iter()
        .filter_map(|&call| plan.get(call)?.symbol.as_ref().map(|s| s.qualified_name.as_str()))
        .collect();
    if !calls.is_empty() {
        let _ = writeln!(out, "Llama a: {}\n", calls.join(", "));
    }
}

/// La sesión entera: sus peticiones y la lista de pasos con su porqué.
fn write_session_context(out: &mut String, review: &SessionReview, plan: &[FlowStep]) {
    out.push_str("## Peticiones\n\n");
    for turn in &review.turns {
        if let Some(prompt) = &turn.prompt {
            let _ = writeln!(out, "{}. «{}»", turn.index + 1, clip(&one_line(prompt), 600));
        }
    }
    out.push_str("\n## Pasos de la revisión\n\n");
    for (index, step) in plan.iter().enumerate() {
        let name = step.symbol.as_ref().map_or("(otros cambios)", |s| s.qualified_name.as_str());
        let why = rationales(review, step).last().map(|r| clip(&one_line(r), 200)).unwrap_or_default();
        let _ = writeln!(
            out,
            "{}. `{name}` en `{}` (+{} −{}) {why}",
            index + 1,
            review.files[step.file].path,
            step.added(),
            step.removed()
        );
        if out.len() > ASK_MAX_CHARS / 2 {
            let _ = writeln!(out, "… y {} pasos más", plan.len() - index - 1);
            break;
        }
    }
}

/// El código actual del símbolo del paso (o el principio del fichero), con
/// números de línea.
fn current_code(path: &str, step: &FlowStep) -> Option<String> {
    let source = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    let (start, end) =
        step.symbol.as_ref().map_or((1, 120), |s| (s.start_line.saturating_sub(5).max(1), s.end_line + 5));
    let end = end.min(lines.len());
    (start <= end).then(|| {
        (start..=end).map(|number| format!("{number:>5}  {}", lines[number - 1])).collect::<Vec<_>>().join("\n")
    })
}

fn turn_prompt(review: &SessionReview, step: &FlowStep) -> Option<String> {
    let turn = step.turn?;
    review.turns.iter().find(|t| t.index == turn)?.prompt.clone()
}

fn rationales<'a>(review: &'a SessionReview, step: &FlowStep) -> Vec<&'a str> {
    step.sources
        .iter()
        .filter_map(|&source| match &review.steps[source] {
            Step::Agent { rationale: Some(rationale), .. } => Some(rationale.as_str()),
            _ => None,
        })
        .collect()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    use std::path::PathBuf;

    use huella_core::{AfterState, EditContext, EditKind, EditOrigin, FileEdit};
    use huella_decisions::ExtractError;
    use huella_store::NewComment;

    use super::*;

    const CODE: &str = "export function redeem(code: string) {\n  if (!code) return;\n  return check(code);\n}\n\nexport function check(code: string) {\n  return code.length > 3;\n}\n";

    fn review_at(path: &str) -> (SessionReview, Vec<FlowStep>) {
        let edit = FileEdit {
            origin: EditOrigin {
                agent: "t",
                session_id: "sesion-1".into(),
                event_id: "e".into(),
                timestamp: None,
                cwd: None,
            },
            path: PathBuf::from(path),
            kind: EditKind::Create,
            before: None,
            after: AfterState::Known(CODE.into()),
            user_modified: false,
            context: EditContext {
                prompt: Some("añade el canje".into()),
                rationale: Some("valido el código antes de canjear".into()),
            },
        };
        let review = huella_review::build("sesion-1", &[edit], |e| e.path.display().to_string(), |_| None);
        let plan = huella_review::flow::plan(&review);
        (review, plan)
    }

    fn step_named<'a>(plan: &'a [FlowStep], name: &str) -> (usize, &'a FlowStep) {
        plan.iter().enumerate().find(|(_, s)| s.symbol.as_ref().is_some_and(|s| s.qualified_name == name)).unwrap()
    }

    #[test]
    fn feedback_lists_open_comments_with_their_code_and_flagged_steps() {
        let store = Store::open_in_memory().unwrap();
        let (review, plan) = review_at("/r/canje.ts");
        let (_, redeem) = step_named(&plan, "redeem");
        let (_, check) = step_named(&plan, "check");
        store
            .add_comment(&NewComment {
                session_id: "sesion-1",
                step_key: &redeem.key(&review),
                path: "/r/canje.ts",
                symbol: Some("redeem"),
                quote: Some("if (!code) return;"),
                body: "Un código vacío debería lanzar un error, no salir en silencio.",
            })
            .unwrap();
        store.set_step_verdict("sesion-1", &check.key(&review), &check.content_hash(), Some(Verdict::Discard)).unwrap();

        let feedback = feedback(&store, &review, &plan).unwrap();
        assert_eq!(feedback.items, 2);
        let text = &feedback.markdown;
        assert!(text.contains("## 1. `redeem` en `/r/canje.ts`"), "{text}");
        assert!(text.contains("Parte de la petición: «añade el canje»"), "{text}");
        assert!(text.contains("Línea: `if (!code) return;`"), "{text}");
        assert!(text.contains("+   if (!code) return;"), "el código del cambio: {text}");
        assert!(text.contains("## 2. `check` en `/r/canje.ts`\n\nDeshaz este cambio."), "{text}");
    }

    #[test]
    fn without_anything_open_there_is_nothing_to_correct() {
        let store = Store::open_in_memory().unwrap();
        let (review, plan) = review_at("/r/canje.ts");
        let feedback = feedback(&store, &review, &plan).unwrap();
        assert_eq!(feedback.items, 0);
        assert!(feedback.markdown.contains("No hay nada pendiente"));
    }

    struct Fake {
        requests: RefCell<Vec<String>>,
    }

    impl JsonModel for Fake {
        fn model_id(&self) -> String {
            "fake:1".into()
        }

        fn complete_json(&self, system: &str, user: &str, _schema: &Value) -> Result<String, ExtractError> {
            assert_eq!(system, ASK_SYSTEM);
            self.requests.borrow_mut().push(user.to_owned());
            Ok(r#"{"answer": "Sale sin error porque el agente lo trató como opcional."}"#.into())
        }
    }

    #[test]
    fn asking_about_a_step_sends_its_context_and_keeps_the_answer() {
        let store = Store::open_in_memory().unwrap();
        let (review, plan) = review_at("/r/no-existe.ts");
        let (index, _) = step_named(&plan, "redeem");
        let model = Fake { requests: RefCell::new(Vec::new()) };
        let question = "¿Por qué sale sin error? Mi clave es API_KEY=a8f3k2j9d0s7q1w4e6r5";
        let answer = ask(&store, &model, &review, &plan, Some(index), question).unwrap();
        assert_eq!(answer.model, "fake:1");
        let request = model.requests.borrow()[0].clone();
        assert!(request.contains("«añade el canje»"), "la petición: {request}");
        assert!(request.contains("valido el código antes de canjear"), "la explicación: {request}");
        assert!(request.contains("+   if (!code) return;"), "el diff: {request}");
        assert!(request.contains("API_KEY=[secreto]") && !request.contains("a8f3k2"), "sin secretos: {request}");
        let stored = store.questions("sesion-1").unwrap();
        assert_eq!(stored[0].answer, answer.answer);
        assert_eq!(stored[0].step_key.as_deref(), Some(plan[index].key(&review).as_str()));

        // La siguiente pregunta ve la anterior.
        ask(&store, &model, &review, &plan, None, "¿Qué falta por probar?").unwrap();
        assert!(model.requests.borrow()[1].contains("## Preguntas anteriores"));
    }

    #[test]
    fn a_comment_whose_line_disappeared_is_probably_handled() {
        let dir = std::env::temp_dir().join(format!("huella-loop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("canje.ts");
        std::fs::write(&file, CODE).unwrap();
        let path = file.display().to_string();
        let store = Store::open_in_memory().unwrap();
        let (review, plan) = review_at(&path);
        let (_, redeem) = step_named(&plan, "redeem");
        let key = redeem.key(&review);
        let comment = NewComment {
            session_id: "sesion-1",
            step_key: &key,
            path: &path,
            symbol: Some("redeem"),
            quote: Some("if (!code) return;"),
            body: "lanza un error",
        };
        store.add_comment(&comment).unwrap();
        assert!(!follow_up(&store, &review).unwrap().1[0].quote_gone);
        std::fs::write(&file, CODE.replace("if (!code) return;", "if (!code) throw new Error('vacío');")).unwrap();
        assert!(follow_up(&store, &review).unwrap().1[0].quote_gone);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
