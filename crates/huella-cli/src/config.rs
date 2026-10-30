//! Configuración del usuario en `config.toml`, junto a la base de datos.
//!
//! Huella se arranca desde cualquier sitio (el editor, el agente por MCP, la
//! terminal), así que su configuración es del usuario, no de un proyecto. Las
//! opciones de la orden y las variables de entorno mandan sobre el fichero.
//! La clave del modelo nunca va en él: o `HUELLA_LLM_API_KEY`, o un fichero
//! aparte (`api_key_file`) que solo pueda leer el usuario.

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::Provider;
use crate::web::Editor;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub llm: Llm,
    pub embed: Embed,
    pub ui: Ui,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Llm {
    pub provider: Option<Provider>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// Fichero con la clave del modelo; debe tener permisos `600`.
    pub api_key_file: Option<PathBuf>,
    /// Razonamiento del modelo (`false` lo desactiva en Qwen3 y similares);
    /// sin definir, el servidor decide.
    pub thinking: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Embed {
    pub model: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub editor: Option<Editor>,
}

// Se puede recargar: `huella setup` la cambia y sigue usándola en el mismo proceso.
static CONFIG: RwLock<Option<Arc<Config>>> = RwLock::new(None);

/// `$HUELLA_CONFIG`, o `config.toml` en la carpeta de datos de Huella.
pub fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HUELLA_CONFIG") {
        return Some(PathBuf::from(path));
    }
    dirs::data_dir().map(|dir| dir.join("huella").join("config.toml"))
}

/// Lee el fichero al arrancar (y tras cambiarlo). Si no existe, todo queda
/// por defecto; si está mal escrito, falla en vez de ignorarlo en silencio.
pub fn init() -> Result<()> {
    init_from(path().as_deref())
}

/// Como `init`, desde un fichero concreto (el que acaba de escribir `setup`).
pub fn init_from(path: Option<&Path>) -> Result<()> {
    let config = match path {
        Some(path) if path.exists() => load(path)?,
        _ => Config::default(),
    };
    *CONFIG.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(config));
    Ok(())
}

/// Los tests que cambian o dependen de la configuración global no corren a la vez.
#[cfg(test)]
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| format!("no se pudo leer {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("{} no es válido", path.display()))
}

pub fn get() -> Arc<Config> {
    CONFIG.read().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or_default()
}

/// Editor de la orden, o el del fichero, o Zed.
pub fn editor(arg: Option<Editor>) -> Editor {
    arg.or(get().ui.editor).unwrap_or(Editor::Zed)
}

/// Clave del modelo: `env_var` si está definida y, si no, `api_key_file`.
pub fn api_key(env_var: &str) -> Result<Option<String>> {
    if let Some(key) = std::env::var(env_var).ok().filter(|key| !key.trim().is_empty()) {
        return Ok(Some(key));
    }
    get().llm.api_key_file.as_deref().map(read_key_file).transpose()
}

pub fn read_key_file(path: &Path) -> Result<String> {
    let path = expand_home(path);
    check_private(&path)?;
    let key =
        std::fs::read_to_string(&path).with_context(|| format!("no se pudo leer la clave en {}", path.display()))?;
    let key = key.trim();
    if key.is_empty() {
        bail!("{} está vacío", path.display());
    }
    Ok(key.to_owned())
}

/// Cambia `config.toml` conservando lo que el usuario escribió a mano
/// (comentarios, orden, otras claves). Lo crea si no existe.
pub fn update(path: &Path, edit: impl FnOnce(&mut toml_edit::DocumentMut)) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("no se pudo leer {}", path.display())),
    };
    let mut document: toml_edit::DocumentMut =
        text.parse().with_context(|| format!("{} no es válido", path.display()))?;
    edit(&mut document);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("no se pudo crear {}", parent.display()))?;
    }
    std::fs::write(path, document.to_string()).with_context(|| format!("no se pudo escribir {}", path.display()))
}

/// La tabla `[key]` de `parent`, creándola si falta. Siempre como sección
/// (`[llm]`), no en línea (`llm = { … }`): el fichero lo lee y edita una persona.
pub fn table<'a>(parent: &'a mut toml_edit::Table, key: &str) -> &'a mut toml_edit::Table {
    let item = parent.entry(key).or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(inline) = item.as_inline_table().cloned() {
        *item = toml_edit::Item::Table(inline.into_table());
    }
    if !item.is_table() {
        *item = toml_edit::Item::Table(toml_edit::Table::new());
    }
    item.as_table_mut().expect("acaba de convertirse en tabla")
}

/// Dónde guarda `huella setup` la clave del modelo: fuera de `config.toml`.
pub fn default_key_file() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("huella").join("llm-key"))
}

/// Guarda la clave con permisos `600` desde el primer momento: nunca existe
/// una versión del fichero que otros puedan leer.
pub fn write_key_file(path: &Path, key: &str) -> Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("no se pudo crear {}", parent.display()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| format!("no se pudo escribir {}", path.display()))?;
    #[cfg(unix)]
    {
        // Si ya existía con otros permisos, `mode` no los cambia.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    writeln!(file, "{}", key.trim())?;
    Ok(())
}

fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), dirs::home_dir()) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_owned(),
    }
}

/// Una clave que otros usuarios pueden leer no se usa: se avisa de cómo arreglarlo.
#[cfg(unix)]
fn check_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .with_context(|| format!("no existe el fichero de la clave {}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        bail!("{} lo pueden leer otros usuarios: ejecuta chmod 600 {}", path.display(), path.display());
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_private(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("huella-config-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_every_section() {
        let path = temp("ok").join("config.toml");
        std::fs::write(
            &path,
            "[llm]\nprovider = \"openai-compatible\"\nbase_url = \"http://localhost:11434/v1\"\nmodel = \"qwen2.5:7b\"\n\
             api_key_file = \"~/.config/huella/key\"\nthinking = false\n\n[embed]\nmodel = \"bge-m3\"\n\n[ui]\neditor = \"vscode\"\n",
        )
        .unwrap();
        let config = load(&path).unwrap();
        assert!(matches!(config.llm.provider, Some(Provider::OpenaiCompatible)));
        assert_eq!(config.llm.model.as_deref(), Some("qwen2.5:7b"));
        assert_eq!(config.llm.thinking, Some(false));
        assert_eq!(config.embed.model.as_deref(), Some("bge-m3"));
        assert!(matches!(config.ui.editor, Some(Editor::Vscode)));
    }

    #[test]
    fn rejects_unknown_keys_instead_of_ignoring_them() {
        let path = temp("typo").join("config.toml");
        std::fs::write(&path, "[llm]\nmodelo = \"qwen\"\n").unwrap();
        assert!(load(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn key_files_must_be_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp("key").join("key");
        std::fs::write(&path, "  sk-prueba\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_key_file(&path).unwrap_err().to_string().contains("chmod 600"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_key_file(&path).unwrap(), "sk-prueba");
    }

    #[cfg(unix)]
    #[test]
    fn written_key_files_are_private_even_if_they_existed() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp("write").join("sub/llm-key");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "vieja").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_key_file(&path, " nueva \n").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(read_key_file(&path).unwrap(), "nueva");
    }

    #[test]
    fn updates_keep_what_the_user_wrote() {
        let path = temp("update").join("config.toml");
        std::fs::write(&path, "# mi configuración\n[ui]\neditor = \"zed\" # el de siempre\n").unwrap();
        update(&path, |doc| {
            table(doc.as_table_mut(), "llm")["model"] = toml_edit::value("qwen2.5:7b");
        })
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mi configuración") && text.contains("# el de siempre"));
        assert!(text.contains("[llm]\nmodel = \"qwen2.5:7b\""), "{text}");
        let config = load(&path).unwrap();
        assert_eq!(config.llm.model.as_deref(), Some("qwen2.5:7b"));
        assert!(matches!(config.ui.editor, Some(Editor::Zed)));
    }
}
