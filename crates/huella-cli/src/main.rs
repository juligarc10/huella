//! `huella`: CLI de Huella.

mod report;
mod review;
mod sample;
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
        /// Proveedor del modelo.
        #[arg(long, value_enum, default_value_t = Provider::Anthropic)]
        provider: Provider,
        /// Modelo (por defecto claude-haiku-5-5 con Anthropic; obligatorio con openai-compatible).
        #[arg(long)]
        model: Option<String>,
        /// URL base de la API (por defecto la oficial, o Ollama local con openai-compatible).
        #[arg(long)]
        base_url: Option<String>,
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
        /// Incluye las reemplazadas, rechazadas y solo propuestas.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum ReviewCommand {
    /// Sesiones recientes con ediciones.
    List {
        #[arg(long, default_value_t = 15)]
        limit: usize,
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
        Command::Index { projects } => index(&mut open_store(cli.db)?, &projects_dir(projects)?),
        Command::Why { location, limit } => why(&open_store(cli.db)?, &location, limit),
        Command::Decisions {
            action: DecisionsCommand::Extract { provider, model, base_url, limit, chunk_chars, projects },
        } => {
            let extractor = build_extractor(provider, model, base_url)?;
            let mut store = open_store(cli.db)?;
            let projects = projects_dir(projects)?;
            // El proyecto de cada sesión sale de los ficheros que editó: hay que indexarlos antes.
            huella_engine::index(&mut store, &projects)?;
            extract(&mut store, &projects, extractor.as_ref(), &ExtractionOptions { limit, chunk_chars })
        }
        Command::Decisions { action: DecisionsCommand::List { entity, project, all } } => {
            let project = project
                .map(|path| fs::canonicalize(&path).unwrap_or(path))
                .map(|path| huella_engine::normalize_path(&path, None));
            let filter = DecisionFilter { entity, project, include_inactive: all };
            list_decisions(&open_store(cli.db)?, &filter)
        }
        Command::Review { action: ReviewCommand::List { limit } } => {
            let mut store = open_store(cli.db)?;
            huella_engine::index(&mut store, &projects_dir(None)?)?;
            emit(&review::render_sessions(&huella_engine::list_sessions(&store, limit)?));
            Ok(())
        }
        Command::Review { action: ReviewCommand::Open { session, editor, port, no_browser, projects } } => {
            let store = open_store(cli.db)?;
            let review = huella_engine::session_review(&projects_dir(projects)?, &session)?;
            let plan = huella_review::flow::plan(&review);
            web::serve(&store, &review, &plan, &web::Options { editor, port, open_browser: !no_browser })
        }
        Command::Review { action: ReviewCommand::Show { session, file, by_file, projects } } => {
            let review = huella_engine::session_review(&projects_dir(projects)?, &session)?;
            if by_file {
                emit(&review::render_review(&review, file.as_deref()));
            } else {
                let plan = huella_review::flow::plan(&review);
                let points = huella_review::attention::attention(&review, &plan);
                emit(&review::render_flow(&review, &plan, &points, file.as_deref()));
            }
            Ok(())
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

fn build_extractor(provider: Provider, model: Option<String>, base_url: Option<String>) -> Result<Box<dyn JsonModel>> {
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
            (other, _) => format!(" · {other}"),
        };
        println!("● {} — {date}{status}", decision.title);
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
