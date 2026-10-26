//! Servidor LSP de Huella: muestra en el editor lo que se habló de cada símbolo.
//!
//! - **Hover**: al pasar el ratón por una función, las ediciones que la
//!   tocaron, con la petición y la explicación del agente.
//! - **`CodeLens`**: encima de cada símbolo con historia, cuántas ediciones y
//!   sesiones lo tocaron.
//!
//! Un hilo aparte indexa las transcripciones nuevas periódicamente con su
//! propia conexión a SQLite (WAL permite leer mientras se escribe) y pide al
//! editor que refresque los `CodeLens` cuando hay novedades.

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, select};
use huella_store::Store;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _};
use lsp_types::request::{CodeLensRefresh, CodeLensRequest, HoverRequest, Request as _};
use lsp_types::{
    CodeLens, CodeLensOptions, Command, Hover, HoverContents, HoverProviderCapability, InitializeParams,
    InitializeResult, MarkupContent, MarkupKind, Position, Range, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

/// Ediciones que se muestran en el hover; el resto se resume en una línea.
const HOVER_ENTRIES: usize = 3;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("error del protocolo LSP: {0}")]
    Protocol(#[from] lsp_server::ProtocolError),
    #[error("respuesta inválida: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] huella_store::StoreError),
    #[error("el canal con el editor se cerró")]
    Disconnected,
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ServerError>;

/// Configuración del servidor.
#[derive(Debug, Clone)]
pub struct Config {
    pub db: PathBuf,
    /// Transcripciones a indexar en segundo plano; `None` desactiva el indexado.
    pub projects: Option<PathBuf>,
    pub index_every: Duration,
}

/// Sirve por stdin/stdout hasta que el editor cierra la sesión.
///
/// # Errors
/// Si falla el protocolo o no se puede abrir la base de datos.
pub fn serve_stdio(config: &Config) -> Result<()> {
    let (connection, io_threads) = Connection::stdio();
    serve(&connection, config)?;
    drop(connection);
    io_threads.join()?;
    Ok(())
}

/// Bucle principal sobre una conexión ya abierta (stdio o memoria en tests).
///
/// # Errors
/// Si falla el protocolo o no se puede abrir la base de datos.
pub fn serve(connection: &Connection, config: &Config) -> Result<()> {
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        code_lens_provider: Some(CodeLensOptions { resolve_provider: Some(false) }),
        ..ServerCapabilities::default()
    };
    let (initialize_id, params) = connection.initialize_start()?;
    let result = InitializeResult {
        capabilities,
        server_info: Some(ServerInfo { name: "huella".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
    };
    connection.initialize_finish(initialize_id, serde_json::to_value(result)?)?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let can_refresh_lenses = params
        .capabilities
        .workspace
        .and_then(|workspace| workspace.code_lens)
        .and_then(|lens| lens.refresh_support)
        .unwrap_or(false);

    let mut server = Server { store: Store::open(&config.db)?, documents: HashMap::new(), next_request: 0 };
    let indexed = spawn_indexer(config);
    loop {
        select! {
            recv(connection.receiver) -> message => {
                let message = message.map_err(|_| ServerError::Disconnected)?;
                if let Message::Request(request) = &message
                    && connection.handle_shutdown(request)?
                {
                    return Ok(());
                }
                server.handle(connection, message)?;
            }
            recv(indexed) -> new_edits => {
                if new_edits.is_ok_and(|count| count > 0) && can_refresh_lenses {
                    server.request(connection, CodeLensRefresh::METHOD, serde_json::Value::Null)?;
                }
            }
        }
    }
}

/// Indexa al arrancar y luego cada `index_every`. Avisa de cuántas ediciones
/// nuevas encontró. Los fallos se registran en stderr (el log del editor) sin
/// tumbar el servidor: la memoria ya indexada sigue sirviéndose.
fn spawn_indexer(config: &Config) -> Receiver<usize> {
    let (sender, receiver) = crossbeam_channel::unbounded();
    let Some(projects) = config.projects.clone() else { return receiver };
    let (db, every) = (config.db.clone(), config.index_every);
    thread::spawn(move || {
        let mut store = match Store::open(&db) {
            Ok(store) => store,
            Err(error) => return eprintln!("huella: no se pudo abrir {} para indexar: {error}", db.display()),
        };
        loop {
            match huella_engine::index(&mut store, &projects) {
                Ok(report) => {
                    if sender.send(report.new_edits).is_err() {
                        return;
                    }
                }
                Err(error) => eprintln!("huella: fallo al indexar: {error}"),
            }
            thread::sleep(every);
        }
    });
    receiver
}

struct Server {
    store: Store,
    /// Contenido actual de los ficheros abiertos, que puede no estar guardado.
    documents: HashMap<Uri, String>,
    next_request: i32,
}

impl Server {
    fn handle(&mut self, connection: &Connection, message: Message) -> Result<()> {
        match message {
            Message::Request(request) => {
                let response = self.respond(request);
                connection.sender.send(Message::Response(response)).map_err(|_| ServerError::Disconnected)
            }
            Message::Notification(notification) => {
                self.notify(notification);
                Ok(())
            }
            Message::Response(_) => Ok(()),
        }
    }

    fn respond(&self, request: Request) -> Response {
        let id = request.id.clone();
        let result = match request.method.as_str() {
            HoverRequest::METHOD => parse::<lsp_types::HoverParams>(request).map(|params| {
                let position = &params.text_document_position_params;
                serde_json::to_value(self.hover(&position.text_document.uri, position.position))
            }),
            CodeLensRequest::METHOD => parse::<lsp_types::CodeLensParams>(request)
                .map(|params| serde_json::to_value(self.code_lenses(&params.text_document.uri))),
            _ => return Response::new_err(id, ErrorCode::MethodNotFound as i32, "método no soportado".into()),
        };
        match result {
            Ok(Ok(value)) => Response::new_ok(id, value),
            Ok(Err(error)) => Response::new_err(id, ErrorCode::InternalError as i32, error.to_string()),
            Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
        }
    }

    fn notify(&mut self, notification: Notification) {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(params) = parse_notification::<lsp_types::DidOpenTextDocumentParams>(notification) {
                    self.documents.insert(params.text_document.uri, params.text_document.text);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(mut params) = parse_notification::<lsp_types::DidChangeTextDocumentParams>(notification)
                    && let Some(change) = params.content_changes.pop()
                {
                    self.documents.insert(params.text_document.uri, change.text);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(params) = parse_notification::<lsp_types::DidCloseTextDocumentParams>(notification) {
                    self.documents.remove(&params.text_document.uri);
                }
            }
            _ => {}
        }
    }

    fn hover(&self, uri: &Uri, position: Position) -> Option<Hover> {
        let (path, source) = self.document(uri)?;
        let line = usize::try_from(position.line).ok()? + 1;
        let memory = huella_engine::memory_at(&self.store, &path, source, line)
            .inspect_err(|error| eprintln!("huella: hover en {}: {error}", path.display()))
            .ok()??;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: huella_engine::render_markdown(&memory, HOVER_ENTRIES),
            }),
            range: Some(line_range(memory.symbol.start_line, memory.symbol.end_line)),
        })
    }

    fn code_lenses(&self, uri: &Uri) -> Vec<CodeLens> {
        let Some((path, source)) = self.document(uri) else { return Vec::new() };
        let lenses = huella_engine::lenses(&self.store, &path, source)
            .inspect_err(|error| eprintln!("huella: CodeLens en {}: {error}", path.display()))
            .unwrap_or_default();
        lenses
            .into_iter()
            .map(|lens| CodeLens {
                range: line_range(lens.symbol.start_line, lens.symbol.start_line),
                command: Some(Command {
                    title: lens_title(&lens.activity),
                    // Sin comando: el texto informa y el hover da el detalle.
                    command: String::new(),
                    arguments: None,
                }),
                data: None,
            })
            .collect()
    }

    fn document(&self, uri: &Uri) -> Option<(PathBuf, &str)> {
        let source = self.documents.get(uri)?;
        Some((uri_to_path(uri.as_str())?, source.as_str()))
    }

    fn request(&mut self, connection: &Connection, method: &str, params: serde_json::Value) -> Result<()> {
        self.next_request += 1;
        let request = Request::new(RequestId::from(format!("huella-{}", self.next_request)), method.into(), params);
        connection.sender.send(Message::Request(request)).map_err(|_| ServerError::Disconnected)
    }
}

fn parse<P: serde::de::DeserializeOwned>(request: Request) -> std::result::Result<P, serde_json::Error> {
    serde_json::from_value(request.params)
}

fn parse_notification<P: serde::de::DeserializeOwned>(
    notification: Notification,
) -> std::result::Result<P, serde_json::Error> {
    serde_json::from_value(notification.params)
}

fn lens_title(activity: &huella_store::SymbolActivity) -> String {
    let edits = if activity.edits == 1 { "1 edición".to_owned() } else { format!("{} ediciones", activity.edits) };
    let sessions =
        if activity.sessions == 1 { "1 sesión".to_owned() } else { format!("{} sesiones", activity.sessions) };
    let last = activity.last_at.as_deref().and_then(|at| at.get(..10)).map(|day| format!(" · última {day}"));
    format!("Huella · {edits} · {sessions}{}", last.unwrap_or_default())
}

/// Líneas 1-based e inclusivas → rango LSP (0-based, fin exclusivo).
fn line_range(start_line: usize, end_line: usize) -> Range {
    let to_lsp = |line: usize| u32::try_from(line.saturating_sub(1)).unwrap_or(u32::MAX);
    Range::new(Position::new(to_lsp(start_line), 0), Position::new(to_lsp(end_line) + 1, 0))
}

/// `file:///Users/ana/mi%20repo/a.ts` → `/Users/ana/mi repo/a.ts`.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = encoded.get(index + 1..index + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    let path = PathBuf::from(String::from_utf8(decoded).ok()?);
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests;
