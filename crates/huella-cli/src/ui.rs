//! `huella ui`: la aplicación local de Huella (buscar, decisiones, revisión
//! de sesiones y estado), servida desde el propio binario.
//!
//! La interfaz es una aplicación web compilada en `ui/dist` e incrustada en el
//! ejecutable. Solo escucha en 127.0.0.1, rechaza cualquier `Host` que no sea
//! el suyo (así una web externa no puede llegar a ella con un DNS que apunte a
//! 127.0.0.1) y la API exige el token de esta ejecución en una cabecera, que
//! una web de otro origen no puede enviar sin permiso. Las tareas largas
//! (indexar, vectores, extraer decisiones, medir refactors) corren en hilos con
//! su propia conexión a la base de datos y la interfaz consulta su progreso.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use huella_engine::lineage::{Link, read_from_disk};
use huella_engine::triage::{Assessment, Level, assess};
use huella_review::SessionReview;
use huella_review::flow::FlowStep;
use huella_store::{DecisionFilter, FoundBy, HistoryEntry, SearchHit, SearchQuery, Store, StoredDecision, SymbolRole};
use serde::Serialize;
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::ModelArgs;
use crate::web::Editor;

#[derive(rust_embed::Embed)]
#[folder = "../../ui/dist"]
struct Assets;

/// Puerto por defecto de `huella ui`.
pub const DEFAULT_PORT: u16 = 4848;
/// Puerto del servidor de desarrollo de Vite (`pnpm dev` en `ui/`).
const VITE_PORT: u16 = 5173;
const SEARCH_LIMIT: usize = 30;
const JOBS: [&str; 4] = ["index", "embed", "refactors", "extract"];

pub struct Options {
    pub port: u16,
    pub open_browser: bool,
    pub editor: Editor,
    /// Acepta también el servidor de desarrollo de Vite, que reenvía `/api`.
    pub dev: bool,
    pub db: PathBuf,
    pub sources: huella_engine::Sources,
    pub model: ModelArgs,
}

pub(crate) struct App {
    store: Store,
    db: PathBuf,
    sources: huella_engine::Sources,
    editor: Editor,
    token: String,
    allowed_hosts: Vec<String>,
    jobs: Jobs,
    model: ModelArgs,
    /// La última revisión cargada: reconstruirla relee las transcripciones.
    review: Option<(SessionReview, Vec<FlowStep>)>,
    /// Actividad de la sesión cargada: la evidencia de cada paso.
    activity: Vec<huella_core::Activity>,
    /// Impacto de la sesión cargada, que recorre el proyecto: se calcula una vez.
    impact: Option<Value>,
    /// Cobertura en curso por sesión: `None` mientras se calcula, o el error.
    covering: Arc<Mutex<HashMap<String, Option<String>>>>,
    /// Guías en curso por sesión.
    guiding: Arc<Mutex<HashMap<String, Guiding>>>,
    /// Preguntas que el modelo aún está respondiendo, por sesión.
    asking: Asking,
    /// Preguntas sobre código que el modelo aún está respondiendo.
    code_asking: Arc<Mutex<Vec<PendingCodeAsk>>>,
}

/// Una pregunta sobre código en curso o que falló.
#[derive(Debug, Clone, Serialize)]
struct PendingCodeAsk {
    /// Ruta normalizada del fichero.
    path: String,
    quote: String,
    question: String,
    error: Option<String>,
}

/// Una pregunta a la sesión en curso o que falló.
#[derive(Debug, Clone, Serialize)]
struct PendingAsk {
    question: String,
    step: Option<usize>,
    /// Si el modelo falló, por qué.
    error: Option<String>,
}

type Asking = Arc<Mutex<HashMap<String, Vec<PendingAsk>>>>;

/// Guías de una sesión que un hilo está generando, una a una.
#[derive(Debug, Clone, Default, Serialize)]
struct Guiding {
    /// Claves de los pasos que faltan, la primera en curso.
    pending: Vec<String>,
    /// Pasos que fallaron, con el porqué.
    errors: Vec<String>,
}

/// Estado de una tarea en segundo plano.
#[derive(Debug, Clone, Default, Serialize)]
struct Job {
    /// `idle`, `running`, `done` o `failed`.
    state: &'static str,
    message: Option<String>,
    done: usize,
    total: usize,
    result: Option<Value>,
    /// Segundos desde 1970 en que terminó.
    finished_at: Option<u64>,
}

type Jobs = Arc<Mutex<BTreeMap<String, Job>>>;

impl App {
    /// Estado de la aplicación, sin red: el servidor le añade su token y sus
    /// `Host` permitidos; el escritorio lo usa tal cual.
    pub(crate) fn new(
        store: Store,
        db: PathBuf,
        sources: huella_engine::Sources,
        editor: Editor,
        model: ModelArgs,
    ) -> Self {
        let jobs: Jobs = Arc::new(Mutex::new(JOBS.iter().map(|kind| ((*kind).to_owned(), idle())).collect()));
        Self {
            store,
            db,
            sources,
            editor,
            token: String::new(),
            allowed_hosts: Vec::new(),
            jobs,
            model,
            review: None,
            activity: Vec::new(),
            impact: None,
            covering: Arc::new(Mutex::new(HashMap::new())),
            guiding: Arc::new(Mutex::new(HashMap::new())),
            asking: Arc::new(Mutex::new(HashMap::new())),
            code_asking: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn db(&self) -> &Path {
        &self.db
    }

    pub(crate) fn sources(&self) -> &huella_engine::Sources {
        &self.sources
    }
}

/// Respuesta independiente del transporte: la escribe el servidor HTTP o la
/// devuelve el protocolo propio de la aplicación de escritorio.
pub(crate) struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    /// `Cache-Control`, si hace falta.
    pub cache: Option<&'static str>,
    pub body: Vec<u8>,
}

/// Una petición a la aplicación: la API bajo `/api/` y, si no, la interfaz.
/// La seguridad del transporte (token, `Host`) la comprueba quien llama.
pub(crate) fn dispatch(app: &mut App, method: &str, path: &str, query: &str, body: &str) -> Reply {
    if !path.starts_with("/api/") {
        return asset(path);
    }
    match api(app, method, path, &query_params(query), body) {
        Ok((status, value)) => json_reply(status, &value),
        Err(error) => json_reply(500, &json!({"error": format!("{error:#}")})),
    }
}

pub fn serve(options: Options) -> Result<()> {
    // El puerto pedido; si está ocupado (otra `huella ui` abierta), uno libre.
    let server = Server::http(("127.0.0.1", options.port))
        .or_else(|_| Server::http(("127.0.0.1", 0)))
        .map_err(|error| anyhow::anyhow!("no se pudo abrir el servidor local: {error}"))?;
    let port = server.server_addr().to_ip().context("dirección del servidor desconocida")?.port();
    let mut store = Store::open(&options.db).with_context(|| format!("abriendo {}", options.db.display()))?;
    huella_engine::index(&mut store, &options.sources)?;
    let token = crate::web::token("huella-ui");
    let mut allowed_hosts = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let page_port = if options.dev {
        allowed_hosts.extend([format!("127.0.0.1:{VITE_PORT}"), format!("localhost:{VITE_PORT}")]);
        VITE_PORT
    } else {
        port
    };
    let url = format!("http://localhost:{page_port}/?t={token}");
    let mut app = App::new(store, options.db, options.sources, options.editor, options.model);
    app.token = token;
    app.allowed_hosts = allowed_hosts;
    if options.dev && Assets::get("index.html").is_none() {
        println!("(sin ui/dist: la interfaz la sirve Vite)");
    }
    println!("Huella en {url}");
    println!("Ctrl+C para terminar.");
    if options.open_browser {
        crate::web::open_in_browser(&url);
    }
    for request in server.incoming_requests() {
        if let Err(error) = handle(&mut app, request) {
            eprintln!("aviso: {error:#}");
        }
    }
    Ok(())
}

fn handle(app: &mut App, mut request: Request) -> Result<()> {
    let host = header(&request, "Host").unwrap_or_default();
    if !app.allowed_hosts.contains(&host) {
        return Ok(request.respond(Response::from_string("host no permitido").with_status_code(403))?);
    }
    let url = request.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    if path.starts_with("/api/") && header(&request, "X-Huella-Token").as_deref() != Some(app.token.as_str()) {
        let reply = json_reply(401, &json!({"error": "token no válido: abre la URL que imprime `huella ui`"}));
        return respond(request, reply);
    }
    let method = if *request.method() == Method::Post { "POST" } else { "GET" };
    let mut body = String::new();
    if method == "POST" {
        std::io::Read::read_to_string(request.as_reader(), &mut body)?;
    }
    let reply = dispatch(app, method, path, query, &body);
    respond(request, reply)
}

fn respond(request: Request, reply: Reply) -> Result<()> {
    let mut response =
        Response::from_data(reply.body).with_status_code(reply.status).with_header(content_type(reply.content_type));
    if let Some(cache) = reply.cache {
        response = response.with_header(raw_header("Cache-Control", cache));
    }
    Ok(request.respond(response)?)
}

fn api(app: &mut App, method: &str, path: &str, params: &HashMap<String, String>, body: &str) -> Result<(u16, Value)> {
    let segments: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
    let value = match (method, segments.as_slice()) {
        ("GET", ["overview"]) => overview(app)?,
        ("GET", ["search"]) => search(app, params)?,
        ("GET", ["decisions"]) => decisions(app, params)?,
        ("POST", ["decisions", uid, "status"]) => set_status(app, uid, body)?,
        ("POST", ["adr"]) => export_adr(app, body)?,
        ("GET", ["symbol"]) => symbol(app, params)?,
        ("GET", ["sessions"]) => sessions(app, params)?,
        ("GET", ["inbox"]) => inbox(app)?,
        ("GET", ["branch"]) => branch(app, params)?,
        ("GET", ["review", session]) => review(app, session)?,
        ("POST", ["review", session, "mark"]) => mark(app, session, body)?,
        ("POST", ["review", session, "status"]) => set_status_of_review(app, session, body)?,
        ("POST", ["review", session, "comments"]) => add_comment(app, session, body)?,
        ("GET", ["review", session, "feedback"]) => feedback(app, session)?,
        ("GET", ["review", session, "impact"]) => impact(app, session)?,
        ("POST", ["review", session, "coverage"]) => return start_coverage(app, session),
        ("POST", ["review", session, "guide"]) => return start_guide(app, session, body),
        ("POST", ["review", session, "guide", "check"]) => check_guide(app, session, body)?,
        ("POST", ["review", session, "ask"]) => return ask(app, session, body),
        ("POST", ["comments", id]) => update_comment(app, id, body)?,
        ("POST", ["code", "ask"]) => return ask_code(app, body),
        ("GET", ["code", "questions"]) => code_questions(app, params)?,
        ("POST", ["code", "questions", id, "delete"]) => {
            json!({"deleted": app.store.delete_code_question(id.parse().context("id no válido")?)?})
        }
        ("GET", ["status"]) => status(app)?,
        ("GET", ["jobs"]) => json!({"jobs": *app.jobs.lock().unwrap_or_else(PoisonError::into_inner)}),
        ("POST", ["jobs", kind]) => return Ok(start_job(app, kind)),
        ("POST", ["open"]) => open(app, body)?,
        _ => return Ok((404, json!({"error": "no encontrado"}))),
    };
    Ok((200, value))
}

fn overview(app: &App) -> Result<Value> {
    let stats = app.store.stats()?;
    let decisions = app.store.decisions(&DecisionFilter::default())?.len();
    let sessions = huella_engine::list_sessions(&app.store, 200)?;
    let mut projects: BTreeSet<String> = app.store.decision_projects()?.into_iter().collect();
    projects.extend(sessions.iter().filter_map(|s| s.project.clone()));
    Ok(json!({
        "stats": {
            "sessions": stats.sessions,
            "edits": stats.edits,
            "symbols": stats.anchored_symbols,
            "transcripts": stats.transcripts,
            "decisions": decisions,
        },
        "projects": projects,
        "recent": sessions.iter().take(6).map(session_json).collect::<Vec<_>>(),
        "editor": editor_name(app.editor),
        "semantic": crate::embedder(None, None).map(|e| huella_decisions::Embedder::model_id(&e)),
        "extractor": extractor_label(&app.model),
    }))
}

fn search(app: &App, params: &HashMap<String, String>) -> Result<Value> {
    let text = params.get("q").cloned().unwrap_or_default();
    let query = SearchQuery {
        text,
        project: non_empty(params.get("project")),
        limit: SEARCH_LIMIT,
        include_inactive: params.get("all").is_some_and(|v| v == "1"),
    };
    let embedder = params.get("semantic").is_some_and(|v| v == "1").then(|| crate::embedder(None, None)).flatten();
    let (hits, warning) = huella_engine::embeddings::search(
        &app.store,
        &query,
        embedder.as_ref().map(|e| e as &dyn huella_decisions::Embedder),
    )?;
    let mut projects: HashMap<String, Option<String>> = HashMap::new();
    let mut out = Vec::new();
    for hit in &hits {
        out.push(match hit {
            SearchHit::Request { session_id, at, prompt, snippet, paths, symbols, edits, score, found_by } => {
                if !projects.contains_key(session_id) {
                    projects.insert(session_id.clone(), huella_engine::session_project(&app.store, session_id)?);
                }
                json!({
                    "kind": "request",
                    "key": format!("{session_id}:{}", prompt.as_deref().unwrap_or_default()),
                    "session_id": session_id,
                    "project": projects[session_id],
                    "at": at,
                    "prompt": prompt,
                    "snippet": snippet,
                    "paths": paths,
                    "symbols": symbols.iter().map(|s| json!({"path": s.path, "name": s.name})).collect::<Vec<_>>(),
                    "edits": edits,
                    "score": score,
                    "found_by": found_by_name(*found_by),
                })
            }
            SearchHit::Decision { decision, snippet, score, found_by } => json!({
                "kind": "decision",
                "key": decision.uid,
                "decision": decision_json(decision),
                "snippet": snippet,
                "score": score,
                "found_by": found_by_name(*found_by),
            }),
        });
    }
    Ok(json!({"hits": out, "warning": warning, "semantic": embedder.is_some()}))
}

fn decisions(app: &App, params: &HashMap<String, String>) -> Result<Value> {
    let project = non_empty(params.get("project"));
    let filter = DecisionFilter { project: project.clone(), include_inactive: true, ..DecisionFilter::default() };
    let all = app.store.decisions(&filter)?;
    let scope = params.get("scope").map_or("project", String::as_str);
    let status = params.get("status").map_or("active", String::as_str);
    let mut counts: BTreeMap<&str, usize> = [("project", 0), ("user", 0), ("path", 0)].into_iter().collect();
    for decision in all.iter().filter(|d| d.status == "accepted") {
        *counts.entry(scope_of(decision)).or_default() += 1;
    }
    let listed: Vec<Value> = all
        .iter()
        .filter(|d| scope_of(d) == scope)
        .filter(|d| match status {
            "active" => d.status == "accepted",
            "superseded" => d.status == "superseded",
            "inactive" => matches!(d.status.as_str(), "rejected" | "proposed"),
            _ => true,
        })
        .map(decision_json)
        .collect();
    Ok(json!({"decisions": listed, "counts": counts, "project": project}))
}

fn set_status(app: &App, uid: &str, body: &str) -> Result<Value> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let status = request["status"].as_str().unwrap_or_default();
    if !matches!(status, "accepted" | "rejected" | "superseded" | "proposed" | "auto") {
        bail!("estado desconocido: {status}");
    }
    let found = app.store.decisions_by_uid(uid)?;
    let [decision] = found.as_slice() else { bail!("no hay una única decisión con el id {uid}") };
    app.store.set_decision_status(&decision.uid, (status != "auto").then_some(status))?;
    let updated = app.store.decisions_by_uid(&decision.uid)?;
    Ok(json!({"decision": updated.first().map(decision_json)}))
}

fn export_adr(app: &App, body: &str) -> Result<Value> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let project = request["project"].as_str().filter(|p| p.starts_with('/')).context("falta el proyecto")?;
    let out = Path::new(project).join("docs").join("adr");
    let report = huella_engine::adr::export_adrs(&app.store, project, &out, false)?;
    let show = |paths: &[PathBuf]| paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>();
    Ok(json!({
        "out": out.display().to_string(),
        "written": show(&report.written),
        "unchanged": report.unchanged,
        "edited_by_hand": show(&report.edited_by_hand),
        "orphaned": show(&report.orphaned),
    }))
}

fn symbol(app: &App, params: &HashMap<String, String>) -> Result<Value> {
    let path = params.get("path").filter(|p| p.starts_with('/')).context("falta la ruta absoluta del fichero")?;
    let source = std::fs::read_to_string(path).with_context(|| format!("{path} ya no existe o no se puede leer"))?;
    let memory = if let Some(name) = params.get("name") {
        huella_engine::memory_of(&app.store, Path::new(path), &source, name, &read_from_disk)?
    } else {
        let line = params.get("line").and_then(|l| l.parse().ok()).unwrap_or(1);
        huella_engine::memory_at(&app.store, Path::new(path), &source, line)?
    };
    let key = huella_engine::normalize_path(Path::new(path), None);
    let filter = DecisionFilter { scope: Some("path".into()), ..DecisionFilter::default() };
    let applying: Vec<Value> = app
        .store
        .decisions(&filter)?
        .iter()
        .filter(|d| d.scope_path.as_deref().is_some_and(|scope| key.starts_with(scope)))
        .map(decision_json)
        .collect();
    let memory = memory.map(|memory| {
        json!({
            "symbol": memory.symbol,
            "changed": memory.changed_since_last_session,
            "entries": memory.entries.iter().map(entry_json).collect::<Vec<_>>(),
            "origins": memory.origins.iter().map(|o| json!({
                "path": o.path,
                "name": o.qualified_name,
                "link": match o.link { Link::Renamed => "renamed", Link::Moved => "moved", Link::MovedAndRenamed => "moved_and_renamed" },
                "similarity": o.similarity,
            })).collect::<Vec<_>>(),
        })
    });
    Ok(json!({"path": path, "memory": memory, "decisions": applying}))
}

/// Revisión de la rama actual de un proyecto contra su base.
fn branch(app: &App, params: &HashMap<String, String>) -> Result<Value> {
    let project = non_empty(params.get("project")).context("indica el proyecto")?;
    let base = non_empty(params.get("base"));
    let review = huella_engine::branch::branch_review(&app.sources, Path::new(&project), base.as_deref())?;
    Ok(serde_json::to_value(review)?)
}

fn sessions(app: &App, params: &HashMap<String, String>) -> Result<Value> {
    let limit = params.get("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
    let listed = huella_engine::list_sessions(&app.store, limit)?;
    Ok(json!({"sessions": listed.iter().map(session_json).collect::<Vec<_>>()}))
}

/// Cuánto de cada nivel del triaje tiene ya veredicto.
#[derive(Debug, Default, Serialize)]
struct Comprehension {
    must: Tally,
    acknowledge: Tally,
    skip: Tally,
}

#[derive(Debug, Default, Serialize)]
struct Tally {
    total: usize,
    done: usize,
}

impl Comprehension {
    fn count(&mut self, level: Level, done: bool) {
        let tally = match level {
            Level::Must => &mut self.must,
            Level::Acknowledge => &mut self.acknowledge,
            Level::Skip => &mut self.skip,
        };
        tally.total += 1;
        tally.done += usize::from(done);
    }
}

fn review(app: &mut App, session: &str) -> Result<Value> {
    load_review(app, session)?;
    let (review, plan) = app.review.as_ref().context("revisión no cargada")?;
    let points = huella_review::attention::attention(review, plan);
    let narrations = huella_engine::narration::narrations(&app.store, review, plan, &points)?;
    let mut value = crate::web::payload(review, plan, &app.store.review_marks(&review.session_id)?, app.editor);
    value["narrations"] = serde_json::to_value(&narrations)?;
    let project = huella_engine::session_project(&app.store, &review.session_id)?;
    value["project"] = json!(project);
    let Assessment { coverage, evidence, policy, policy_error, triage } =
        assess(&app.store, review, plan, &points, &app.activity, project.as_deref())?;
    let context: Vec<_> = evidence.iter().zip(&triage).collect();
    let mut guides = huella_engine::guide::guides(&app.store, review, plan, &context)?;
    // Veredicto de cada paso, si se dio con su contenido actual.
    let verdicts = app.store.review_verdicts(&review.session_id)?;
    let mut comprehension = Comprehension::default();
    if let Some(steps) = value["steps"].as_array_mut() {
        for (index, (((step, flow), found), triaged)) in
            steps.iter_mut().zip(plan).zip(evidence).zip(&triage).enumerate()
        {
            let verdict = verdicts
                .get(&flow.key(review))
                .filter(|v| v.content_hash == flow.content_hash())
                .map(|v| v.verdict.as_str());
            comprehension.count(triaged.level, verdict.is_some());
            step["verdict"] = json!(verdict);
            step["evidence"] = serde_json::to_value(found)?;
            step["triage"] = serde_json::to_value(triaged)?;
            step["guide"] = serde_json::to_value(guides.remove(&index))?;
        }
    }
    value["policy"] = json!({
        "preset": policy.preset, "source": policy.source, "levels": policy.levels, "default": policy.default,
        "error": policy_error,
    });
    value["guiding"] = json!(app.guiding.lock().unwrap_or_else(PoisonError::into_inner).get(&review.session_id));
    value["comprehension"] = serde_json::to_value(comprehension)?;
    app.store.record_review_steps(&review.session_id, plan.len())?;
    let states = app.store.review_states()?;
    value["status"] = json!(states.get(&review.session_id).map_or("pending", |s| s.status.as_str()));
    value["comments"] = json!(app.store.comments(&review.session_id)?.iter().map(comment_json).collect::<Vec<_>>());
    value["questions"] = json!(app.store.questions(&review.session_id)?.iter().map(|q| json!({
        "id": q.id, "step_key": q.step_key, "question": q.question, "answer": q.answer, "model": q.model, "created_at": q.created_at,
    })).collect::<Vec<_>>());
    value["asking"] = json!(
        app.asking.lock().unwrap_or_else(PoisonError::into_inner).get(&review.session_id).cloned().unwrap_or_default()
    );
    let (follow_ups, hints) = huella_engine::review_loop::follow_up(&app.store, review)?;
    value["follow_ups"] = json!(follow_ups.iter().map(|f| json!({
        "session_id": f.session_id, "paths": f.paths, "last_at": f.last_at, "same_session": f.session_id == review.session_id,
    })).collect::<Vec<_>>());
    value["hints"] = json!(
        hints
            .iter()
            .map(|h| json!({"id": h.id, "quote_gone": h.quote_gone, "edited_after": h.edited_after}))
            .collect::<Vec<_>>()
    );
    value["ask_model"] = json!(extractor_label(&app.model));
    value["ask_problem"] = json!(crate::model_problem(&app.model));
    value["coverage"] = serde_json::to_value(coverage)?;
    value["covering"] = match app.covering.lock().unwrap_or_else(PoisonError::into_inner).get(&review.session_id) {
        Some(None) => json!({"running": true}),
        Some(Some(error)) => json!({"running": false, "error": error}),
        None => Value::Null,
    };
    Ok(value)
}

/// La bandeja: sesiones recientes con su estado de revisión.
pub(crate) fn inbox(app: &App) -> Result<Value> {
    let states = app.store.review_states()?;
    let sessions = huella_engine::list_sessions(&app.store, 100)?;
    let items: Vec<Value> = sessions
        .iter()
        .map(|listing| {
            let state = states.get(&listing.summary.id);
            let mut item = session_json(listing);
            item["status"] = json!(state.map_or("pending", |s| s.status.as_str()));
            item["steps"] = json!(state.and_then(|s| s.steps));
            item["reviewed"] = json!(state.map_or(0, |s| s.reviewed));
            item["open_comments"] = json!(state.map_or(0, |s| s.open_comments));
            item["status_at"] = json!(state.map(|s| s.updated_at.clone()));
            item
        })
        .collect();
    Ok(json!({"sessions": items}))
}

fn comment_json(comment: &huella_store::ReviewComment) -> Value {
    json!({
        "id": comment.id,
        "step_key": comment.step_key,
        "path": comment.path,
        "symbol": comment.symbol,
        "quote": comment.quote,
        "body": comment.body,
        "status": comment.status,
        "created_at": comment.created_at,
        "resolved_at": comment.resolved_at,
        "resolution": comment.resolution,
    })
}

/// Pasa la sesión a «en revisión» la primera vez que se marca o comenta algo.
fn start_reviewing(app: &App, session_id: &str, steps: usize) -> Result<()> {
    if app.store.review_status(session_id)?.is_none() {
        app.store.set_review_status(session_id, Some("in_review"), Some(steps))?;
    }
    Ok(())
}

fn set_status_of_review(app: &mut App, session: &str, body: &str) -> Result<Value> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let status = request["status"].as_str().unwrap_or_default();
    if !matches!(status, "pending" | "in_review" | "changes_requested" | "approved") {
        bail!("estado desconocido: {status}");
    }
    load_review(app, session)?;
    let (session_id, steps) = app
        .review
        .as_ref()
        .map(|(review, plan)| (review.session_id.clone(), plan.len()))
        .context("revisión no cargada")?;
    app.store.set_review_status(&session_id, (status != "pending").then_some(status), Some(steps))?;
    Ok(json!({"status": status}))
}

fn add_comment(app: &mut App, session: &str, body: &str) -> Result<Value> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let key = request["key"].as_str().context("falta key")?;
    let text = request["body"].as_str().map(str::trim).filter(|t| !t.is_empty()).context("el comentario está vacío")?;
    load_review(app, session)?;
    let (review, plan) = app.review.as_ref().context("revisión no cargada")?;
    let step = plan.iter().find(|step| step.key(review) == key).context("ese paso ya no existe")?;
    let comment = huella_store::NewComment {
        session_id: &review.session_id,
        step_key: key,
        path: &review.files[step.file].path,
        symbol: step.symbol.as_ref().map(|s| s.qualified_name.as_str()),
        quote: request["quote"].as_str().filter(|q| !q.trim().is_empty()),
        body: text,
    };
    let id = app.store.add_comment(&comment)?;
    start_reviewing(app, &review.session_id, plan.len())?;
    Ok(json!({"id": id}))
}

fn update_comment(app: &App, id: &str, body: &str) -> Result<Value> {
    let id: i64 = id.parse().context("id de comentario no válido")?;
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let found = match request["status"].as_str() {
        Some("deleted") => app.store.delete_comment(id)?,
        Some(status @ ("open" | "resolved" | "dismissed")) => {
            app.store.set_comment_status(id, status, request["resolution"].as_str())?
        }
        other => bail!("estado desconocido: {other:?}"),
    };
    if !found {
        bail!("no hay ningún comentario {id}");
    }
    Ok(json!({}))
}

/// Cobertura de la petición de cada turno: la calcula un hilo con el modelo
/// configurado; la revisión la muestra en curso mientras tanto.
fn start_coverage(app: &mut App, session: &str) -> Result<(u16, Value)> {
    if let Some(problem) = crate::model_problem(&app.model) {
        bail!(problem);
    }
    load_review(app, session)?;
    let (review, plan) = app.review.clone().context("revisión no cargada")?;
    let session_id = review.session_id.clone();
    {
        let mut running = app.covering.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(running.get(&session_id), Some(None)) {
            return Ok((202, json!({"running": true})));
        }
        running.insert(session_id.clone(), None);
    }
    let (covering, db, model) = (Arc::clone(&app.covering), app.db.clone(), app.model.clone());
    std::thread::spawn(move || {
        let outcome = (|| -> Result<Vec<String>> {
            let store = Store::open(&db)?;
            let model = crate::build_model(&model)?;
            let points = huella_review::attention::attention(&review, &plan);
            Ok(huella_engine::coverage::cover(&store, &review, &plan, &points, model.as_ref(), |_| {})?.failures)
        })();
        let mut all = covering.lock().unwrap_or_else(PoisonError::into_inner);
        match outcome {
            Ok(failures) if failures.is_empty() => {
                all.remove(&session_id);
            }
            Ok(failures) => {
                all.insert(session_id, Some(failures.join("; ")));
            }
            Err(error) => {
                all.insert(session_id, Some(format!("{error:#}")));
            }
        }
    });
    Ok((202, json!({"running": true})))
}

/// Guía de los pasos pedidos (`keys`): un hilo los recorre en orden con el
/// modelo configurado. Si ya hay uno con esta sesión, los nuevos se añaden a
/// su cola.
fn start_guide(app: &mut App, session: &str, body: &str) -> Result<(u16, Value)> {
    if let Some(problem) = crate::model_problem(&app.model) {
        bail!(problem);
    }
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let keys: Vec<String> = request["keys"]
        .as_array()
        .context("falta keys")?
        .iter()
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect();
    load_review(app, session)?;
    let (review, plan) = app.review.clone().context("revisión no cargada")?;
    let session_id = review.session_id.clone();
    let project = huella_engine::session_project(&app.store, &session_id)?;
    let points = huella_review::attention::attention(&review, &plan);
    let Assessment { evidence, triage, .. } =
        assess(&app.store, &review, &plan, &points, &app.activity, project.as_deref())?;
    let wanted: Vec<String> = plan
        .iter()
        .zip(&triage)
        .map(|(flow, triaged)| (flow.key(&review), triaged))
        .filter(|(key, triaged)| keys.contains(key) && huella_engine::guide::wanted(triaged))
        .map(|(key, _)| key)
        .collect();
    {
        let mut all = app.guiding.lock().unwrap_or_else(PoisonError::into_inner);
        let running = all.get(&session_id).is_some_and(|guiding| !guiding.pending.is_empty());
        let guiding = all.entry(session_id.clone()).or_default();
        if !running {
            guiding.errors.clear();
        }
        for key in wanted {
            if !guiding.pending.contains(&key) {
                guiding.pending.push(key);
            }
        }
        if running || guiding.pending.is_empty() {
            return Ok((202, json!(guiding)));
        }
    }
    let (guiding, db, model) = (Arc::clone(&app.guiding), app.db.clone(), app.model.clone());
    std::thread::spawn(move || {
        let setup =
            Store::open(&db).map_err(anyhow::Error::from).and_then(|store| Ok((store, crate::build_model(&model)?)));
        loop {
            let Some(key) = guiding
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&session_id)
                .and_then(|guiding| guiding.pending.first().cloned())
            else {
                break;
            };
            let outcome = match &setup {
                Ok((store, llm)) => plan
                    .iter()
                    .position(|flow| flow.key(&review) == key)
                    .context("el paso ya no está en la revisión")
                    .and_then(|index| {
                        huella_engine::guide::guide_step(
                            store,
                            &review,
                            &plan,
                            index,
                            &evidence[index],
                            &triage[index],
                            llm.as_ref(),
                        )
                        .map_err(anyhow::Error::from)
                    }),
                Err(error) => Err(anyhow::anyhow!("{error:#}")),
            };
            let mut all = guiding.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(state) = all.get_mut(&session_id) else { break };
            state.pending.retain(|pending| *pending != key);
            if let Err(error) = outcome {
                state.errors.push(format!("{key}: {error:#}"));
            }
        }
    });
    Ok((202, json!({"running": true})))
}

/// Marca o desmarca una comprobación de la guía de un paso.
fn check_guide(app: &mut App, session: &str, body: &str) -> Result<Value> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let key = request["key"].as_str().context("falta key")?;
    let index = request["index"].as_u64().and_then(|i| usize::try_from(i).ok()).context("falta index")?;
    let done = request["done"].as_bool().context("falta done")?;
    load_review(app, session)?;
    let (review, plan) = app.review.as_ref().context("revisión no cargada")?;
    start_reviewing(app, &review.session_id, plan.len())?;
    if !app.store.set_guide_check(&review.session_id, key, index, done)? {
        bail!("el paso no tiene guía");
    }
    Ok(json!({}))
}

/// Qué código del proyecto, fuera de la sesión, usa lo que cambió cada paso.
fn impact(app: &mut App, session: &str) -> Result<Value> {
    load_review(app, session)?;
    if let Some(cached) = &app.impact {
        return Ok(cached.clone());
    }
    let (review, plan) = app.review.as_ref().context("revisión no cargada")?;
    let project = huella_engine::session_project(&app.store, &review.session_id)?;
    let value = match project.as_deref().map(Path::new).filter(|path| path.is_dir()) {
        Some(project) => {
            json!({"project": project, "steps": huella_engine::impact::impact(project, review, plan)})
        }
        None => json!({"project": project, "steps": [], "missing": true}),
    };
    app.impact = Some(value.clone());
    Ok(value)
}

fn feedback(app: &mut App, session: &str) -> Result<Value> {
    load_review(app, session)?;
    let (review, plan) = app.review.as_ref().context("revisión no cargada")?;
    let feedback = huella_engine::review_loop::feedback(&app.store, review, plan)?;
    Ok(json!({"markdown": feedback.markdown, "items": feedback.items}))
}

/// Pregunta a la sesión: la responde un hilo con el modelo configurado y la
/// revisión la muestra como pendiente mientras tanto.
/// Pregunta sobre un trozo de código (`path`, `quote` y, si hay, `start`/`end`):
/// la responde un hilo con el modelo configurado, como las preguntas a la sesión.
fn ask_code(app: &mut App, body: &str) -> Result<(u16, Value)> {
    if let Some(problem) = crate::model_problem(&app.model) {
        bail!(problem);
    }
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let text = |key: &str| request[key].as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
    let path = PathBuf::from(text("path").context("falta el fichero")?);
    let question = text("question").context("la pregunta está vacía")?;
    let quote = request["quote"].as_str().unwrap_or_default().to_owned();
    let line = |key: &str| request[key].as_u64().and_then(|n| usize::try_from(n).ok());
    let lines = line("start").zip(line("end"));
    let key = huella_engine::normalize_path(&path, None);
    let pending = PendingCodeAsk { path: key, quote: quote.clone(), question: question.clone(), error: None };
    app.code_asking.lock().unwrap_or_else(PoisonError::into_inner).push(pending.clone());
    let (asking, db, model, sources) =
        (Arc::clone(&app.code_asking), app.db.clone(), app.model.clone(), app.sources.clone());
    std::thread::spawn(move || {
        let outcome = (|| -> Result<()> {
            let store = Store::open(&db)?;
            let llm = crate::build_model(&model)?;
            huella_engine::code_ask::ask_code(&store, Some(&sources), llm.as_ref(), &path, &quote, lines, &question)?;
            Ok(())
        })();
        let mut all = asking.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(index) =
            all.iter().position(|p| p.path == pending.path && p.question == pending.question && p.error.is_none())
        {
            match outcome {
                Ok(()) => {
                    all.remove(index);
                }
                Err(error) => all[index].error = Some(format!("{error:#}")),
            }
        }
    });
    Ok((202, json!({"asking": true})))
}

/// Preguntas sobre el código de un fichero, con sus líneas actuales, y las que
/// el modelo aún está respondiendo.
fn code_questions(app: &App, params: &HashMap<String, String>) -> Result<Value> {
    let path = PathBuf::from(non_empty(params.get("path")).context("falta el fichero")?);
    let key = huella_engine::normalize_path(&path, None);
    let questions: Vec<Value> = huella_engine::code_ask::questions_for(&app.store, &path)?
        .into_iter()
        .map(|located| {
            let q = located.question;
            json!({
                "id": q.id,
                "question": q.question,
                "answer": q.answer,
                "model": q.model,
                "created_at": q.created_at,
                "quote": q.quote,
                "lines": located.lines.map(|(start, end)| [start, end]),
            })
        })
        .collect();
    let pending: Vec<PendingCodeAsk> = app
        .code_asking
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|p| p.path == key)
        .cloned()
        .collect();
    Ok(json!({"questions": questions, "pending": pending, "ask_problem": crate::model_problem(&app.model)}))
}

fn ask(app: &mut App, session: &str, body: &str) -> Result<(u16, Value)> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let question = request["question"]
        .as_str()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .context("la pregunta está vacía")?
        .to_owned();
    let step = request["step"].as_u64().and_then(|s| usize::try_from(s).ok());
    if let Some(problem) = crate::model_problem(&app.model) {
        bail!(problem);
    }
    load_review(app, session)?;
    let (review, plan) = app.review.clone().context("revisión no cargada")?;
    let session_id = review.session_id.clone();
    let pending = PendingAsk { question: question.clone(), step, error: None };
    app.asking.lock().unwrap_or_else(PoisonError::into_inner).entry(session_id.clone()).or_default().push(pending);
    let (asking, db, model) = (Arc::clone(&app.asking), app.db.clone(), app.model.clone());
    std::thread::spawn(move || {
        let outcome = (|| -> Result<()> {
            let store = Store::open(&db)?;
            let extractor = crate::build_model(&model)?;
            huella_engine::review_loop::ask(&store, extractor.as_ref(), &review, &plan, step, &question)?;
            Ok(())
        })();
        let mut all = asking.lock().unwrap_or_else(PoisonError::into_inner);
        let list = all.entry(session_id).or_default();
        if let Some(index) = list.iter().position(|p| p.question == question && p.error.is_none()) {
            match outcome {
                Ok(()) => {
                    list.remove(index);
                }
                Err(error) => list[index].error = Some(format!("{error:#}")),
            }
        }
    });
    Ok((202, json!({"asking": true})))
}

fn mark(app: &mut App, session: &str, body: &str) -> Result<Value> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let (key, hash) = (request["key"].as_str().context("falta key")?, request["hash"].as_str().context("falta hash")?);
    load_review(app, session)?;
    let (session_id, steps) = app
        .review
        .as_ref()
        .map(|(review, plan)| (review.session_id.clone(), plan.len()))
        .context("revisión no cargada")?;
    // `verdict` (ok, change, discard o null) o, como antes, `reviewed`.
    let verdict = match request.get("verdict") {
        Some(Value::String(text)) => Some(huella_store::Verdict::parse(text).context("veredicto desconocido")?),
        Some(Value::Null) => None,
        _ => request["reviewed"].as_bool().unwrap_or(true).then_some(huella_store::Verdict::Ok),
    };
    app.store.set_step_verdict(&session_id, key, hash, verdict)?;
    start_reviewing(app, &session_id, steps)?;
    Ok(json!({}))
}

/// Carga la revisión de la sesión salvo que sea la última cargada.
fn load_review(app: &mut App, session: &str) -> Result<()> {
    let cached = app.review.as_ref().is_some_and(|(review, _)| review.session_id.starts_with(session));
    if !cached {
        let review = huella_engine::session_review(&app.sources, session)?;
        let plan = huella_review::flow::plan(&review);
        app.activity = huella_engine::evidence::session_activity(&app.sources, &review.session_id)?;
        app.impact = None;
        app.review = Some((review, plan));
    }
    Ok(())
}

fn status(app: &App) -> Result<Value> {
    let stats = app.store.stats()?;
    let semantic = match crate::embedder(None, None) {
        Some(embedder) => {
            let model = huella_decisions::Embedder::model_id(&embedder);
            json!({
                "model": model,
                "vectors": app.store.embedded(&model)?,
                "pending": app.store.pending_embeddings(&model, usize::MAX)?.len(),
            })
        }
        None => Value::Null,
    };
    Ok(json!({
        "stats": {
            "transcripts": stats.transcripts,
            "sessions": stats.sessions,
            "edits": stats.edits,
            "symbols": stats.anchored_symbols,
        },
        "db": app.db.display().to_string(),
        "agents": app.sources.names(),
        "semantic": semantic,
        "extractor": extractor_label(&app.model),
        "jobs": *app.jobs.lock().unwrap_or_else(PoisonError::into_inner),
    }))
}

fn open(app: &App, body: &str) -> Result<Value> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let path = request["path"].as_str().filter(|p| p.starts_with('/')).context("falta la ruta absoluta")?;
    if !Path::new(path).exists() {
        bail!("{path} ya no existe");
    }
    let line = request["line"].as_u64().and_then(|l| usize::try_from(l).ok()).unwrap_or(1);
    crate::open_in_editor(app.editor, None, Some((path.to_owned(), line)));
    Ok(json!({}))
}

/// Lanza una tarea en un hilo; responde 409 si ya está en marcha.
fn start_job(app: &App, kind: &str) -> (u16, Value) {
    if !JOBS.contains(&kind) {
        return (404, json!({"error": format!("tarea desconocida: {kind}")}));
    }
    {
        let mut jobs = app.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        if jobs.get(kind).is_some_and(|job| job.state == "running") {
            return (409, json!({"error": "ya está en marcha"}));
        }
        jobs.insert(kind.to_owned(), Job { state: "running", ..Job::default() });
    }
    let (jobs, kind) = (Arc::clone(&app.jobs), kind.to_owned());
    let (db, sources, model) = (app.db.clone(), app.sources.clone(), app.model.clone());
    std::thread::spawn(move || {
        let progress = |done: usize, total: usize, message: Option<String>| {
            let mut all = jobs.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(job) = all.get_mut(&kind) {
                (job.done, job.total) = (done, total);
                if message.is_some() {
                    job.message = message;
                }
            }
        };
        let outcome = run_job(&kind, &db, &sources, &model, &progress);
        let mut all = jobs.lock().unwrap_or_else(PoisonError::into_inner);
        let job = all.entry(kind).or_default();
        job.finished_at = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs());
        match outcome {
            Ok(result) => {
                job.state = "done";
                job.result = Some(result);
                job.message = None;
            }
            Err(error) => {
                job.state = "failed";
                job.message = Some(format!("{error:#}"));
            }
        }
    });
    (202, json!({"started": true}))
}

fn run_job(
    kind: &str,
    db: &Path,
    projects: &huella_engine::Sources,
    model: &ModelArgs,
    progress: &dyn Fn(usize, usize, Option<String>),
) -> Result<Value> {
    let mut store = Store::open(db)?;
    match kind {
        "index" => {
            let report = huella_engine::index(&mut store, projects)?;
            Ok(
                json!({"transcripts": report.transcripts, "updated": report.updated, "new_edits": report.new_edits, "failures": report.failures}),
            )
        }
        "embed" => {
            let embedder = crate::embedder(None, None)
                .context("define el modelo en [embed] de config.toml (o HUELLA_EMBED_MODEL) y reinicia huella ui")?;
            let report = huella_engine::embeddings::embed_pending(&mut store, &embedder, 100_000, |done, total| {
                progress(done, total, None);
            })?;
            if let Some(failure) = report.failure {
                bail!("el modelo falló tras {} vectores: {failure}", report.embedded);
            }
            Ok(json!({"embedded": report.embedded, "skipped": report.skipped}))
        }
        "refactors" => {
            let report = huella_engine::refactor_bench::refactor_benchmark(&store, 600)?;
            let rows: Vec<Value> = huella_engine::refactor_bench::Refactor::ALL
                .iter()
                .filter_map(|refactor| report.by_refactor.get(refactor).map(|t| (refactor, t)))
                .map(|(refactor, tally)| json!({"label": refactor.label(), "applicable": tally.applicable, "survived": tally.survived}))
                .collect();
            Ok(json!({
                "samples": report.samples,
                "survival": report.survival(),
                "refactors": rows,
                "probes": report.probes,
                "false_positives": report.false_positives,
            }))
        }
        "extract" => {
            let extractor = crate::build_model(model)?;
            huella_engine::index(&mut store, projects)?;
            let mut seen = 0;
            let report = huella_engine::extract_decisions(
                &mut store,
                projects,
                extractor.as_ref(),
                &huella_engine::ExtractionOptions::default(),
                |conversation| {
                    seen += 1;
                    let short = &conversation.session_id[..8.min(conversation.session_id.len())];
                    progress(seen, 0, Some(format!("sesión {short}")));
                },
            )?;
            Ok(json!({
                "extracted": report.extracted,
                "unchanged": report.unchanged,
                "decisions": report.decisions,
                "failures": report.failures,
            }))
        }
        other => bail!("tarea desconocida: {other}"),
    }
}

/// Un fichero de la interfaz. Las rutas de la aplicación son de la propia
/// página: todo lo que no es un fichero es `index.html`.
fn asset(path: &str) -> Reply {
    let file = path.trim_start_matches('/');
    let found = Assets::get(file).filter(|_| !file.is_empty()).map(|asset| (file, asset));
    let Some((name, asset)) = found.or_else(|| Assets::get("index.html").map(|asset| ("index.html", asset))) else {
        let message = "falta ui/dist: compila la interfaz con `pnpm --dir ui build` o usa `--dev` con Vite";
        return Reply { status: 503, content_type: "text/plain; charset=utf-8", cache: None, body: message.into() };
    };
    Reply {
        status: 200,
        content_type: mime(name),
        // Los ficheros de assets llevan su hash en el nombre.
        cache: name.starts_with("assets/").then_some("public, max-age=31536000, immutable"),
        body: asset.data.into_owned(),
    }
}

fn json_reply(status: u16, value: &Value) -> Reply {
    Reply {
        status,
        content_type: "application/json; charset=utf-8",
        cache: Some("no-store"),
        body: value.to_string().into_bytes(),
    }
}

fn decision_json(decision: &StoredDecision) -> Value {
    json!({
        "uid": decision.uid,
        "title": decision.title,
        "decision": decision.decision,
        "rationale": decision.rationale,
        "alternatives": decision.alternatives,
        "entities": decision.entities,
        "status": decision.status,
        "superseded_by": decision.superseded_by,
        "decided_at": decision.decided_at,
        "extractor": decision.extractor,
        "session_id": decision.session_id,
        "project": decision.project,
        "scope": decision.scope,
        "scope_path": decision.scope_path,
        "overridden": decision.overridden,
    })
}

fn entry_json(entry: &HistoryEntry) -> Value {
    json!({
        "edit_id": entry.edit_id,
        "session_id": entry.session_id,
        "at": entry.at,
        "removed": entry.role == SymbolRole::Removed,
        "kind": entry.kind,
        "prompt": entry.context.prompt,
        "rationale": entry.context.rationale,
    })
}

fn session_json(listing: &huella_engine::SessionListing) -> Value {
    let summary = &listing.summary;
    json!({
        "id": summary.id,
        "agent": summary.agent,
        "project": listing.project,
        "first_at": summary.first_at,
        "last_at": summary.last_at,
        "edits": summary.edits,
        "files": summary.files,
    })
}

fn scope_of(decision: &StoredDecision) -> &'static str {
    match decision.scope.as_str() {
        "user" => "user",
        "path" => "path",
        _ => "project",
    }
}

fn found_by_name(found_by: FoundBy) -> &'static str {
    match found_by {
        FoundBy::Words => "words",
        FoundBy::Meaning => "meaning",
        FoundBy::Both => "both",
    }
}

fn editor_name(editor: Editor) -> &'static str {
    match editor {
        Editor::Zed => "zed",
        Editor::Vscode => "vscode",
    }
}

/// Qué modelo extraería decisiones, para mostrarlo.
pub fn extractor_label(model: &ModelArgs) -> String {
    match (model.provider(), &model.model()) {
        (crate::Provider::Anthropic, model) => {
            format!("anthropic:{}", model.as_deref().unwrap_or(huella_decisions::anthropic::DEFAULT_MODEL))
        }
        (crate::Provider::OpenaiCompatible, Some(model)) => format!("openai-compatible:{model}"),
        (crate::Provider::OpenaiCompatible, None) => "openai-compatible (falta --model)".to_owned(),
    }
}

fn idle() -> Job {
    Job { state: "idle", ..Job::default() }
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty()).cloned()
}

fn header(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_owned())
}

fn content_type(value: &str) -> Header {
    raw_header("Content-Type", value)
}

fn raw_header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).expect("cabecera válida")
}

fn mime(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

/// `a=1&b=hola%20mundo` → {a: 1, b: hola mundo}.
fn query_params(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("zz"), 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings_are_decoded() {
        let params = query_params("q=rate+limit&project=%2FUsers%2Fana%2Fcanjea&x=%C3%B1&bad=%zz");
        assert_eq!(params["q"], "rate limit");
        assert_eq!(params["project"], "/Users/ana/canjea");
        assert_eq!(params["x"], "ñ");
        assert_eq!(params["bad"], "%zz");
    }

    #[test]
    fn unknown_files_fall_back_to_the_app_mime_types() {
        assert_eq!(mime("assets/index-1a2b.js"), "text/javascript; charset=utf-8");
        assert_eq!(mime("fonts/geist.woff2"), "font/woff2");
        assert_eq!(mime("raro"), "application/octet-stream");
    }

    #[test]
    fn asking_without_a_model_says_what_is_missing() {
        let _config = crate::config::TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let model = |name: Option<&str>| ModelArgs {
            provider: Some(crate::Provider::OpenaiCompatible),
            model: name.map(str::to_owned),
            base_url: None,
        };
        assert!(crate::model_problem(&model(None)).is_some_and(|problem| problem.contains("HUELLA_LLM_MODEL")));
        // Sin clave también vale: un servidor local no la pide.
        assert_eq!(crate::model_problem(&model(Some("qwen2.5:7b"))), None);
    }
}
