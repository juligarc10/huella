//! Extensiones de Huella en los editores.
//!
//! VS Code se instala solo, con el `.vsix` de la release o uno local. Zed aún
//! no deja instalar extensiones desde la terminal y la de Huella no está en su
//! registro, así que se detecta y se explica cómo instalarla.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::Dirs;

const VSCODE_ID: &str = "juligarc10.huella";
const VSIX_URL: &str = "https://github.com/juligarc10/huella/releases/latest/download/huella.vsix";

/// La orden `code`, también si VS Code está instalado pero no en el PATH.
pub fn vscode_cli(dirs: &Dirs) -> Option<PathBuf> {
    dirs.on_path("code").or_else(|| {
        let app = dirs.apps.join("Visual Studio Code.app/Contents/Resources/app/bin/code");
        app.exists().then_some(app)
    })
}

pub fn vscode_extension_installed(code: &Path) -> Result<bool> {
    let output = Command::new(code).arg("--list-extensions").output().context("no se pudo ejecutar code")?;
    if !output.status.success() {
        bail!("code --list-extensions terminó con {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).lines().any(|line| line.trim().eq_ignore_ascii_case(VSCODE_ID)))
}

/// Instala el `.vsix` indicado o, si no hay, el de la última release.
pub fn install_vscode_extension(code: &Path, vsix: Option<&Path>) -> Result<()> {
    let downloaded;
    let vsix = if let Some(path) = vsix {
        path
    } else {
        downloaded = download_vsix()?;
        downloaded.as_path()
    };
    let status = Command::new(code)
        .args(["--install-extension"])
        .arg(vsix)
        .arg("--force")
        .status()
        .context("no se pudo ejecutar code --install-extension")?;
    if !status.success() {
        bail!("code --install-extension terminó con {status}");
    }
    Ok(())
}

fn download_vsix() -> Result<PathBuf> {
    let mut response = ureq::get(VSIX_URL).call().with_context(|| format!("no se pudo descargar {VSIX_URL}"))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(50 * 1024 * 1024)
        .read_to_vec()
        .context("la descarga de la extensión se cortó")?;
    // Un .vsix es un zip: si no lo parece, la URL devolvió otra cosa.
    if !bytes.starts_with(b"PK") {
        bail!("{VSIX_URL} no devolvió un .vsix");
    }
    let path = std::env::temp_dir().join(format!("huella-{}.vsix", std::process::id()));
    std::fs::write(&path, bytes).with_context(|| format!("no se pudo guardar {}", path.display()))?;
    Ok(path)
}

/// Carpeta de datos de Zed, si Zed está instalado.
pub fn zed_dir(dirs: &Dirs) -> Option<PathBuf> {
    ["Zed", "zed"].iter().map(|name| dirs.data.join(name)).find(|dir| dir.is_dir())
}

/// Si la extensión de Huella está instalada en Zed (también como extensión de desarrollo).
pub fn zed_extension_installed(zed: &Path) -> bool {
    zed.join("extensions").join("installed").join("huella").exists()
}

pub const ZED_MANUAL: &str = "en Zed, ejecuta «zed: install dev extension» y elige la carpeta extensions/zed del \
     repositorio de Huella (git clone https://github.com/juligarc10/huella)";
