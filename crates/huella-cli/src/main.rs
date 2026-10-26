//! `huella`: CLI de Huella.

mod report;
mod sample;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use huella_anchoring::{AnchorReport, Language, anchor_edit, extract_symbols};
use huella_claude_code::{discover_transcripts, read_transcript};
use huella_core::FileEdit;
use huella_store::Store;
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
