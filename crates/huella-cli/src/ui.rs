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
    pub projects: PathBuf,
    pub model: ModelArgs,
}

struct App {
    store: Store,
    db: PathBuf,
    projects: PathBuf,
    editor: Editor,
    token: String,
    allowed_hosts: Vec<String>,
    jobs: Jobs,
    model: ModelArgs,
    /// La última revisión cargada: reconstruirla relee las transcripciones.
    review: Option<(SessionReview, Vec<FlowStep>)>,
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

pub fn serve(options: Options) -> Result<()> {
    // El puerto pedido; si está ocupado (otra `huella ui` abierta), uno libre.
    let server = Server::http(("127.0.0.1", options.port))
        .or_else(|_| Server::http(("127.0.0.1", 0)))
        .map_err(|error| anyhow::anyhow!("no se pudo abrir el servidor local: {error}"))?;
    let port = server.server_addr().to_ip().context("dirección del servidor desconocida")?.port();
    let mut store = Store::open(&options.db).with_context(|| format!("abriendo {}", options.db.display()))?;
    huella_engine::index(&mut store, &options.projects)?;
    let token = crate::web::token("huella-ui");
    let mut allowed_hosts = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let page_port = if options.dev {
        allowed_hosts.extend([format!("127.0.0.1:{VITE_PORT}"), format!("localhost:{VITE_PORT}")]);
        VITE_PORT
    } else {
        port
    };
    let url = format!("http://localhost:{page_port}/?t={token}");
    let jobs: Jobs = Arc::new(Mutex::new(JOBS.iter().map(|kind| ((*kind).to_owned(), idle())).collect()));
    let mut app = App {
        store,
        db: options.db,
        projects: options.projects,
        editor: options.editor,
        token,
        allowed_hosts,
        jobs,
        model: options.model,
        review: None,
    };
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
    if !path.starts_with("/api/") {
        return serve_asset(request, path);
    }
    if header(&request, "X-Huella-Token").as_deref() != Some(app.token.as_str()) {
        return respond_json(request, 401, &json!({"error": "token no válido: abre la URL que imprime `huella ui`"}));
    }
    let method = request.method().clone();
    let mut body = String::new();
    if method == Method::Post {
        std::io::Read::read_to_string(request.as_reader(), &mut body)?;
    }
    let params = query_params(query);
    match api(app, &method, path, &params, &body) {
        Ok((status, value)) => respond_json(request, status, &value),
        Err(error) => respond_json(request, 500, &json!({"error": format!("{error:#}")})),
    }
}

fn api(
    app: &mut App,
    method: &Method,
    path: &str,
    params: &HashMap<String, String>,
    body: &str,
) -> Result<(u16, Value)> {
    let segments: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
    let value = match (method, segments.as_slice()) {
        (Method::Get, ["overview"]) => overview(app)?,
        (Method::Get, ["search"]) => search(app, params)?,
        (Method::Get, ["decisions"]) => decisions(app, params)?,
        (Method::Post, ["decisions", uid, "status"]) => set_status(app, uid, body)?,
        (Method::Post, ["adr"]) => export_adr(app, body)?,
        (Method::Get, ["symbol"]) => symbol(app, params)?,
        (Method::Get, ["sessions"]) => sessions(app, params)?,
        (Method::Get, ["review", session]) => review(app, session)?,
        (Method::Post, ["review", session, "mark"]) => mark(app, session, body)?,
        (Method::Get, ["status"]) => status(app)?,
        (Method::Get, ["jobs"]) => json!({"jobs": *app.jobs.lock().unwrap_or_else(PoisonError::into_inner)}),
        (Method::Post, ["jobs", kind]) => return Ok(start_job(app, kind)),
        (Method::Post, ["open"]) => open(app, body)?,
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

fn sessions(app: &App, params: &HashMap<String, String>) -> Result<Value> {
    let limit = params.get("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
    let listed = huella_engine::list_sessions(&app.store, limit)?;
    Ok(json!({"sessions": listed.iter().map(session_json).collect::<Vec<_>>()}))
}

fn review(app: &mut App, session: &str) -> Result<Value> {
    load_review(app, session)?;
    let (review, plan) = app.review.as_ref().context("revisión no cargada")?;
    let points = huella_review::attention::attention(review, plan);
    let narrations = huella_engine::narration::narrations(&app.store, review, plan, &points)?;
    let mut value = crate::web::payload(review, plan, &app.store.review_marks(&review.session_id)?, app.editor);
    value["narrations"] = serde_json::to_value(&narrations)?;
    value["project"] = json!(huella_engine::session_project(&app.store, &review.session_id)?);
    Ok(value)
}

fn mark(app: &mut App, session: &str, body: &str) -> Result<Value> {
    let request: Value = serde_json::from_str(body).context("cuerpo JSON no válido")?;
    let (key, hash) = (request["key"].as_str().context("falta key")?, request["hash"].as_str().context("falta hash")?);
    load_review(app, session)?;
    let session_id = app.review.as_ref().map(|(review, _)| review.session_id.clone()).context("revisión no cargada")?;
    app.store.set_review_mark(&session_id, key, hash, request["reviewed"].as_bool().unwrap_or(true))?;
    Ok(json!({}))
}

/// Carga la revisión de la sesión salvo que sea la última cargada.
fn load_review(app: &mut App, session: &str) -> Result<()> {
    let cached = app.review.as_ref().is_some_and(|(review, _)| review.session_id.starts_with(session));
    if !cached {
        let review = huella_engine::session_review(&app.projects, session)?;
        let plan = huella_review::flow::plan(&review);
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
        "projects_dir": app.projects.display().to_string(),
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
    let (db, projects, model) = (app.db.clone(), app.projects.clone(), app.model.clone());
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
        let outcome = run_job(&kind, &db, &projects, model, &progress);
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
    projects: &Path,
    model: ModelArgs,
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
                .context("define HUELLA_EMBED_MODEL (y HUELLA_EMBED_URL si no es Ollama local) y reinicia huella ui")?;
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

fn serve_asset(request: Request, path: &str) -> Result<()> {
    let file = path.trim_start_matches('/');
    // Las rutas de la aplicación son de la propia página: todo lo demás es index.html.
    let found = Assets::get(file).filter(|_| !file.is_empty()).map(|asset| (file, asset));
    let Some((name, asset)) = found.or_else(|| Assets::get("index.html").map(|asset| ("index.html", asset))) else {
        let message = "falta ui/dist: compila la interfaz con `pnpm --dir ui build` o usa `--dev` con Vite";
        return Ok(request.respond(Response::from_string(message).with_status_code(503))?);
    };
    let mut response = Response::from_data(asset.data.into_owned()).with_header(content_type(mime(name)));
    if name.starts_with("assets/") {
        // Los ficheros de assets llevan su hash en el nombre.
        response = response.with_header(raw_header("Cache-Control", "public, max-age=31536000, immutable"));
    }
    Ok(request.respond(response)?)
}

fn respond_json(request: Request, status: u16, value: &Value) -> Result<()> {
    let response = Response::from_string(value.to_string())
        .with_status_code(status)
        .with_header(content_type("application/json; charset=utf-8"))
        .with_header(raw_header("Cache-Control", "no-store"));
    Ok(request.respond(response)?)
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
fn extractor_label(model: &ModelArgs) -> String {
    match (model.provider, &model.model) {
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
}
