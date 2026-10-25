//! `huella`: CLI del prototipo F0.

mod report;
mod sample;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use huella_anchoring::{AnchorReport, Language, anchor_edit, extract_symbols};
use huella_claude_code::{discover_transcripts, read_transcript};
use huella_core::FileEdit;
use serde::Serialize;

use crate::report::Summary;

#[derive(Parser)]
#[command(name = "huella", version, about = "Ancla las sesiones de agentes de IA a los símbolos del código")]
struct Cli {
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
            let projects = match projects {
                Some(path) => path,
                None => default_projects_dir()?,
            };
            let sample = sample.zip(sample_out).map(|(size, path)| SampleRequest { size, offset: sample_offset, path });
            scan(&projects, json.as_deref(), sample)
        }
        Command::Symbols { file } => print_symbols(&file),
    }
}

fn default_projects_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("no se encontró HOME; usa --projects")?;
    Ok(Path::new(&home).join(".claude/projects"))
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
