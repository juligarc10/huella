//! `huella doctor` y `huella setup`: comprobar y dejar lista la instalación.
//!
//! Las dos órdenes comparten las comprobaciones: `doctor` solo las enseña,
//! `setup` arregla lo que puede (preguntando, o sin preguntar con `--yes`) y
//! termina enseñándolas también.

mod agents;
mod editors;
mod model;
mod wizard;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use huella_store::Store;

pub use self::agents::McpAgent;
pub use self::wizard::{SetupOptions, setup};
use crate::ModelArgs;

/// Carpetas del usuario; en los tests apuntan a una carpeta temporal.
pub struct Dirs {
    pub home: PathBuf,
    /// `~/Library/Application Support` en macOS, `~/.config` en Linux.
    pub config: PathBuf,
    /// `~/Library/Application Support` en macOS, `~/.local/share` en Linux.
    pub data: PathBuf,
    /// Dónde buscar órdenes (`claude`, `code`, `huella`).
    pub path: OsString,
    /// Aplicaciones instaladas fuera del `PATH` (`/Applications` en macOS).
    pub apps: PathBuf,
    /// Raíz de la API de Ollama, para proponer sus modelos.
    pub ollama: String,
}

impl Dirs {
    pub fn detect() -> Option<Self> {
        Some(Self {
            home: dirs::home_dir()?,
            config: dirs::config_dir()?,
            data: dirs::data_dir()?,
            path: std::env::var_os("PATH").unwrap_or_default(),
            apps: PathBuf::from("/Applications"),
            ollama: "http://localhost:11434".to_owned(),
        })
    }

    /// Primer `name` ejecutable del `PATH`.
    pub fn on_path(&self, name: &str) -> Option<PathBuf> {
        let names: Vec<String> =
            if cfg!(windows) { vec![format!("{name}.exe"), format!("{name}.cmd")] } else { vec![name.to_owned()] };
        std::env::split_paths(&self.path)
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|path| path.is_file())
    }

    /// URL compatible con `OpenAI` del Ollama local.
    pub fn ollama_v1(&self) -> String {
        format!("{}/v1", self.ollama)
    }

    /// Carpetas vacías bajo `root`, para los tests.
    #[cfg(test)]
    pub fn sandbox(root: &Path) -> Self {
        Self {
            home: root.join("home"),
            config: root.join("config"),
            data: root.join("data"),
            path: std::ffi::OsString::new(),
            apps: root.join("apps"),
            // Puerto cerrado: los tests no dependen del Ollama de la máquina.
            ollama: "http://127.0.0.1:9".to_owned(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    /// Opcional o mejorable; Huella funciona igual.
    Warn,
    /// Algo no va a funcionar.
    Fail,
}

pub struct Check {
    pub area: &'static str,
    pub status: Status,
    pub detail: String,
    /// Qué hacer para arreglarlo.
    pub hint: Option<String>,
}

impl Check {
    fn new(area: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self { area, status, detail: detail.into(), hint: None }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Lo que hay que saber para comprobar una instalación.
pub struct Context<'a> {
    pub dirs: &'a Dirs,
    pub db: Option<PathBuf>,
    pub model: &'a ModelArgs,
    pub config_path: Option<PathBuf>,
    /// Error al leer `config.toml`, si lo hubo: no impide comprobar el resto.
    pub config_error: Option<&'a anyhow::Error>,
}

/// El binario que se registra en los agentes: el que se está ejecutando.
pub fn current_exe() -> PathBuf {
    std::env::current_exe().and_then(std::fs::canonicalize).unwrap_or_else(|_| PathBuf::from("huella"))
}

pub fn checks(context: &Context<'_>, online: bool) -> Vec<Check> {
    let mut checks = vec![binary_check(context.dirs), config_check(context)];
    checks.push(model::llm_check(context.model, online));
    checks.push(model::embed_check(online));
    checks.push(db_check(context.db.as_deref()));
    let exe = current_exe();
    for agent in McpAgent::ALL {
        if let Some(check) = agent_check(agent, context.dirs, &exe) {
            checks.push(check);
        }
    }
    checks.extend(editor_checks(context.dirs));
    checks
}

fn binary_check(dirs: &Dirs) -> Check {
    let exe = current_exe();
    match dirs.on_path("huella").map(|path| std::fs::canonicalize(&path).unwrap_or(path)) {
        Some(found) if found == exe => Check::new("Binario", Status::Ok, exe.display().to_string()),
        Some(found) => {
            Check::new("Binario", Status::Warn, format!("{} (en el PATH hay otro: {})", exe.display(), found.display()))
                .hint("los agentes y editores usan el del PATH; reinstala o quita el antiguo")
        }
        None => Check::new("Binario", Status::Warn, format!("{} no está en el PATH", exe.display()))
            .hint(format!("añade {} al PATH", exe.parent().map_or_else(String::new, |dir| dir.display().to_string()))),
    }
}

fn config_check(context: &Context<'_>) -> Check {
    let path = context.config_path.clone();
    let shown = path.as_ref().map_or_else(|| "config.toml".to_owned(), |path| path.display().to_string());
    match (context.config_error, path) {
        (Some(error), _) => {
            Check::new("Configuración", Status::Fail, format!("{error:#}")).hint(format!("corrige {shown}"))
        }
        (None, Some(path)) if path.exists() => Check::new("Configuración", Status::Ok, shown),
        _ => Check::new("Configuración", Status::Warn, format!("no hay {shown}; se usan los valores por defecto"))
            .hint("huella setup la crea"),
    }
}

fn db_check(db: Option<&Path>) -> Check {
    let Some(path) = db.map(Path::to_owned).or_else(huella_engine::default_db_path) else {
        return Check::new("Base de datos", Status::Fail, "no se encontró la carpeta de datos").hint("usa --db");
    };
    match Store::schema_status(&path) {
        Ok(None) => Check::new("Base de datos", Status::Warn, format!("{} aún no existe", path.display()))
            .hint("huella setup la crea e indexa las sesiones"),
        Ok(Some(status)) if status.is_newer() => Check::new(
            "Base de datos",
            Status::Fail,
            format!(
                "{} es de una versión más nueva de Huella (esquema {} > {})",
                path.display(),
                status.found,
                status.supported
            ),
        )
        .hint("actualiza huella"),
        Ok(Some(status)) if status.needs_migration() => Check::new(
            "Base de datos",
            Status::Warn,
            format!("{}: esquema {} → {} pendiente", path.display(), status.found, status.supported),
        )
        .hint("huella setup guarda una copia antes de migrarla"),
        Ok(Some(_)) => Check::new("Base de datos", Status::Ok, path.display().to_string()),
        Err(error) => Check::new("Base de datos", Status::Fail, format!("{}: {error}", path.display())),
    }
}

fn agent_check(agent: McpAgent, dirs: &Dirs, exe: &Path) -> Option<Check> {
    if !agent.detected(dirs) {
        return None;
    }
    let area = match agent {
        McpAgent::ClaudeCode => "MCP · Claude Code",
        McpAgent::Codex => "MCP · Codex",
        McpAgent::CopilotVscode => "MCP · Copilot VS Code",
        McpAgent::CopilotCli => "MCP · Copilot CLI",
    };
    Some(match agent.registered(dirs) {
        Ok(true) => Check::new(area, Status::Ok, "registrado"),
        Ok(false) => {
            Check::new(area, Status::Warn, "sin registrar: el agente no ve la memoria").hint("huella setup lo registra")
        }
        Err(error) => Check::new(area, Status::Warn, format!("{error:#}")).hint(agent.manual(dirs, exe)),
    })
}

fn editor_checks(dirs: &Dirs) -> Vec<Check> {
    let mut checks = Vec::new();
    if let Some(code) = editors::vscode_cli(dirs) {
        checks.push(match editors::vscode_extension_installed(&code) {
            Ok(true) => Check::new("VS Code", Status::Ok, "extensión instalada"),
            Ok(false) => {
                Check::new("VS Code", Status::Warn, "sin la extensión de Huella").hint("huella setup la instala")
            }
            Err(error) => Check::new("VS Code", Status::Warn, format!("{error:#}")),
        });
    }
    if let Some(zed) = editors::zed_dir(dirs) {
        checks.push(if editors::zed_extension_installed(&zed) {
            Check::new("Zed", Status::Ok, "extensión instalada")
        } else {
            Check::new("Zed", Status::Warn, "sin la extensión de Huella").hint(editors::ZED_MANUAL)
        });
    }
    checks
}

pub fn print_checks(checks: &[Check]) {
    let width = checks.iter().map(|check| check.area.chars().count()).max().unwrap_or(0);
    for check in checks {
        let mark = match check.status {
            Status::Ok => "✓",
            Status::Warn => "!",
            Status::Fail => "✗",
        };
        let pad = width - check.area.chars().count();
        println!("{mark} {}{}  {}", check.area, " ".repeat(pad), check.detail);
        if let Some(hint) = check.hint.as_ref().filter(|_| check.status != Status::Ok) {
            for line in hint.lines() {
                println!("  {}  → {line}", " ".repeat(width));
            }
        }
    }
}

pub fn doctor_command(
    db: Option<PathBuf>,
    model: &ModelArgs,
    online: bool,
    config_error: Option<&anyhow::Error>,
) -> Result<()> {
    let dirs = Dirs::detect().context("no se encontró la carpeta personal")?;
    doctor(&Context { dirs: &dirs, db, model, config_path: crate::config::path(), config_error }, online)
}

pub fn setup_command(options: SetupOptions, config_error: Option<&anyhow::Error>) -> Result<()> {
    let dirs = Dirs::detect().context("no se encontró la carpeta personal")?;
    let config_path = crate::config::path().context("no se encontró la carpeta de datos; usa HUELLA_CONFIG")?;
    setup(options, &dirs, &config_path, config_error)
}

/// `huella doctor`: falla (código de salida 1) si alguna comprobación falla.
pub fn doctor(context: &Context<'_>, online: bool) -> Result<()> {
    let checks = checks(context, online);
    print_checks(&checks);
    let failed = checks.iter().filter(|check| check.status == Status::Fail).count();
    let warned = checks.iter().filter(|check| check.status == Status::Warn).count();
    println!();
    if failed > 0 {
        anyhow::bail!("{failed} comprobaciones fallan");
    }
    if warned > 0 {
        println!("Huella funciona; {warned} cosas se pueden mejorar (huella setup).");
    } else {
        println!("Todo en orden.");
    }
    Ok(())
}
