//! Configuración del usuario en `config.toml`, junto a la base de datos.
//!
//! Huella se arranca desde cualquier sitio (el editor, el agente por MCP, la
//! terminal), así que su configuración es del usuario, no de un proyecto. Las
//! opciones de la orden y las variables de entorno mandan sobre el fichero.
//! La clave del modelo nunca va en él: o `HUELLA_LLM_API_KEY`, o un fichero
//! aparte (`api_key_file`) que solo pueda leer el usuario.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

static CONFIG: OnceLock<Config> = OnceLock::new();

/// `$HUELLA_CONFIG`, o `config.toml` en la carpeta de datos de Huella.
pub fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HUELLA_CONFIG") {
        return Some(PathBuf::from(path));
    }
    dirs::data_dir().map(|dir| dir.join("huella").join("config.toml"))
}

/// Lee el fichero una vez al arrancar. Si no existe, todo queda por defecto;
/// si está mal escrito, falla en vez de ignorarlo en silencio.
pub fn init() -> Result<()> {
    let config = match path() {
        Some(path) if path.exists() => load(&path)?,
        _ => Config::default(),
    };
    let _ = CONFIG.set(config);
    Ok(())
}

pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| format!("no se pudo leer {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("{} no es válido", path.display()))
}

pub fn get() -> &'static Config {
    CONFIG.get_or_init(Config::default)
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
             api_key_file = \"~/.config/huella/key\"\n\n[embed]\nmodel = \"bge-m3\"\n\n[ui]\neditor = \"vscode\"\n",
        )
        .unwrap();
        let config = load(&path).unwrap();
        assert!(matches!(config.llm.provider, Some(Provider::OpenaiCompatible)));
        assert_eq!(config.llm.model.as_deref(), Some("qwen2.5:7b"));
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
}
