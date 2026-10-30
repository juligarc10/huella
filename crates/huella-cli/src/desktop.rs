//! La aplicación de escritorio: la misma interfaz y la misma API que
//! `huella ui`, pero sin servidor ni puerto. La app nativa le pasa cada
//! petición de su protocolo propio y devuelve la respuesta tal cual.
//!
//! Sin red no hacen falta el token ni la comprobación del `Host`: nada fuera
//! de la ventana de la aplicación puede hacer peticiones a este protocolo.

use std::sync::{Mutex, PoisonError};

use anyhow::{Context as _, Result};
use huella_store::Store;

use crate::ui::{self, App};
use crate::{ModelArgs, config};

/// Respuesta a una petición de la interfaz.
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    /// Valor de `Cache-Control`, si hace falta.
    pub cache_control: Option<&'static str>,
    pub body: Vec<u8>,
}

/// La aplicación, lista para responder peticiones desde varios hilos.
pub struct Desktop {
    app: Mutex<App>,
}

impl Desktop {
    /// Abre la base de datos y lee la configuración del usuario, como `huella ui`.
    /// El modelo sale de `config.toml`: una app abierta desde el Dock o el menú
    /// de inicio no hereda las variables de entorno de la terminal.
    ///
    /// # Errors
    /// Si `config.toml` no es válido o la base de datos no se puede abrir.
    pub fn open() -> Result<Self> {
        config::init()?;
        let db = huella_engine::default_db_path().context("no se encontró la carpeta de datos")?;
        let store = Store::open(&db).with_context(|| format!("abriendo {}", db.display()))?;
        let model = ModelArgs { provider: None, model: None, base_url: None };
        let app = App::new(store, db, crate::sources(None)?, config::editor(None), model);
        Ok(Self { app: Mutex::new(app) })
    }

    /// Indexa lo nuevo de las sesiones en otro hilo, con su propia conexión:
    /// la ventana responde mientras tanto.
    pub fn index_in_background(&self) {
        let (db, sources) = {
            let app = self.app.lock().unwrap_or_else(PoisonError::into_inner);
            (app.db().to_owned(), app.sources().clone())
        };
        std::thread::spawn(move || {
            let indexed = Store::open(&db).map_err(anyhow::Error::from).and_then(|mut store| {
                huella_engine::index(&mut store, &sources)?;
                Ok(())
            });
            if let Err(error) = indexed {
                eprintln!("aviso: no se pudieron indexar las sesiones: {error:#}");
            }
        });
    }

    /// Responde una petición de la interfaz: `path` puede llevar `?consulta`.
    pub fn respond(&self, method: &str, path: &str, body: &[u8]) -> Response {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let body = String::from_utf8_lossy(body);
        let mut app = self.app.lock().unwrap_or_else(PoisonError::into_inner);
        let reply = ui::dispatch(&mut app, method, path, query, &body);
        Response {
            status: reply.status,
            content_type: reply.content_type,
            cache_control: reply.cache,
            body: reply.body,
        }
    }

    /// Sesiones que esperan algo de la persona (sin revisar o a medias), para
    /// el icono de la barra de menús.
    ///
    /// # Errors
    /// Si falla la base de datos.
    pub fn pending_reviews(&self) -> Result<usize> {
        let app = self.app.lock().unwrap_or_else(PoisonError::into_inner);
        let inbox = ui::inbox(&app)?;
        Ok(inbox["sessions"].as_array().map_or(0, |sessions| {
            sessions
                .iter()
                .filter(|session| matches!(session["status"].as_str(), Some("pending" | "in_review")))
                .count()
        }))
    }
}
