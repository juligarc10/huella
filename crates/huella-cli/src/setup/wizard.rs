//! `huella setup`: deja la instalación lista, paso a paso.
//!
//! Cada paso mira primero si ya está hecho, así que repetir la orden es
//! seguro: sirve tanto para instalar como para actualizar.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, Password, Select};
use huella_store::Store;

use super::model::{self, LlmChoice};
use super::{Context, Dirs, McpAgent, Status, current_exe, editors};
use crate::{ModelArgs, Provider, config};

pub struct SetupOptions {
    /// Sin preguntas: la respuesta por defecto en cada paso.
    pub yes: bool,
    /// `.vsix` local en vez del de la última release.
    pub vsix: Option<PathBuf>,
    pub skip_index: bool,
    pub db: Option<PathBuf>,
    pub projects: Option<PathBuf>,
    /// Modelo indicado por opciones o variables de entorno: se guarda tal cual.
    pub model: ModelArgs,
}

/// Preguntas, o sus respuestas por defecto con `--yes`.
struct Prompt {
    yes: bool,
    theme: ColorfulTheme,
}

impl Prompt {
    fn confirm(&self, question: &str, default: bool) -> Result<bool> {
        if self.yes {
            return Ok(default);
        }
        Ok(Confirm::with_theme(&self.theme).with_prompt(question).default(default).interact()?)
    }

    fn select(&self, question: &str, items: &[&str], default: usize) -> Result<usize> {
        if self.yes {
            return Ok(default);
        }
        Ok(Select::with_theme(&self.theme).with_prompt(question).items(items).default(default).interact()?)
    }

    fn input(&self, question: &str, default: Option<&str>) -> Result<String> {
        let mut input = Input::<String>::with_theme(&self.theme).with_prompt(question);
        if let Some(default) = default {
            input = input.default(default.to_owned());
        }
        Ok(input.interact_text()?.trim().to_owned())
    }

    /// Clave sin eco; vacía si no se escribe nada.
    fn secret(&self, question: &str) -> Result<String> {
        Ok(Password::with_theme(&self.theme)
            .with_prompt(question)
            .allow_empty_password(true)
            .interact()?
            .trim()
            .to_owned())
    }
}

fn step(title: &str) {
    println!("\n── {title}");
}

pub fn setup(
    options: SetupOptions,
    dirs: &Dirs,
    config_path: &Path,
    config_error: Option<&anyhow::Error>,
) -> Result<()> {
    if let Some(error) = config_error {
        bail!("corrige {} antes de seguir: {error:#}", config_path.display());
    }
    if !options.yes && !std::io::stdin().is_terminal() {
        bail!("huella setup pregunta en la terminal; sin terminal, usa --yes");
    }
    let prompt = Prompt { yes: options.yes, theme: ColorfulTheme::default() };
    println!(
        "Huella · instalación en {}",
        config_path.parent().map_or_else(String::new, |dir| dir.display().to_string())
    );

    step("Base de datos");
    let db = database(options.db.clone())?;

    step("Modelo de lenguaje (decisiones y preguntas a la sesión)");
    language_model(&prompt, dirs, config_path, &options.model)?;

    step("Búsqueda por significado (opcional)");
    embeddings(&prompt, dirs, config_path)?;

    step("Agentes (MCP)");
    let exe = current_exe();
    let mut any_agent = false;
    for agent in McpAgent::ALL.into_iter().filter(|agent| agent.detected(dirs)) {
        any_agent = true;
        register_agent(&prompt, agent, dirs, &exe)?;
    }
    if !any_agent {
        println!("No se encontró ningún agente compatible (Claude Code, Codex, Copilot).");
    }

    step("Editores");
    editors_step(&prompt, dirs, config_path, options.vsix.as_deref())?;

    if !options.skip_index {
        step("Primer indexado");
        let mut store = Store::open(&db).with_context(|| format!("abriendo {}", db.display()))?;
        crate::index(&mut store, &crate::sources(options.projects)?)?;
    }

    step("Resultado");
    let checks = super::checks(
        &Context {
            dirs,
            db: Some(db),
            model: &options.model,
            config_path: Some(config_path.to_owned()),
            config_error: None,
        },
        false,
    );
    super::print_checks(&checks);
    if checks.iter().any(|check| check.status == Status::Fail) {
        bail!("queda algo por arreglar (arriba); huella doctor lo vuelve a comprobar");
    }
    println!("\nListo. Abre la aplicación con: huella ui");
    Ok(())
}

/// Crea la base de datos o la migra, guardando antes una copia.
fn database(explicit: Option<PathBuf>) -> Result<PathBuf> {
    let path =
        explicit.or_else(huella_engine::default_db_path).context("no se encontró dónde guardar datos; usa --db")?;
    match Store::schema_status(&path)? {
        None => {
            Store::open(&path).with_context(|| format!("creando {}", path.display()))?;
            println!("Creada en {}", path.display());
        }
        Some(status) if status.is_newer() => bail!(
            "{} es de una versión más nueva de Huella (esquema {} > {}): actualiza el binario",
            path.display(),
            status.found,
            status.supported
        ),
        Some(status) if status.needs_migration() => {
            let copy = backup_path(&path, status.found);
            Store::backup(&path, &copy).with_context(|| format!("no se pudo copiar {}", path.display()))?;
            println!("Copia antes de migrar: {}", copy.display());
            Store::open(&path).with_context(|| format!("migrando {}", path.display()))?;
            println!("Migrada del esquema {} al {}.", status.found, status.supported);
        }
        Some(_) => println!("✓ {}", path.display()),
    }
    Ok(path)
}

/// `huella.pre-v11.db`, o con la hora si ya hay una copia con ese nombre.
fn backup_path(db: &Path, version: i64) -> PathBuf {
    let stem = db.file_stem().map_or_else(|| "huella".into(), |stem| stem.to_string_lossy().into_owned());
    let candidate = db.with_file_name(format!("{stem}.pre-v{version}.db"));
    if !candidate.exists() {
        return candidate;
    }
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    db.with_file_name(format!("{stem}.pre-v{version}-{seconds}.db"))
}

fn language_model(prompt: &Prompt, dirs: &Dirs, config_path: &Path, given: &ModelArgs) -> Result<()> {
    let explicit = given.provider.is_some() || given.model.is_some() || given.base_url.is_some();
    let choice = if explicit {
        // Opciones o variables de entorno: se guardan para no tener que repetirlas.
        Some(LlmChoice { provider: given.provider(), base_url: given.base_url(), model: given.model(), key_file: None })
    } else if crate::model_problem(given).is_none() {
        println!("✓ {}", crate::ui::extractor_label(given));
        if prompt.confirm("¿Cambiarlo?", false)? { choose_llm(prompt, dirs)? } else { None }
    } else if prompt.yes {
        println!("! Sin modelo: huella setup --provider … --model … --base-url … lo guarda sin preguntar.");
        None
    } else {
        choose_llm(prompt, dirs)?
    };
    let Some(choice) = choice else {
        return Ok(());
    };
    model::save_llm(config_path, &choice)?;
    config::init_from(Some(config_path))?;
    println!("Guardado en {}", config_path.display());
    if let Some(problem) = crate::model_problem(given) {
        println!("! {problem}");
        return Ok(());
    }
    print!("Probando {}… ", crate::ui::extractor_label(given));
    match model::test_llm(given) {
        Ok(()) => println!("responde."),
        Err(error) => {
            println!("no responde: {error:#}\n  (queda guardado; revísalo y prueba con huella doctor --online)");
        }
    }
    Ok(())
}

fn choose_llm(prompt: &Prompt, dirs: &Dirs) -> Result<Option<LlmChoice>> {
    let ollama = model::ollama_models(&dirs.ollama);
    let options = [
        "API de Claude (Anthropic)",
        "Ollama en este equipo",
        "Otro servidor compatible con OpenAI (llama.cpp, LM Studio, vLLM…)",
        "Ahora no",
    ];
    let default = usize::from(!ollama.is_empty());
    Ok(match prompt.select("¿Qué modelo usará Huella?", &options, default)? {
        0 => {
            let model = prompt.input("Modelo", Some(huella_decisions::anthropic::DEFAULT_MODEL))?;
            let key_file = if std::env::var_os("ANTHROPIC_API_KEY").is_some() {
                println!("Se usará ANTHROPIC_API_KEY de tu entorno.");
                None
            } else {
                save_key(prompt, "Clave de la API de Claude")?
            };
            Some(LlmChoice { provider: Provider::Anthropic, base_url: None, model: Some(model), key_file })
        }
        1 => {
            let model = if ollama.is_empty() {
                println!("Ollama no responde en {} o no tiene modelos (ollama pull qwen2.5:7b).", dirs.ollama);
                prompt.input("Modelo", Some("qwen2.5:7b"))?
            } else {
                let names: Vec<&str> = ollama.iter().map(String::as_str).collect();
                ollama[prompt.select("Modelo", &names, 0)?].clone()
            };
            Some(LlmChoice {
                provider: Provider::OpenaiCompatible,
                base_url: Some(dirs.ollama_v1()),
                model: Some(model),
                key_file: None,
            })
        }
        2 => {
            let base_url = prompt.input("URL base (termina en /v1)", None)?;
            let model = prompt.input("Modelo", None)?;
            let key_file = save_key(prompt, "Clave (vacía si el servidor no la pide)")?;
            Some(LlmChoice {
                provider: Provider::OpenaiCompatible,
                base_url: Some(base_url),
                model: Some(model),
                key_file,
            })
        }
        _ => None,
    })
}

/// Pide la clave sin eco y la guarda aparte, con permisos 600.
fn save_key(prompt: &Prompt, question: &str) -> Result<Option<PathBuf>> {
    let key = prompt.secret(question)?;
    if key.is_empty() {
        return Ok(None);
    }
    let path = config::default_key_file().context("no se encontró la carpeta personal")?;
    config::write_key_file(&path, &key)?;
    println!("Clave guardada en {} (solo la puedes leer tú).", path.display());
    if std::env::var_os("HUELLA_LLM_API_KEY").is_some() {
        println!("! HUELLA_LLM_API_KEY está definida y manda sobre este fichero.");
    }
    Ok(Some(path))
}

fn embeddings(prompt: &Prompt, dirs: &Dirs, config_path: &Path) -> Result<()> {
    if let Some(embedder) = crate::embedder(None, None) {
        println!("✓ {}", huella_decisions::Embedder::model_id(&embedder));
        return Ok(());
    }
    let ollama = model::ollama_models(&dirs.ollama);
    let embedding_models: Vec<&str> = ollama
        .iter()
        .map(String::as_str)
        .filter(|name| ["embed", "bge", "nomic", "e5"].iter().any(|hint| name.contains(hint)))
        .collect();
    let suggested = !embedding_models.is_empty();
    if !prompt.confirm("¿Activar la búsqueda por significado? Necesita un modelo de embeddings", suggested)? {
        println!("Sin embeddings: la búsqueda es por palabras.");
        return Ok(());
    }
    let (model, base_url) = if suggested {
        (embedding_models[prompt.select("Modelo de embeddings", &embedding_models, 0)?].to_owned(), dirs.ollama_v1())
    } else {
        let base_url = prompt.input("URL base", Some(&dirs.ollama_v1()))?;
        (prompt.input("Modelo de embeddings", Some("bge-m3"))?, base_url)
    };
    model::save_embed(config_path, &model, Some(&base_url))?;
    config::init_from(Some(config_path))?;
    let check = model::embed_check(true);
    println!("{} {}", if check.status == Status::Ok { "✓" } else { "!" }, check.detail);
    Ok(())
}

fn register_agent(prompt: &Prompt, agent: McpAgent, dirs: &Dirs, exe: &Path) -> Result<()> {
    match agent.registered(dirs) {
        Ok(true) => println!("✓ {}", agent.label()),
        Ok(false) => {
            if !prompt.confirm(&format!("¿Registrar Huella en {}?", agent.label()), true)? {
                return Ok(());
            }
            match agent.register(dirs, exe) {
                Ok(()) => println!("✓ {} registrado", agent.label()),
                Err(error) => println!("! {}: {error:#}\n  a mano: {}", agent.label(), agent.manual(dirs, exe)),
            }
        }
        Err(error) => println!("! {}: {error:#}\n  a mano: {}", agent.label(), agent.manual(dirs, exe)),
    }
    Ok(())
}

fn editors_step(prompt: &Prompt, dirs: &Dirs, config_path: &Path, vsix: Option<&Path>) -> Result<()> {
    let code = editors::vscode_cli(dirs);
    if let Some(code) = &code {
        match editors::vscode_extension_installed(code) {
            Ok(true) => println!("✓ VS Code"),
            Ok(false) if prompt.confirm("¿Instalar la extensión de Huella en VS Code?", true)? => {
                match editors::install_vscode_extension(code, vsix) {
                    Ok(()) => println!("✓ VS Code: extensión instalada"),
                    Err(error) => println!("! VS Code: {error:#}"),
                }
            }
            Ok(false) => {}
            Err(error) => println!("! VS Code: {error:#}"),
        }
    }
    let zed = editors::zed_dir(dirs);
    if let Some(zed) = &zed {
        if editors::zed_extension_installed(zed) {
            println!("✓ Zed");
        } else {
            println!("! Zed: {}", editors::ZED_MANUAL);
        }
    }
    if code.is_none() && zed.is_none() {
        println!("No se encontró VS Code ni Zed; la aplicación (huella ui) funciona igual.");
    }

    // A qué editor llevan los enlaces «Abrir en…», si aún no se ha elegido.
    if config::get().ui.editor.is_none() {
        let editor = match (code.is_some(), zed.is_some()) {
            (true, false) => Some("vscode"),
            (false, true) => Some("zed"),
            (true, true) => Some(
                ["zed", "vscode"][prompt.select("¿Qué editor abren los enlaces de Huella?", &["Zed", "VS Code"], 0)?],
            ),
            (false, false) => None,
        };
        if let Some(editor) = editor {
            config::update(config_path, |document| {
                config::table(document.as_table_mut(), "ui")["editor"] = toml_edit::value(editor);
            })?;
            config::init_from(Some(config_path))?;
            println!("Los enlaces abren {editor}.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups_never_overwrite_an_earlier_copy() {
        let dir = std::env::temp_dir().join(format!("huella-wizard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("huella.db");
        let first = backup_path(&db, 11);
        assert_eq!(first.file_name().unwrap(), "huella.pre-v11.db");
        std::fs::write(&first, "").unwrap();
        let second = backup_path(&db, 11);
        assert_ne!(first, second);
        assert!(second.to_string_lossy().contains("huella.pre-v11-"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn setup_with_yes_saves_the_model_and_registers_the_agents_found() {
        let _config = config::TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = std::env::temp_dir().join(format!("huella-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dirs = Dirs::sandbox(&root);
        std::fs::create_dir_all(dirs.home.join(".codex")).unwrap();
        std::fs::write(dirs.home.join(".codex/config.toml"), "# mío\nmodel = \"gpt-5\"\n").unwrap();
        std::fs::create_dir_all(dirs.home.join(".copilot")).unwrap();
        std::fs::create_dir_all(super::super::agents::vscode_user_dir(&dirs)).unwrap();
        let config_path = root.join("data/huella/config.toml");

        let options = SetupOptions {
            yes: true,
            vsix: None,
            skip_index: true,
            db: Some(root.join("data/huella/huella.db")),
            projects: None,
            model: ModelArgs {
                provider: Some(Provider::OpenaiCompatible),
                model: Some("qwen-prueba".to_owned()),
                // Puerto cerrado: la prueba del modelo falla y setup sigue.
                base_url: Some("http://127.0.0.1:9/v1".to_owned()),
            },
        };
        setup(options, &dirs, &config_path, None).unwrap();

        let saved = config::load(&config_path).unwrap();
        assert!(matches!(saved.llm.provider, Some(Provider::OpenaiCompatible)));
        assert_eq!(saved.llm.model.as_deref(), Some("qwen-prueba"));
        for agent in [McpAgent::Codex, McpAgent::CopilotCli, McpAgent::CopilotVscode] {
            assert!(agent.registered(&dirs).unwrap(), "{} sin registrar", agent.label());
        }
        assert!(!McpAgent::ClaudeCode.detected(&dirs), "sin ~/.claude ni claude en el PATH");
        assert!(std::fs::read_to_string(dirs.home.join(".codex/config.toml")).unwrap().contains("# mío"));

        config::init_from(None).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
