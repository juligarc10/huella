//! `huella`: CLI de Huella.

mod mcp;
mod report;
mod review;
mod sample;
mod search;
mod web;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use huella_anchoring::{AnchorReport, Language, anchor_edit, extract_symbols};
use huella_claude_code::{discover_transcripts, read_transcript};
use huella_core::FileEdit;
use huella_decisions::JsonModel;
use huella_decisions::anthropic::Anthropic;
use huella_decisions::openai::OpenAiCompatible;
use huella_engine::ExtractionOptions;
use huella_store::{DecisionFilter, Store};
use serde::Serialize;

use crate::report::Summary;

#[derive(Parser)]
#[command(name = "huella", version, about = "Ancla las sesiones de agentes de IA a los símbolos del código")]
struct Cli {
    /// Base de datos (por defecto `$HUELLA_DB` o la carpeta de datos del usuario).
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Mide cuánta memoria sobrevive a refactors hechos fuera de las sesiones
    /// (renombrar, mover), aplicados en memoria a los anclajes reales.
    Refactors {
        /// Anclajes vivos a probar como mucho, repartidos por todos los ficheros.
        #[arg(long, default_value_t = 600)]
        limit: usize,
    },
    /// Ancla todas las ediciones de las transcripciones de Claude Code y mide la tasa de anclaje.
    Scan {
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
        /// Escribe el resultado de cada edición en este fichero JSON.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Escribe una muestra de N anclajes en markdown para validarlos a mano.
        #[arg(long, value_name = "N", requires = "sample_out")]
        sample: Option<usize>,
        /// Fichero markdown de la muestra.
        #[arg(long, value_name = "FICHERO")]
        sample_out: Option<PathBuf>,
        /// Desplazamiento de la muestra: valores distintos dan muestras disjuntas.
        #[arg(long, default_value_t = 0)]
        sample_offset: usize,
    },
    /// Lista los símbolos que Huella extrae de un fichero.
    Symbols { file: PathBuf },
    /// Indexa en la base de datos lo nuevo de las transcripciones.
    Index {
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Muestra qué se habló del símbolo en una posición: `huella why src/app.ts:42`.
    Why {
        /// Fichero y línea, separados por dos puntos.
        #[arg(value_name = "FICHERO:LÍNEA")]
        location: String,
        /// Número máximo de ediciones a mostrar.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Busca en la memoria: peticiones, explicaciones del agente, ficheros,
    /// símbolos y decisiones. Sin tildes ni mayúsculas que importen.
    Search {
        /// Palabras a buscar; la última vale también como prefijo.
        #[arg(required = true)]
        words: Vec<String>,
        /// Solo lo de este proyecto (carpeta).
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long, default_value_t = 15)]
        limit: usize,
        /// Incluir decisiones reemplazadas, rechazadas o solo propuestas.
        #[arg(long)]
        all: bool,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Decisiones de proyecto extraídas de las conversaciones.
    Decisions {
        #[command(subcommand)]
        action: DecisionsCommand,
    },
    /// Revisión de lo que cambió en una sesión, con el porqué de cada bloque.
    Review {
        #[command(subcommand)]
        action: ReviewCommand,
    },
    /// Servidor MCP por stdin/stdout para que los agentes consulten la memoria.
    Mcp {
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Servidor LSP para el editor (hover y `CodeLens`), por stdin/stdout.
    Lsp {
        /// Carpeta de transcripciones a indexar en segundo plano (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
        /// Segundos entre indexados en segundo plano; 0 lo desactiva.
        #[arg(long, default_value_t = 30)]
        index_every: u64,
    },
}

#[derive(Subcommand)]
enum DecisionsCommand {
    /// Extrae con un modelo las decisiones de las sesiones nuevas o que han crecido.
    Extract {
        #[command(flatten)]
        model: ModelArgs,
        /// Número máximo de sesiones a procesar.
        #[arg(long)]
        limit: Option<usize>,
        /// Caracteres por trozo de diálogo (unos 3,5 por token): ajústalo al contexto del modelo.
        #[arg(long, default_value_t = huella_decisions::DEFAULT_CHUNK_CHARS)]
        chunk_chars: usize,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Lista las decisiones vigentes.
    List {
        /// Solo las que mencionan esta tecnología o concepto (`rust`, `sqlite`…).
        #[arg(long)]
        entity: Option<String>,
        /// Solo las de este proyecto (ruta del repositorio).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Solo las de este alcance: `user` (tus preferencias, valen en todos los
        /// proyectos), `project` o `path` (una carpeta o fichero).
        #[arg(long, value_parser = ["user", "project", "path"])]
        scope: Option<String>,
        /// Incluye las reemplazadas, rechazadas y solo propuestas.
        #[arg(long)]
        all: bool,
    },
    /// Corrige a mano el estado de una decisión; sobrevive a volver a extraer.
    Set {
        /// Id de la decisión (el que muestra `huella decisions list`) o su comienzo.
        id: String,
        /// `accepted`, `rejected`, `superseded`, `proposed`, o `auto` para volver al extraído.
        #[arg(value_parser = ["accepted", "rejected", "superseded", "proposed", "auto"])]
        status: String,
    },
}

#[derive(Subcommand)]
enum ReviewCommand {
    /// Sesiones recientes con ediciones.
    List {
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    /// Activa la revisión de una sesión en el editor: sus pasos pendientes
    /// aparecen como diagnosis y una acción de código los marca revisados.
    Start {
        /// Id de la sesión o su comienzo.
        session: String,
        /// Abre en el editor el proyecto de la sesión, en el primer paso pendiente.
        #[arg(long)]
        open: bool,
        /// Editor que abre `--open`.
        #[arg(long, value_enum, default_value_t = web::Editor::Zed)]
        editor: web::Editor,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Revisa la sesión en Zed con su vista de cambios de git: prepara un
    /// worktree donde el diff es exactamente lo que hizo la sesión, activa la
    /// revisión y lo abre. Preparar (stage) un fichero entero lo marca revisado.
    Zed {
        /// Id de la sesión o su comienzo.
        session: String,
        /// Termina: borra el worktree y su rama temporal.
        #[arg(long)]
        done: bool,
        /// No abre Zed; solo prepara el worktree.
        #[arg(long)]
        no_open: bool,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Quita la revisión activa del editor.
    Stop,
    /// Genera con un modelo el resumen de cada turno, notas por paso y riesgos.
    Narrate {
        /// Id de la sesión o su comienzo.
        session: String,
        #[command(flatten)]
        model: ModelArgs,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Abre la revisión guiada de una sesión en el navegador.
    Open {
        /// Id de la sesión o su comienzo.
        session: String,
        /// Editor al que llevan los enlaces «Abrir en…».
        #[arg(long, value_enum, default_value_t = web::Editor::Zed)]
        editor: web::Editor,
        /// Puerto local (0 = uno libre).
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// No abre el navegador; solo imprime la URL.
        #[arg(long)]
        no_browser: bool,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Cambios netos de una sesión, en el orden del flujo, con su origen.
    Show {
        /// Id de la sesión o su comienzo (los 8 primeros caracteres bastan).
        session: String,
        /// Solo los ficheros cuya ruta contiene este texto.
        #[arg(long)]
        file: Option<String>,
        /// Fichero a fichero en lugar de en el orden del flujo.
        #[arg(long)]
        by_file: bool,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
}

/// Qué modelo usar para las tareas que lo necesitan.
#[derive(clap::Args)]
struct ModelArgs {
    /// Proveedor del modelo.
    #[arg(long, value_enum, default_value_t = Provider::Anthropic)]
    provider: Provider,
    /// Modelo (por defecto claude-haiku-5-5 con Anthropic; obligatorio con openai-compatible).
    #[arg(long)]
    model: Option<String>,
    /// URL base de la API (por defecto la oficial, o Ollama local con openai-compatible).
    #[arg(long)]
    base_url: Option<String>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Provider {
    /// API de Claude (`ANTHROPIC_API_KEY`).
    Anthropic,
    /// Cualquier servidor con la API de chat de `OpenAI`: Ollama, LM Studio, vLLM…
    /// (clave opcional en `HUELLA_LLM_API_KEY`).
    OpenaiCompatible,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Scan { projects, json, sample, sample_out, sample_offset } => {
            let projects = projects_dir(projects)?;
            let sample = sample.zip(sample_out).map(|(size, path)| SampleRequest { size, offset: sample_offset, path });
            scan(&projects, json.as_deref(), sample)
        }
        Command::Symbols { file } => print_symbols(&file),
        Command::Refactors { limit } => refactors(&open_store(cli.db)?, limit),
        Command::Index { projects } => index(&mut open_store(cli.db)?, &projects_dir(projects)?),
        Command::Search { words, project, limit, all, projects } => {
            let mut store = open_store(cli.db)?;
            huella_engine::index(&mut store, &projects_dir(projects)?)?;
            let project =
                project.map(|path| huella_engine::normalize_path(&fs::canonicalize(&path).unwrap_or(path), None));
            let query = huella_store::SearchQuery { text: words.join(" "), project, limit, include_inactive: all };
            emit(&search::render_hits(&store.search(&query)?));
            Ok(())
        }
        Command::Why { location, limit } => why(&open_store(cli.db)?, &location, limit),
        Command::Decisions { action: DecisionsCommand::Extract { model, limit, chunk_chars, projects } } => {
            let extractor = build_model(model)?;
            let mut store = open_store(cli.db)?;
            let projects = projects_dir(projects)?;
            // El proyecto de cada sesión sale de los ficheros que editó: hay que indexarlos antes.
            huella_engine::index(&mut store, &projects)?;
            extract(&mut store, &projects, extractor.as_ref(), &ExtractionOptions { limit, chunk_chars })
        }
        Command::Decisions { action: DecisionsCommand::List { entity, project, scope, all } } => {
            let project = project
                .map(|path| fs::canonicalize(&path).unwrap_or(path))
                .map(|path| huella_engine::normalize_path(&path, None));
            let filter = DecisionFilter { entity, project, scope, include_inactive: all };
            list_decisions(&open_store(cli.db)?, &filter)
        }
        Command::Decisions { action: DecisionsCommand::Set { id, status } } => {
            let store = open_store(cli.db)?;
            let found = store.decisions_by_uid(&id)?;
            let decision = match found.as_slice() {
                [one] => one,
                [] => bail!("ninguna decisión tiene un id que empiece por «{id}»"),
                many => bail!(
                    "«{id}» coincide con varias decisiones: {}",
                    many.iter().map(|d| format!("{} ({})", d.uid, d.title)).collect::<Vec<_>>().join(", ")
                ),
            };
            let status = (status != "auto").then_some(status);
            store.set_decision_status(&decision.uid, status.as_deref())?;
            let now = &store.decisions_by_uid(&decision.uid)?[0];
            println!("«{}»: {}", now.title, search::status_label(&now.status));
            Ok(())
        }
        Command::Review { action } => review_command(cli.db, action),
        Command::Mcp { projects } => {
            let mut context = mcp::Context { store: open_store(cli.db)?, projects: projects_dir(projects)? };
            mcp::serve(&mut context)
        }
        Command::Lsp { projects, index_every } => {
            let db = cli
                .db
                .or_else(huella_engine::default_db_path)
                .context("no se encontró dónde guardar datos; usa --db")?;
            let projects = if index_every == 0 { None } else { Some(projects_dir(projects)?) };
            let config = huella_lsp::Config { db, projects, index_every: Duration::from_secs(index_every.max(1)) };
            huella_lsp::serve_stdio(&config)?;
            Ok(())
        }
    }
}

/// Subcomandos de `huella review`.
fn review_command(db: Option<PathBuf>, action: ReviewCommand) -> Result<()> {
    match action {
        ReviewCommand::List { limit } => {
            let mut store = open_store(db)?;
            huella_engine::index(&mut store, &projects_dir(None)?)?;
            emit(&review::render_sessions(&huella_engine::list_sessions(&store, limit)?));
            Ok(())
        }
        ReviewCommand::Start { session, open, editor, projects } => {
            let mut store = open_store(db)?;
            let projects = projects_dir(projects)?;
            huella_engine::index(&mut store, &projects)?;
            let review = huella_engine::session_review(&projects, &session)?;
            let plan = huella_review::flow::plan(&review);
            store.set_active_review(Some(&review.session_id))?;
            println!(
                "Revisión activa: sesión {} ({} pasos). Los editores con Huella la muestran como diagnosis \
                 en unos segundos; `huella review stop` la quita.",
                &review.session_id[..8.min(review.session_id.len())],
                plan.len()
            );
            let project = huella_engine::session_project(&store, &review.session_id)?;
            match &project {
                Some(project) => println!("Proyecto: {project} (ábrelo en el editor para ver la revisión)"),
                None => println!("La sesión no tiene un proyecto claro: abre la carpeta de sus ficheros."),
            }
            if open {
                open_in_editor(editor, project.as_deref(), first_pending(&store, &review, &plan)?);
            }
            Ok(())
        }
        ReviewCommand::Zed { session, done, no_open, projects } => review_zed(db, &session, done, !no_open, projects),
        ReviewCommand::Stop => {
            open_store(db)?.set_active_review(None)?;
            println!("Sin revisión activa.");
            Ok(())
        }
        ReviewCommand::Narrate { session, model, projects } => {
            let model = build_model(model)?;
            let store = open_store(db)?;
            let review = huella_engine::session_review(&projects_dir(projects)?, &session)?;
            let plan = huella_review::flow::plan(&review);
            let points = huella_review::attention::attention(&review, &plan);
            println!("Narrando la revisión con {}…", model.model_id());
            let report = huella_engine::narration::narrate(&store, &review, &plan, &points, model.as_ref(), |turn| {
                println!("  · turno {}", turn + 1);
            })?;
            println!("Hecho: {} turnos narrados, {} ya estaban al día", report.narrated, report.cached);
            for failure in &report.failures {
                eprintln!("aviso: {failure}");
            }
            Ok(())
        }
        ReviewCommand::Open { session, editor, port, no_browser, projects } => {
            let store = open_store(db)?;
            let review = huella_engine::session_review(&projects_dir(projects)?, &session)?;
            let plan = huella_review::flow::plan(&review);
            // La misma revisión aparece también en el editor mientras se hace en la web.
            store.set_active_review(Some(&review.session_id))?;
            web::serve(&store, &review, &plan, &web::Options { editor, port, open_browser: !no_browser })
        }
        ReviewCommand::Show { session, file, by_file, projects } => {
            let review = huella_engine::session_review(&projects_dir(projects)?, &session)?;
            if by_file {
                emit(&review::render_review(&review, file.as_deref()));
            } else {
                let plan = huella_review::flow::plan(&review);
                let points = huella_review::attention::attention(&review, &plan);
                let narrations = huella_engine::narration::narrations(&open_store(db)?, &review, &plan, &points)?;
                emit(&review::render_flow(&review, &plan, &points, &narrations, file.as_deref()));
            }
            Ok(())
        }
    }
}

/// `huella review zed`: prepara el worktree de revisión y lo abre en Zed, o lo
/// borra con `--done`.
fn review_zed(db: Option<PathBuf>, session: &str, done: bool, open: bool, projects: Option<PathBuf>) -> Result<()> {
    let mut store = open_store(db)?;
    let projects = projects_dir(projects)?;
    huella_engine::index(&mut store, &projects)?;
    let review = huella_engine::session_review(&projects, session)?;
    let short = &review.session_id[..8.min(review.session_id.len())];
    if done {
        if huella_engine::workspace::remove_workspace(&store, &review.session_id)? {
            println!("Worktree de revisión de {short} borrado (las marcas se conservan).");
        } else {
            println!("La sesión {short} no tenía worktree de revisión.");
        }
        if store.active_review()?.as_deref() == Some(review.session_id.as_str()) {
            store.set_active_review(None)?;
        }
        return Ok(());
    }
    let prepared = huella_engine::workspace::prepare_workspace(&store, &review)?;
    let plan = huella_review::flow::plan(&review);
    store.set_active_review(Some(&review.session_id))?;
    let workspace = &prepared.workspace;
    println!(
        "{} {} ({} ficheros, rama {})",
        if prepared.reused { "Worktree de revisión ya preparado:" } else { "Worktree de revisión preparado:" },
        workspace.path,
        prepared.files,
        workspace.branch
    );
    for path in &prepared.outside {
        println!("  fuera del repositorio, no incluido: {path}");
    }
    println!(
        "En Zed: la vista de cambios de git (Project Diff) muestra lo que hizo la sesión {short}; \
         prepara (stage) un fichero para marcarlo revisado. Huella explica cada paso con avisos.\n\
         Al terminar: huella review zed {short} --done"
    );
    if open {
        let file = first_pending(&store, &review, &plan)?.map(|(path, line)| {
            let in_workspace = path
                .strip_prefix(&workspace.repo)
                .map_or(path.clone(), |relative| format!("{}{relative}", workspace.path));
            (in_workspace, line)
        });
        open_in_editor(web::Editor::Zed, Some(&workspace.path), file);
    }
    Ok(())
}

/// Fichero y línea del primer paso sin revisar.
fn first_pending(
    store: &huella_store::Store,
    review: &huella_review::SessionReview,
    plan: &[huella_review::flow::FlowStep],
) -> Result<Option<(String, usize)>> {
    let marks = store.review_marks(&review.session_id)?;
    Ok(plan
        .iter()
        .find(|step| marks.get(&step.key(review)) != Some(&step.content_hash()))
        .map(|step| (review.files[step.file].path.clone(), step.symbol.as_ref().map_or(1, |s| s.start_line))))
}

/// `huella refactors`: el criterio de salida de la F3.
fn refactors(store: &huella_store::Store, limit: usize) -> Result<()> {
    use huella_engine::refactor_bench::{Refactor, ratio};
    let started = std::time::Instant::now();
    let report = huella_engine::refactor_bench::refactor_benchmark(store, limit)?;
    println!("Anclajes vivos probados: {} ({:.1} s)\n", report.samples, started.elapsed().as_secs_f64());
    for refactor in Refactor::ALL {
        let Some(tally) = report.by_refactor.get(&refactor) else { continue };
        println!(
            "  {:<34} {:>5} de {:>5}  {:>5.1} %",
            refactor.label(),
            tally.survived,
            tally.applicable,
            100.0 * ratio(tally.survived, tally.applicable)
        );
        for lost in &tally.lost_examples {
            println!("      perdido: {lost}");
        }
    }
    let survival = report.survival();
    println!(
        "\nSobreviven: {:.1} % (criterio: más del 80 %) {}",
        100.0 * survival,
        if survival > 0.8 { "✓" } else { "✗" }
    );
    println!(
        "Falsos positivos: {} de {} sondas ({:.1} %): código ajeno en el lugar de una función borrada",
        report.false_positives,
        report.probes,
        100.0 * ratio(report.false_positives, report.probes)
    );
    for example in &report.false_positive_examples {
        println!("      falso positivo: {example}");
    }
    Ok(())
}

/// Abre el proyecto y, si hay, el fichero del primer paso pendiente en su línea.
fn open_in_editor(editor: web::Editor, project: Option<&str>, file: Option<(String, usize)>) {
    let mut command = std::process::Command::new(match editor {
        web::Editor::Zed => "zed",
        web::Editor::Vscode => "code",
    });
    command.args(project);
    if let Some((path, line)) = file {
        if matches!(editor, web::Editor::Vscode) {
            command.arg("-g");
        }
        command.arg(format!("{path}:{line}"));
    }
    if let Err(error) = command.spawn() {
        eprintln!("No se pudo abrir el editor ({error}); ¿está su comando en el PATH?");
    }
}

/// Escribe en la salida estándar sin fallar si quien lee la cierra antes
/// (`huella review show … | head`).
fn emit(text: &str) {
    use std::io::Write as _;
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

fn projects_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    explicit.or_else(huella_engine::default_projects_dir).context("no se encontró la carpeta personal; usa --projects")
}

fn open_store(explicit: Option<PathBuf>) -> Result<Store> {
    let path =
        explicit.or_else(huella_engine::default_db_path).context("no se encontró dónde guardar datos; usa --db")?;
    Store::open(&path).with_context(|| format!("abriendo {}", path.display()))
}

fn index(store: &mut Store, projects: &Path) -> Result<()> {
    let started = Instant::now();
    let report = huella_engine::index(store, projects)?;
    let stats = store.stats()?;
    println!(
        "Indexado en {:.1} s: {} transcripciones, {} con novedades, {} ediciones nuevas{}",
        started.elapsed().as_secs_f64(),
        report.transcripts,
        report.updated,
        report.new_edits,
        if report.restarted > 0 {
            format!(" ({} releídas desde el principio)", report.restarted)
        } else {
            String::new()
        }
    );
    for failure in &report.failures {
        eprintln!("aviso: {failure}");
    }
    println!(
        "Memoria: {} sesiones, {} ediciones, {} símbolos con historia",
        stats.sessions, stats.edits, stats.anchored_symbols
    );
    Ok(())
}

fn why(store: &Store, location: &str, limit: usize) -> Result<()> {
    let (file, line) = location
        .rsplit_once(':')
        .and_then(|(file, line)| Some((file, line.parse::<usize>().ok()?)))
        .context("usa el formato FICHERO:LÍNEA, por ejemplo src/app.ts:42")?;
    let path = fs::canonicalize(file).with_context(|| format!("no se encontró {file}"))?;
    let source = fs::read_to_string(&path).with_context(|| format!("leyendo {}", path.display()))?;
    match huella_engine::memory_at(store, &path, &source, line)? {
        Some(memory) => print!("{}", huella_engine::render_markdown(&memory, limit)),
        None => println!("Huella no recuerda ninguna sesión que tocara {file}:{line}."),
    }
    Ok(())
}

fn build_model(args: ModelArgs) -> Result<Box<dyn JsonModel>> {
    let ModelArgs { provider, model, base_url } = args;
    Ok(match provider {
        Provider::Anthropic => Box::new(Anthropic::from_env(model, base_url)?),
        Provider::OpenaiCompatible => {
            let model = model.context("con openai-compatible indica el modelo con --model (p. ej. qwen2.5:7b)")?;
            Box::new(OpenAiCompatible::new(base_url, model, std::env::var("HUELLA_LLM_API_KEY").ok()))
        }
    })
}

fn extract(store: &mut Store, projects: &Path, extractor: &dyn JsonModel, options: &ExtractionOptions) -> Result<()> {
    let started = Instant::now();
    println!("Extrayendo decisiones con {}…", extractor.model_id());
    let report = huella_engine::extract_decisions(store, projects, extractor, options, |conversation| {
        let first =
            conversation.turns.first().map(|turn| turn.text.split_whitespace().take(8).collect::<Vec<_>>().join(" "));
        println!(
            "  · sesión {} ({} mensajes): {}…",
            &conversation.session_id[..8.min(conversation.session_id.len())],
            conversation.turns.len(),
            first.unwrap_or_default()
        );
    })?;
    println!(
        "Hecho en {:.0} s: {} sesiones extraídas ({} llamadas al modelo), {} sin cambios, {} decisiones nuevas",
        started.elapsed().as_secs_f64(),
        report.extracted,
        report.calls,
        report.unchanged,
        report.decisions
    );
    for failure in &report.failures {
        eprintln!("aviso: {failure}");
    }
    if !report.failures.is_empty() {
        bail!("{} sesiones fallaron; se reintentarán en la próxima extracción", report.failures.len());
    }
    Ok(())
}

fn list_decisions(store: &Store, filter: &DecisionFilter) -> Result<()> {
    let decisions = store.decisions(filter)?;
    if decisions.is_empty() {
        println!("No hay decisiones que mostrar. Extráelas con `huella decisions extract`.");
        return Ok(());
    }
    for decision in &decisions {
        let date = decision.decided_at.as_deref().and_then(|at| at.get(..10)).unwrap_or("sin fecha");
        let status = match (decision.status.as_str(), &decision.superseded_by) {
            ("superseded", Some(by)) => format!(" · reemplazada por «{by}»"),
            ("accepted", _) => String::new(),
            (other, _) => format!(" · {}", search::status_label(other)),
        };
        let scope = match (decision.scope.as_str(), &decision.scope_path) {
            ("user", _) => " · tuya, en todos los proyectos".to_owned(),
            ("path", Some(path)) => format!(" · solo {path}"),
            _ => String::new(),
        };
        let by_hand = if decision.overridden { " (a mano)" } else { "" };
        println!("● {} — {date}{status}{by_hand}{scope}  [{}]", decision.title, decision.uid);
        if !decision.entities.is_empty() {
            println!("  [{}]", decision.entities.join(", "));
        }
        println!("  Decisión: {}", decision.decision);
        println!("  Por qué: {}", decision.rationale);
        for alternative in &decision.alternatives {
            println!("  Descartado: {alternative}");
        }
        if let Some(project) = &decision.project {
            println!("  Proyecto: {project}");
        }
        println!();
    }
    println!("{} decisiones", decisions.len());
    Ok(())
}

struct SampleRequest {
    size: usize,
    offset: usize,
    path: PathBuf,
}

fn scan(projects: &Path, json_out: Option<&Path>, sample: Option<SampleRequest>) -> Result<()> {
    let transcripts = discover_transcripts(projects)?;
    if transcripts.is_empty() {
        bail!("no hay transcripciones .jsonl en {}", projects.display());
    }
    let read_current = |path: &Path| fs::read_to_string(path).ok();
    let mut summary = Summary::default();
    let mut results: Vec<(FileEdit, AnchorReport)> = Vec::new();
    for path in &transcripts {
        let parsed = read_transcript(path)?;
        summary.add_parse_stats(&parsed.stats);
        for edit in parsed.edits {
            let report = anchor_edit(&edit, read_current);
            summary.add(&edit, &report);
            results.push((edit, report));
        }
    }
    print!("{}", summary.render());

    if let Some(path) = json_out {
        let records: Vec<EditRecord> = results.iter().map(EditRecord::from).collect();
        fs::write(path, serde_json::to_string_pretty(&records)?)
            .with_context(|| format!("escribiendo {}", path.display()))?;
        println!("\nDetalle por edición: {}", path.display());
    }
    if let Some(request) = sample {
        let chosen = sample::select(&results, request.size, request.offset);
        let path = &request.path;
        fs::write(path, sample::render(&chosen, read_current))
            .with_context(|| format!("escribiendo {}", path.display()))?;
        println!("Muestra para validar a mano ({} casos): {}", chosen.len(), path.display());
    }
    Ok(())
}

fn print_symbols(file: &Path) -> Result<()> {
    let language = Language::from_path(file).with_context(|| format!("lenguaje no soportado: {}", file.display()))?;
    let source = fs::read_to_string(file).with_context(|| format!("leyendo {}", file.display()))?;
    let extraction = extract_symbols(language, &source)?;
    for symbol in &extraction.symbols {
        let indent = "  ".repeat(symbol.depth);
        println!(
            "{:>5}-{:<5} {indent}{} ({:?}) {}",
            symbol.start_line, symbol.end_line, symbol.qualified_name, symbol.kind, symbol.body_hash
        );
    }
    if extraction.has_syntax_errors {
        eprintln!("aviso: el fichero tiene errores de sintaxis; algunos símbolos pueden faltar");
    }
    Ok(())
}

/// Ruta relativa al directorio de la sesión cuando es posible.
pub(crate) fn display_path(edit: &FileEdit) -> String {
    edit.origin
        .cwd
        .as_deref()
        .and_then(|cwd| edit.path.strip_prefix(cwd).ok())
        .unwrap_or(&edit.path)
        .display()
        .to_string()
}

#[derive(Serialize)]
struct EditRecord<'a> {
    session_id: &'a str,
    event_id: &'a str,
    timestamp: Option<&'a str>,
    path: String,
    language: Option<&'static str>,
    kind: huella_core::EditKind,
    changed_lines: &'a [(usize, usize)],
    syntax_errors: bool,
    anchor: &'a huella_core::AnchorOutcome,
}

impl<'a> From<&'a (FileEdit, AnchorReport)> for EditRecord<'a> {
    fn from((edit, report): &'a (FileEdit, AnchorReport)) -> Self {
        Self {
            session_id: &edit.origin.session_id,
            event_id: &edit.origin.event_id,
            timestamp: edit.origin.timestamp.as_deref(),
            path: display_path(edit),
            language: report.language.map(Language::name),
            kind: edit.kind,
            changed_lines: &report.changed_lines,
            syntax_errors: report.has_syntax_errors,
            anchor: &report.outcome,
        }
    }
}
