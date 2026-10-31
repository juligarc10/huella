//! `huella`: CLI de Huella, y lo que comparte con la aplicación de escritorio.
//!
//! El binario `huella` solo llama a [`run_cli`]. La aplicación de escritorio
//! (`huella-app`) usa [`desktop`] para servir la interfaz y su API sin abrir
//! ningún puerto.

mod config;
pub mod desktop;
mod mcp;
mod report;
mod review;
mod sample;
mod search;
mod setup;
mod ui;
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
use huella_decisions::openai::{OpenAiCompatible, OpenAiEmbeddings};
use huella_engine::ExtractionOptions;
use huella_store::{DecisionFilter, Store};
use serde::{Deserialize, Serialize};

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
    /// Pregunta sobre un trozo de código: `huella ask src/pago.ts:40-62 "¿por
    /// qué reintenta tres veces?"`. Responde con la historia de esas líneas (qué
    /// se pidió, qué explicó el agente, sus commits) y guarda la respuesta junto
    /// a ellas.
    Ask {
        /// Fichero y líneas: `FICHERO:INICIO-FIN` o `FICHERO:LÍNEA`.
        #[arg(value_name = "FICHERO:LÍNEAS")]
        location: String,
        /// La pregunta.
        question: String,
        #[command(flatten)]
        model: ModelArgs,
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
    /// Deja lista la instalación: base de datos (con copia antes de migrar),
    /// modelo, embeddings, Huella en los agentes (MCP) y en los editores, y el
    /// primer indexado. Se puede repetir: solo hace lo que falta.
    Setup {
        /// Sin preguntas: la opción por defecto en cada paso.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Extensión de VS Code local en vez de la de la última release.
        #[arg(long)]
        vsix: Option<PathBuf>,
        /// No indexa las sesiones al terminar.
        #[arg(long)]
        skip_index: bool,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
        /// Modelo a guardar en config.toml sin preguntar.
        #[command(flatten)]
        model: ModelArgs,
    },
    /// Comprueba la instalación: binario, configuración, modelo, base de datos,
    /// agentes y editores, y dice cómo arreglar lo que falte.
    Doctor {
        /// Además, llama al modelo y a los embeddings para ver que responden.
        #[arg(long)]
        online: bool,
        #[command(flatten)]
        model: ModelArgs,
    },
    /// Abre la aplicación local de Huella: buscar en la memoria, decisiones y
    /// ADRs, revisión de sesiones y estado, con las tareas largas en segundo plano.
    Ui {
        /// Puerto local. Uno fijo conserva entre ejecuciones lo que recuerda el
        /// navegador (tema, proyecto); si está ocupado se usa uno libre.
        #[arg(long, default_value_t = ui::DEFAULT_PORT)]
        port: u16,
        /// No abre el navegador; solo imprime la URL.
        #[arg(long)]
        no_browser: bool,
        /// Editor al que lleva «Abrir en…».
        #[arg(long, value_enum)]
        editor: Option<web::Editor>,
        /// Desarrollo de la interfaz: acepta el servidor de Vite (`pnpm --dir ui dev`).
        #[arg(long)]
        dev: bool,
        /// Modelo para extraer decisiones desde la aplicación.
        #[command(flatten)]
        model: ModelArgs,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Calcula los vectores para buscar por significado con un modelo de
    /// embeddings con la API de `OpenAI` (Ollama, llama.cpp, `OpenAI`…). Con
    /// `HUELLA_EMBED_MODEL` definida, `huella search` y el MCP los usan.
    Embed {
        /// Modelo (por defecto `$HUELLA_EMBED_MODEL`, p. ej. nomic-embed-text).
        #[arg(long)]
        model: Option<String>,
        /// URL base (por defecto `$HUELLA_EMBED_URL` u Ollama local).
        #[arg(long)]
        base_url: Option<String>,
        /// Textos a calcular como mucho.
        #[arg(long, default_value_t = 5000)]
        limit: usize,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Exporta las decisiones del proyecto como ADRs en markdown, para
    /// versionarlas con el código. Volver a exportar conserva los números y no
    /// pisa los ADRs editados a mano.
    Adr {
        /// Proyecto (por defecto, el repositorio de la carpeta actual).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Carpeta de salida (por defecto `docs/adr` dentro del proyecto).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Incluye también las decisiones descartadas y solo propuestas.
        #[arg(long)]
        all: bool,
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
        #[arg(long, value_enum)]
        editor: Option<web::Editor>,
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
    /// Comenta un paso de la revisión para que el agente lo corrija.
    Comment {
        /// Id de la sesión o su comienzo.
        session: String,
        /// Número del paso, como lo muestra `review show`.
        step: usize,
        /// Qué hay que cambiar.
        text: String,
        /// La línea del cambio a la que se refiere (su texto).
        #[arg(long)]
        line: Option<String>,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Instrucciones para el agente con lo que hay que corregir (comentarios
    /// abiertos y pasos marcados para cambiar o deshacer).
    Feedback {
        /// Id de la sesión o su comienzo.
        session: String,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Pregunta a la sesión por un paso o por todo lo que hizo.
    Ask {
        /// Id de la sesión o su comienzo.
        session: String,
        /// La pregunta.
        question: String,
        /// Número del paso, como lo muestra `review show`; sin él, la sesión entera.
        #[arg(long)]
        step: Option<usize>,
        #[command(flatten)]
        model: ModelArgs,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
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
    /// Revisa la rama actual contra su base: el diff que se va a entregar, con
    /// cada línea atribuida a la sesión de agente que la escribió o a quien la
    /// cambió a mano.
    Branch {
        /// Rama base (por defecto la principal del remoto, `main` o `master`).
        #[arg(long)]
        base: Option<String>,
        /// Repositorio (por defecto la carpeta actual).
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Comprueba con un modelo si cada turno hace lo que se pidió: requisitos
    /// cumplidos, a medias o sin hacer, y lo que hizo sin pedírselo.
    Coverage {
        /// Id de la sesión o su comienzo.
        session: String,
        #[command(flatten)]
        model: ModelArgs,
        /// Carpeta de transcripciones (por defecto ~/.claude/projects).
        #[arg(long)]
        projects: Option<PathBuf>,
    },
    /// Guía con un modelo los pasos que hay que revisar a fondo: qué hace el
    /// código, en qué orden leerlo y qué comprobar.
    Guide {
        /// Id de la sesión o su comienzo.
        session: String,
        /// Solo este paso (su número en la revisión).
        #[arg(long, conflicts_with = "all")]
        step: Option<usize>,
        /// También los pasos de vistazo, no solo los obligatorios.
        #[arg(long)]
        all: bool,
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
        #[arg(long, value_enum)]
        editor: Option<web::Editor>,
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
#[derive(clap::Args, Clone)]
struct ModelArgs {
    /// Proveedor del modelo.
    #[arg(long, value_enum, env = "HUELLA_LLM_PROVIDER")]
    provider: Option<Provider>,
    /// Modelo (por defecto claude-haiku-5-5 con Anthropic; obligatorio con openai-compatible).
    #[arg(long, env = "HUELLA_LLM_MODEL")]
    model: Option<String>,
    /// URL base de la API (por defecto la oficial, o Ollama local con openai-compatible).
    #[arg(long, env = "HUELLA_LLM_BASE_URL")]
    base_url: Option<String>,
}

/// Opción o variable de entorno; si no, `config.toml`; si no, el valor por defecto.
impl ModelArgs {
    fn provider(&self) -> Provider {
        self.provider.or(config::get().llm.provider).unwrap_or(Provider::Anthropic)
    }

    fn model(&self) -> Option<String> {
        self.model.clone().or_else(|| config::get().llm.model.clone())
    }

    fn base_url(&self) -> Option<String> {
        self.base_url.clone().or_else(|| config::get().llm.base_url.clone())
    }
}

#[derive(Clone, Copy, clap::ValueEnum, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Provider {
    /// API de Claude (`ANTHROPIC_API_KEY`).
    Anthropic,
    /// Cualquier servidor con la API de chat de `OpenAI`: Ollama, LM Studio, vLLM…
    /// (clave opcional en `HUELLA_LLM_API_KEY`).
    OpenaiCompatible,
}

/// La orden `huella` con los argumentos de la línea de órdenes.
#[must_use]
pub fn run_cli() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Lee `config.toml`. Un fichero roto detiene cualquier orden salvo doctor y
/// setup, que tienen que poder explicarlo; a ellas se les pasa el error.
fn load_config(command: &Command) -> Result<Option<anyhow::Error>> {
    match config::init() {
        Ok(()) => Ok(None),
        Err(error) if matches!(command, Command::Doctor { .. } | Command::Setup { .. }) => Ok(Some(error)),
        Err(error) => Err(error),
    }
}

fn run(cli: Cli) -> Result<()> {
    let config_error = load_config(&cli.command)?;
    match cli.command {
        Command::Scan { projects, json, sample, sample_out, sample_offset } => {
            let projects = claude_projects_dir(projects)?;
            let sample = sample.zip(sample_out).map(|(size, path)| SampleRequest { size, offset: sample_offset, path });
            scan(&projects, json.as_deref(), sample)
        }
        Command::Symbols { file } => print_symbols(&file),
        Command::Refactors { limit } => refactors(&open_store(cli.db)?, limit),
        Command::Index { projects } => index(&mut open_store(cli.db)?, &sources(projects)?),
        Command::Search { words, project, limit, all, projects } => {
            let mut store = open_store(cli.db)?;
            huella_engine::index(&mut store, &sources(projects)?)?;
            let project =
                project.map(|path| huella_engine::normalize_path(&fs::canonicalize(&path).unwrap_or(path), None));
            let query = huella_store::SearchQuery { text: words.join(" "), project, limit, include_inactive: all };
            let embedder = embedder(None, None);
            let (hits, warning) = huella_engine::embeddings::search(
                &store,
                &query,
                embedder.as_ref().map(|e| e as &dyn huella_decisions::Embedder),
            )?;
            if let Some(warning) = warning {
                eprintln!("aviso: {warning}");
            }
            emit(&search::render_hits(&hits));
            Ok(())
        }
        Command::Doctor { online, model } => setup::doctor_command(cli.db, &model, online, config_error.as_ref()),
        Command::Setup { yes, vsix, skip_index, projects, model } => {
            let options = setup::SetupOptions { yes, vsix, skip_index, db: cli.db, projects, model };
            setup::setup_command(options, config_error.as_ref())
        }
        Command::Ui { port, no_browser, editor, dev, model, projects } => {
            ui_command(cli.db, port, !no_browser, config::editor(editor), dev, model, projects)
        }
        Command::Embed { model, base_url, limit, projects } => {
            let embedder = embedder(model, base_url)
                .context("indica el modelo con --model o en HUELLA_EMBED_MODEL (p. ej. nomic-embed-text)")?;
            let mut store = open_store(cli.db)?;
            huella_engine::index(&mut store, &sources(projects)?)?;
            println!("Calculando vectores con {}…", huella_decisions::Embedder::model_id(&embedder));
            let report = huella_engine::embeddings::embed_pending(&mut store, &embedder, limit, |done, total| {
                println!("  {done} de {total}");
            })?;
            println!("Hecho: {} vectores nuevos", report.embedded);
            if report.skipped > 0 {
                println!(
                    "  {} textos que el modelo no aceptó ni acortados; se reintentan la próxima vez",
                    report.skipped
                );
            }
            if let Some(failure) = report.failure {
                bail!("el modelo falló: {failure}");
            }
            Ok(())
        }
        Command::Adr { project, out, all } => adr(&open_store(cli.db)?, project, out, all),
        Command::Why { location, limit } => why(&open_store(cli.db)?, &location, limit),
        Command::Ask { location, question, model } => ask_code(&open_store(cli.db)?, &location, &question, &model),
        Command::Decisions { action: DecisionsCommand::Extract { model, limit, chunk_chars, projects } } => {
            let extractor = build_model(&model)?;
            let mut store = open_store(cli.db)?;
            let projects = sources(projects)?;
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
            let mut context = mcp::Context { store: open_store(cli.db)?, sources: sources(projects)? };
            mcp::serve(&mut context)
        }
        Command::Lsp { projects, index_every } => lsp(cli.db, projects, index_every),
    }
}

/// Subcomandos de `huella review`.
fn review_command(db: Option<PathBuf>, action: ReviewCommand) -> Result<()> {
    match action {
        ReviewCommand::List { limit } => {
            let mut store = open_store(db)?;
            huella_engine::index(&mut store, &sources(None)?)?;
            emit(&review::render_sessions(&huella_engine::list_sessions(&store, limit)?));
            Ok(())
        }
        ReviewCommand::Start { session, open, editor, projects } => {
            let mut store = open_store(db)?;
            let projects = sources(projects)?;
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
                open_in_editor(config::editor(editor), project.as_deref(), first_pending(&store, &review, &plan)?);
            }
            Ok(())
        }
        ReviewCommand::Zed { session, done, no_open, projects } => review_zed(db, &session, done, !no_open, projects),
        ReviewCommand::Stop => {
            open_store(db)?.set_active_review(None)?;
            println!("Sin revisión activa.");
            Ok(())
        }
        action @ (ReviewCommand::Comment { .. } | ReviewCommand::Feedback { .. } | ReviewCommand::Ask { .. }) => {
            review_loop_command(db, action)
        }
        ReviewCommand::Narrate { session, model, projects } => {
            let model = build_model(&model)?;
            let store = open_store(db)?;
            let review = huella_engine::session_review(&sources(projects)?, &session)?;
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
        ReviewCommand::Coverage { session, model, projects } => coverage_command(db, &session, &model, projects),
        ReviewCommand::Guide { session, step, all, model, projects } => {
            guide_command(db, &session, step, all, &model, projects)
        }
        ReviewCommand::Branch { base, repo, projects } => branch_command(&repo, base.as_deref(), projects),
        ReviewCommand::Open { session, editor, port, no_browser, projects } => {
            let store = open_store(db)?;
            let review = huella_engine::session_review(&sources(projects)?, &session)?;
            let plan = huella_review::flow::plan(&review);
            // La misma revisión aparece también en el editor mientras se hace en la web.
            store.set_active_review(Some(&review.session_id))?;
            web::serve(
                &store,
                &review,
                &plan,
                &web::Options { editor: config::editor(editor), port, open_browser: !no_browser },
            )
        }
        ReviewCommand::Show { session, file, by_file, projects } => {
            let review = huella_engine::session_review(&sources(projects)?, &session)?;
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
fn branch_command(repo: &Path, base: Option<&str>, projects: Option<PathBuf>) -> Result<()> {
    use huella_engine::branch::{BranchLine, LineOrigin};
    let review = huella_engine::branch::branch_review(&sources(projects)?, repo, base)?;
    let total: usize = review.files.iter().map(|file| file.added).sum();
    let by_sessions: usize = review.sessions.iter().map(|session| session.lines + session.terminal_lines).sum();
    println!(
        "{} contra {} (desde {}): {} ficheros, +{total} líneas",
        review.branch.as_deref().unwrap_or("HEAD"),
        review.base,
        review.merge_base,
        review.files.len()
    );
    println!("\nEscritas por agentes: {by_sessions} líneas en {} sesiones", review.sessions.len());
    for session in &review.sessions {
        let prompt = session.prompt.as_deref().and_then(|p| p.lines().next()).unwrap_or("");
        println!(
            "  {} · {} · {} líneas en {} ficheros, {} por terminal · «{}»",
            &session.session_id[..8.min(session.session_id.len())],
            session.agent,
            session.lines,
            session.files,
            session.terminal_lines,
            prompt.chars().take(70).collect::<String>()
        );
    }
    println!("\nA mano: {} líneas", total - by_sessions);
    for (person, lines) in &review.people {
        println!("  {person}: {lines}");
    }
    println!("\nFicheros:");
    for file in &review.files {
        let agent = file
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| {
                matches!(
                    line,
                    BranchLine::Added { origin: LineOrigin::Session { .. } | LineOrigin::Terminal { .. }, .. }
                )
            })
            .count();
        println!(
            "  {:>5} +{:<4} −{:<4} {:>3}% agente  {}",
            file.status,
            file.added,
            file.removed,
            agent * 100 / file.added.max(1),
            file.path
        );
    }
    for path in &review.skipped {
        println!("  (sin atribuir: {path})");
    }
    Ok(())
}

fn coverage_command(db: Option<PathBuf>, session: &str, model: &ModelArgs, projects: Option<PathBuf>) -> Result<()> {
    use huella_engine::coverage::RequirementStatus;
    let model = build_model(model)?;
    let store = open_store(db)?;
    let review = huella_engine::session_review(&sources(projects)?, session)?;
    let plan = huella_review::flow::plan(&review);
    let points = huella_review::attention::attention(&review, &plan);
    println!("Comprobando la cobertura con {}…", model.model_id());
    let report = huella_engine::coverage::cover(&store, &review, &plan, &points, model.as_ref(), |turn| {
        println!("  · turno {}", turn + 1);
    })?;
    for failure in &report.failures {
        eprintln!("aviso: {failure}");
    }
    let step_name = |index: usize| {
        plan[index]
            .symbol
            .as_ref()
            .map_or_else(|| review.files[plan[index].file].path.clone(), |s| s.qualified_name.clone())
    };
    for (turn, coverage) in huella_engine::coverage::coverages(&store, &review, &plan, &points)? {
        let prompt = review.turns.get(turn).and_then(|t| t.prompt.as_deref()).unwrap_or("(sin petición)");
        println!(
            "\nTurno {} · «{}»{}",
            turn + 1,
            prompt.lines().next().unwrap_or_default(),
            if coverage.stale { " (desfasada)" } else { "" }
        );
        for requirement in &coverage.requirements {
            let mark = match requirement.status {
                RequirementStatus::Done => "✓",
                RequirementStatus::Partial => "~",
                RequirementStatus::Missing => "✗",
            };
            let steps: Vec<String> = requirement.steps.iter().map(|&s| step_name(s)).collect();
            println!(
                "  {mark} {}{}",
                requirement.text,
                if steps.is_empty() { String::new() } else { format!(" → {}", steps.join(", ")) }
            );
            if let Some(note) = &requirement.note {
                println!("      {note}");
            }
        }
        for (step, note) in &coverage.unrequested {
            println!("  + sin pedir: {} — {note}", step_name(*step));
        }
    }
    Ok(())
}

fn guide_command(
    db: Option<PathBuf>,
    session: &str,
    step: Option<usize>,
    all: bool,
    model: &ModelArgs,
    projects: Option<PathBuf>,
) -> Result<()> {
    use huella_engine::triage::Level;
    let model = build_model(model)?;
    let store = open_store(db)?;
    let sources = sources(projects)?;
    let review = huella_engine::session_review(&sources, session)?;
    let plan = huella_review::flow::plan(&review);
    let points = huella_review::attention::attention(&review, &plan);
    let activity = huella_engine::evidence::session_activity(&sources, &review.session_id)?;
    let project = huella_engine::session_project(&store, &review.session_id)?;
    let assessment = huella_engine::triage::assess(&store, &review, &plan, &points, &activity, project.as_deref())?;
    let chosen: Vec<usize> = match step {
        Some(number) => {
            let index = number.checked_sub(1).filter(|&i| i < plan.len()).context("no hay ese paso")?;
            vec![index]
        }
        None => (0..plan.len())
            .filter(|&i| {
                let level = assessment.triage[i].level;
                level == Level::Must || (all && level == Level::Acknowledge)
            })
            .collect(),
    };
    let name = |index: usize| {
        plan[index]
            .symbol
            .as_ref()
            .map_or_else(|| review.files[plan[index].file].path.clone(), |s| s.qualified_name.clone())
    };
    println!("Guiando {} pasos con {}…", chosen.len(), model.model_id());
    for &index in &chosen {
        println!("  · paso {} {}", index + 1, name(index));
        if let Err(error) = huella_engine::guide::guide_step(
            &store,
            &review,
            &plan,
            index,
            &assessment.evidence[index],
            &assessment.triage[index],
            model.as_ref(),
        ) {
            eprintln!("aviso: paso {}: {error}", index + 1);
        }
    }
    let context: Vec<_> = assessment.evidence.iter().zip(&assessment.triage).collect();
    let guides = huella_engine::guide::guides(&store, &review, &plan, &context)?;
    for index in chosen {
        let Some(guide) = guides.get(&index) else { continue };
        let lines = |(from, to): (usize, usize)| {
            if from == to { format!("L{}", from + 1) } else { format!("L{}-{}", from + 1, to + 1) }
        };
        println!("\nPaso {} · {}{}", index + 1, name(index), if guide.stale { " (desfasada)" } else { "" });
        println!("  {}", guide.explanation);
        for (number, stop) in guide.walkthrough.iter().enumerate() {
            println!("  {}. {} {}", number + 1, lines(stop.lines), stop.note);
        }
        for check in &guide.checks {
            let at = check.lines.map(|range| format!(" ({})", lines(range))).unwrap_or_default();
            println!("  [{}] {}{at}", if check.done { "x" } else { " " }, check.check);
        }
    }
    Ok(())
}

fn review_zed(db: Option<PathBuf>, session: &str, done: bool, open: bool, projects: Option<PathBuf>) -> Result<()> {
    let mut store = open_store(db)?;
    let projects = sources(projects)?;
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

/// `huella review comment|feedback|ask`: el bucle de corrección.
fn review_loop_command(db: Option<PathBuf>, action: ReviewCommand) -> Result<()> {
    let store = open_store(db)?;
    let pick = |plan: &[huella_review::flow::FlowStep], step: usize| -> Result<usize> {
        let index = step.checked_sub(1).filter(|&i| i < plan.len());
        index.with_context(|| format!("la revisión tiene {} pasos; elige uno de 1 a {}", plan.len(), plan.len()))
    };
    match action {
        ReviewCommand::Comment { session, step, text, line, projects } => {
            let review = huella_engine::session_review(&sources(projects)?, &session)?;
            let plan = huella_review::flow::plan(&review);
            let chosen = &plan[pick(&plan, step)?];
            let key = chosen.key(&review);
            let id = store.add_comment(&huella_store::NewComment {
                session_id: &review.session_id,
                step_key: &key,
                path: &review.files[chosen.file].path,
                symbol: chosen.symbol.as_ref().map(|s| s.qualified_name.as_str()),
                quote: line.as_deref(),
                body: &text,
            })?;
            store.set_review_status(&review.session_id, Some("changes_requested"), Some(plan.len()))?;
            println!(
                "Comentario {id} en el paso {step}. `huella review feedback {session}` da las instrucciones para el agente."
            );
            Ok(())
        }
        ReviewCommand::Feedback { session, projects } => {
            let review = huella_engine::session_review(&sources(projects)?, &session)?;
            let plan = huella_review::flow::plan(&review);
            emit(&huella_engine::review_loop::feedback(&store, &review, &plan)?.markdown);
            Ok(())
        }
        ReviewCommand::Ask { session, question, step, model, projects } => {
            let model = build_model(&model)?;
            let review = huella_engine::session_review(&sources(projects)?, &session)?;
            let plan = huella_review::flow::plan(&review);
            let step = step.map(|step| pick(&plan, step)).transpose()?;
            let answer = huella_engine::review_loop::ask(&store, model.as_ref(), &review, &plan, step, &question)?;
            emit(&format!("{}\n\n({})\n", answer.answer, answer.model));
            Ok(())
        }
        _ => unreachable!("solo comandos del bucle de corrección"),
    }
}

/// `huella lsp`: el servidor del editor; `index_every` 0 desactiva el indexado.
fn lsp(db: Option<PathBuf>, projects: Option<PathBuf>, index_every: u64) -> Result<()> {
    let db = db.or_else(huella_engine::default_db_path).context("no se encontró dónde guardar datos; usa --db")?;
    let sources = if index_every == 0 { None } else { Some(sources(projects)?) };
    let config = huella_lsp::Config { db, sources, index_every: Duration::from_secs(index_every.max(1)) };
    huella_lsp::serve_stdio(&config)?;
    Ok(())
}

/// `huella ui`: la aplicación local.
fn ui_command(
    db: Option<PathBuf>,
    port: u16,
    open_browser: bool,
    editor: web::Editor,
    dev: bool,
    model: ModelArgs,
    projects: Option<PathBuf>,
) -> Result<()> {
    let db = db.or_else(huella_engine::default_db_path).context("no se encontró dónde guardar datos; usa --db")?;
    if let Some(dir) = db.parent() {
        fs::create_dir_all(dir)?;
    }
    ui::serve(ui::Options { port, open_browser, editor, dev, db, sources: sources(projects)?, model })
}

/// `huella adr`: exporta los ADRs del proyecto.
fn adr(store: &huella_store::Store, project: Option<PathBuf>, out: Option<PathBuf>, all: bool) -> Result<()> {
    let project = if let Some(path) = project {
        fs::canonicalize(&path).unwrap_or(path)
    } else {
        let here = std::env::current_dir()?;
        here.ancestors().find(|dir| dir.join(".git").exists()).map_or(here.clone(), Path::to_path_buf)
    };
    let key = huella_engine::normalize_path(&project, None);
    let out = out.unwrap_or_else(|| Path::new(&key).join("docs").join("adr"));
    let report = huella_engine::adr::export_adrs(store, &key, &out, all)?;
    let total = report.written.len() + report.unchanged + report.edited_by_hand.len();
    if total == 0 {
        println!("{key} no tiene decisiones que exportar. Extráelas con `huella decisions extract`.");
        return Ok(());
    }
    println!("ADRs de {key} en {}: {} escritos, {} al día", out.display(), report.written.len(), report.unchanged);
    for path in &report.written {
        println!("  escrito: {}", path.display());
    }
    for path in &report.edited_by_hand {
        println!("  editado a mano, no se toca: {}", path.display());
    }
    for path in &report.orphaned {
        println!("  su decisión ya no existe (no se borra): {}", path.display());
    }
    Ok(())
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

/// Los agentes cuyas sesiones se leen; `explicit` es la carpeta de
/// transcripciones de Claude Code (`--projects`).
fn sources(explicit: Option<PathBuf>) -> Result<huella_engine::Sources> {
    Ok(huella_engine::Sources::detect(claude_projects_dir(explicit)?))
}

fn claude_projects_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    explicit.or_else(huella_engine::default_projects_dir).context("no se encontró la carpeta personal; usa --projects")
}

fn open_store(explicit: Option<PathBuf>) -> Result<Store> {
    let path =
        explicit.or_else(huella_engine::default_db_path).context("no se encontró dónde guardar datos; usa --db")?;
    Store::open(&path).with_context(|| format!("abriendo {}", path.display()))
}

fn index(store: &mut Store, projects: &huella_engine::Sources) -> Result<()> {
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

fn ask_code(store: &Store, location: &str, question: &str, model: &ModelArgs) -> Result<()> {
    let (file, range) =
        location.rsplit_once(':').context("usa el formato FICHERO:INICIO-FIN, por ejemplo src/app.ts:40-62")?;
    let (start, end) = range.split_once('-').unwrap_or((range, range));
    let number = |text: &str| text.trim().parse::<usize>().context("las líneas tienen que ser números");
    let (start, end) = (number(start)?, number(end)?);
    let path = fs::canonicalize(file).with_context(|| format!("no se encontró {file}"))?;
    let llm = build_model(model)?;
    eprintln!("Preguntando a {} sobre {file}:{start}-{end}…", llm.model_id());
    let sources = sources(None)?;
    let answer = huella_engine::code_ask::ask_code(
        store,
        Some(&sources),
        llm.as_ref(),
        &path,
        "",
        Some((start, end)),
        question,
    )?;
    println!("{}", answer.answer);
    Ok(())
}

/// Modelo de embeddings de los argumentos, de `HUELLA_EMBED_MODEL` y
/// `HUELLA_EMBED_URL` o de `[embed]` en `config.toml`; `None` si no hay ninguno.
pub(crate) fn embedder(model: Option<String>, base_url: Option<String>) -> Option<OpenAiEmbeddings> {
    let config = config::get();
    let embed = &config.embed;
    let model = model
        .or_else(|| std::env::var("HUELLA_EMBED_MODEL").ok())
        .or_else(|| embed.model.clone())
        .filter(|m| !m.trim().is_empty())?;
    let base_url = base_url.or_else(|| std::env::var("HUELLA_EMBED_URL").ok()).or_else(|| embed.base_url.clone());
    // Un fichero de clave mal configurado ya lo avisa el modelo de lenguaje.
    let key = config::api_key("HUELLA_LLM_API_KEY").ok().flatten();
    Some(OpenAiEmbeddings::new(base_url, model, key))
}

fn config_hint() -> String {
    config::path().map_or_else(|| "config.toml".to_owned(), |path| path.display().to_string())
}

/// Qué le falta a la configuración del modelo para poder llamarlo, si algo.
/// Con openai-compatible la clave es opcional: un servidor local no la pide.
fn model_problem(args: &ModelArgs) -> Option<String> {
    match args.provider() {
        Provider::Anthropic => match config::api_key("ANTHROPIC_API_KEY") {
            Ok(Some(_)) => None,
            Ok(None) => Some(format!(
                "No hay modelo configurado: define ANTHROPIC_API_KEY, o un servidor compatible con OpenAI en [llm] \
                 de {} (provider = \"openai-compatible\", base_url, model y, si la pide, api_key_file). \
                 Después vuelve a abrir huella ui.",
                config_hint()
            )),
            Err(error) => Some(format!("{error:#}")),
        },
        Provider::OpenaiCompatible if args.model().is_none() => Some(format!(
            "Falta el modelo: añade model en [llm] de {} (o HUELLA_LLM_MODEL) y vuelve a abrir huella ui.",
            config_hint()
        )),
        Provider::OpenaiCompatible => config::api_key("HUELLA_LLM_API_KEY").err().map(|error| format!("{error:#}")),
    }
}

fn build_model(args: &ModelArgs) -> Result<Box<dyn JsonModel>> {
    let (model, base_url) = (args.model(), args.base_url());
    Ok(match args.provider() {
        Provider::Anthropic => {
            let key = config::api_key("ANTHROPIC_API_KEY")?
                .context("falta la credencial: define ANTHROPIC_API_KEY o api_key_file en config.toml")?;
            Box::new(Anthropic::new(key, model, base_url))
        }
        Provider::OpenaiCompatible => {
            let model = model.context("con openai-compatible indica el modelo con --model o en config.toml")?;
            let thinking = config::get().llm.thinking;
            Box::new(
                OpenAiCompatible::new(base_url, model, config::api_key("HUELLA_LLM_API_KEY")?).with_thinking(thinking),
            )
        }
    })
}

fn extract(
    store: &mut Store,
    projects: &huella_engine::Sources,
    extractor: &dyn JsonModel,
    options: &ExtractionOptions,
) -> Result<()> {
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
