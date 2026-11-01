//! Servidor LSP de Huella: muestra en el editor lo que se habló de cada símbolo.
//!
//! - **Hover**: al pasar el ratón por una función, las ediciones que la
//!   tocaron, con la petición y la explicación del agente.
//! - **`CodeLens`**: encima de cada símbolo con historia, cuántas ediciones y
//!   sesiones lo tocaron, y cuántas preguntas se hicieron de cada trozo.
//! - **Preguntar**: una acción de código sobre cualquier selección abre
//!   Huella para preguntar por esas líneas; las respuestas salen en el hover.
//!
//! Un hilo aparte indexa las transcripciones nuevas periódicamente con su
//! propia conexión a SQLite (WAL permite leer mientras se escribe) y pide al
//! editor que refresque los `CodeLens` cuando hay novedades.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

mod review;

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
/// Caracteres de cada respuesta en el hover.
const HOVER_ANSWER: usize = 1500;
/// Comando de la acción «preguntar sobre estas líneas».
pub const ASK_CODE: &str = "huella.askCode";

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
    /// Agentes cuyas sesiones se indexan en segundo plano; `None` desactiva el indexado.
    pub sources: Option<huella_engine::Sources>,
    pub index_every: Duration,
    /// Abre Huella para preguntar sobre unas líneas; `None` quita la acción.
    pub ask: Option<AskOpener>,
}

/// Líneas sobre las que se quiere preguntar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskTarget {
    pub path: PathBuf,
    /// 1-based e inclusivas.
    pub start_line: usize,
    pub end_line: usize,
    /// Nombre del editor (`clientInfo`), para que Huella sepa adónde volver.
    pub client: Option<String>,
}

/// Abre la página de preguntar de Huella. El servidor LSP no puede pedir
/// texto al editor: la pregunta se escribe allí.
#[derive(Clone)]
pub struct AskOpener(pub Arc<OpenAsk>);

/// Abre Huella sobre esas líneas, o dice por qué no pudo.
pub type OpenAsk = dyn Fn(&AskTarget) -> std::result::Result<(), String> + Send + Sync;

impl std::fmt::Debug for AskOpener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AskOpener")
    }
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
        code_action_provider: Some(lsp_types::CodeActionProviderCapability::Simple(true)),
        execute_command_provider: Some(lsp_types::ExecuteCommandOptions {
            commands: vec![review::MARK_REVIEWED.to_owned(), ASK_CODE.to_owned()],
            ..lsp_types::ExecuteCommandOptions::default()
        }),
        ..ServerCapabilities::default()
    };
    let (initialize_id, params) = connection.initialize_start()?;
    let result = InitializeResult {
        capabilities,
        server_info: Some(ServerInfo { name: "huella".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
    };
    connection.initialize_finish(initialize_id, serde_json::to_value(result)?)?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let client = params.client_info.as_ref().map(|info| info.name.clone());
    let can_refresh_lenses = params
        .capabilities
        .workspace
        .and_then(|workspace| workspace.code_lens)
        .and_then(|lens| lens.refresh_support)
        .unwrap_or(false);

    let mut server = Server {
        store: Store::open(&config.db)?,
        documents: HashMap::new(),
        next_request: 0,
        sources: config.sources.clone(),
        review: None,
        published: HashSet::new(),
        ask: config.ask.clone(),
        client,
    };
    server.reload_review();
    if server.review.is_some() {
        server.sync_workspace();
        server.publish_all(connection)?;
    }
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
                let new_edits = new_edits.unwrap_or_default();
                if new_edits > 0 && can_refresh_lenses {
                    server.request(connection, CodeLensRefresh::METHOD, serde_json::Value::Null)?;
                }
                // La revisión activa puede haber cambiado (otra sesión, o la misma con más ediciones).
                let reloaded = server.reload_review_if_changed(new_edits > 0);
                if server.sync_workspace() || reloaded {
                    server.publish_all(connection)?;
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
    let Some(sources) = config.sources.clone() else { return receiver };
    let (db, every) = (config.db.clone(), config.index_every);
    thread::spawn(move || {
        let mut store = match Store::open(&db) {
            Ok(store) => store,
            Err(error) => return eprintln!("huella: no se pudo abrir {} para indexar: {error}", db.display()),
        };
        loop {
            match huella_engine::index(&mut store, &sources) {
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
    sources: Option<huella_engine::Sources>,
    /// Revisión activa (`huella review start`), si la hay.
    review: Option<review::ActiveReview>,
    /// Documentos con diagnosis publicadas, para poder vaciarlas.
    published: HashSet<String>,
    ask: Option<AskOpener>,
    /// Nombre del editor.
    client: Option<String>,
}

impl Server {
    fn handle(&mut self, connection: &Connection, message: Message) -> Result<()> {
        match message {
            Message::Request(request) => {
                let marks_changed = request.method == lsp_types::request::ExecuteCommand::METHOD;
                let response = self.respond(request);
                connection.sender.send(Message::Response(response)).map_err(|_| ServerError::Disconnected)?;
                if marks_changed {
                    self.publish_all(connection)?;
                }
                Ok(())
            }
            Message::Notification(notification) => {
                if let Some(uri) = self.notify(notification) {
                    self.publish(connection, &uri)?;
                }
                Ok(())
            }
            Message::Response(_) => Ok(()),
        }
    }

    fn reload_review(&mut self) {
        self.review = self.sources.as_ref().and_then(|sources| review::ActiveReview::load(&self.store, sources));
    }

    /// Lleva a marcas lo preparado en el worktree de revisión. Devuelve si cambió algo.
    fn sync_workspace(&self) -> bool {
        self.review.as_ref().is_some_and(|review| review.sync_workspace(&self.store))
    }

    /// Recarga la revisión si cambió la sesión activa o si hay ediciones
    /// nuevas. Devuelve si hay que volver a publicar las diagnosis.
    fn reload_review_if_changed(&mut self, new_edits: bool) -> bool {
        let active = self.store.active_review().ok().flatten();
        let loaded = self.review.as_ref().map(|r| r.session.clone());
        if active != loaded || (active.is_some() && new_edits) {
            self.reload_review();
            return true;
        }
        false
    }

    /// Pasos pendientes de la revisión activa en un fichero, abierto o no (los
    /// cerrados se leen del disco).
    fn publish(&self, connection: &Connection, uri: &Uri) -> Result<()> {
        let diagnostics = match (&self.review, uri_to_path(uri.as_str())) {
            (Some(review), Some(path)) => {
                let on_disk;
                let source = if let Some(text) = self.documents.get(uri) {
                    text.as_str()
                } else {
                    on_disk = std::fs::read_to_string(&path).unwrap_or_default();
                    on_disk.as_str()
                };
                review.diagnostics(&self.store, &huella_engine::normalize_path(&path, None), source)
            }
            _ => Vec::new(),
        };
        let params = lsp_types::PublishDiagnosticsParams { uri: uri.clone(), diagnostics, version: None };
        let notification = Notification::new(
            lsp_types::notification::PublishDiagnostics::METHOD.into(),
            serde_json::to_value(params)?,
        );
        connection.sender.send(Message::Notification(notification)).map_err(|_| ServerError::Disconnected)
    }

    /// Publica los pasos de todos los ficheros de la revisión y de los
    /// abiertos, y vacía los que ya no tienen pasos (revisión cambiada o parada).
    /// Así el panel de diagnósticos del editor lista toda la revisión.
    fn publish_all(&mut self, connection: &Connection) -> Result<()> {
        // Claves de texto: `Uri` tiene mutabilidad interior y no conviene en un `HashSet`.
        let mut targets: HashSet<String> = self.documents.keys().map(|uri| uri.as_str().to_owned()).collect();
        if let Some(review) = &self.review {
            targets
                .extend(review.file_paths().filter_map(|path| path_to_uri(&path)).map(|uri| uri.as_str().to_owned()));
        }
        for uri in targets.union(&self.published).filter_map(|uri| uri.parse::<Uri>().ok()) {
            self.publish(connection, &uri)?;
        }
        self.published = targets;
        Ok(())
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
            lsp_types::request::CodeActionRequest::METHOD => parse::<lsp_types::CodeActionParams>(request)
                .map(|params| serde_json::to_value(self.code_actions(&params))),
            lsp_types::request::ExecuteCommand::METHOD => parse::<lsp_types::ExecuteCommandParams>(request)
                .map(|params| serde_json::to_value(self.execute(&params))),
            _ => return Response::new_err(id, ErrorCode::MethodNotFound as i32, "método no soportado".into()),
        };
        match result {
            Ok(Ok(value)) => Response::new_ok(id, value),
            Ok(Err(error)) => Response::new_err(id, ErrorCode::InternalError as i32, error.to_string()),
            Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
        }
    }

    /// Aplica la notificación y devuelve el documento cuyas diagnosis hay que
    /// volver a publicar.
    fn notify(&mut self, notification: Notification) -> Option<Uri> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params = parse_notification::<lsp_types::DidOpenTextDocumentParams>(notification).ok()?;
                self.documents.insert(params.text_document.uri.clone(), params.text_document.text);
                Some(params.text_document.uri)
            }
            DidChangeTextDocument::METHOD => {
                let mut params = parse_notification::<lsp_types::DidChangeTextDocumentParams>(notification).ok()?;
                let change = params.content_changes.pop()?;
                self.documents.insert(params.text_document.uri.clone(), change.text);
                Some(params.text_document.uri)
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(params) = parse_notification::<lsp_types::DidCloseTextDocumentParams>(notification) {
                    self.documents.remove(&params.text_document.uri);
                }
                None
            }
            _ => None,
        }
    }

    /// Las de la revisión activa (marcar pasos) y, con cualquier selección,
    /// preguntar sobre esas líneas.
    fn code_actions(&self, params: &lsp_types::CodeActionParams) -> Vec<lsp_types::CodeActionOrCommand> {
        let mut actions = code_actions(&params.context.diagnostics);
        if self.ask.is_some() && uri_to_path(params.text_document.uri.as_str()).is_some() {
            let range = params.range;
            let start = range.start.line as usize + 1;
            // Una selección que acaba al principio de una línea no la incluye.
            let end = if range.end.line > range.start.line && range.end.character == 0 {
                range.end.line as usize
            } else {
                range.end.line as usize + 1
            };
            let lines = if start == end { "esta línea".to_owned() } else { format!("las líneas {start}–{end}") };
            actions.push(lsp_types::CodeActionOrCommand::CodeAction(lsp_types::CodeAction {
                title: format!("Huella: preguntar sobre {lines}"),
                command: Some(Command {
                    title: "Preguntar en Huella".into(),
                    command: ASK_CODE.into(),
                    arguments: Some(vec![serde_json::json!({
                        "uri": params.text_document.uri.as_str(), "start": start, "end": end,
                    })]),
                }),
                ..lsp_types::CodeAction::default()
            }));
        }
        actions
    }

    /// `huella.markReviewed` guarda la marca del paso de la diagnosis;
    /// `huella.askCode` abre Huella para preguntar.
    fn execute(&self, params: &lsp_types::ExecuteCommandParams) -> Option<serde_json::Value> {
        if params.command == ASK_CODE {
            let data = params.arguments.first()?;
            let line = |key: &str| data[key].as_u64().and_then(|n| usize::try_from(n).ok());
            let target = AskTarget {
                path: uri_to_path(data["uri"].as_str()?)?,
                start_line: line("start")?,
                end_line: line("end")?,
                client: self.client.clone(),
            };
            if let Some(Err(error)) = self.ask.as_ref().map(|ask| (ask.0)(&target)) {
                eprintln!("huella: no se pudo abrir Huella para preguntar: {error}");
            }
            return None;
        }
        let review = self.review.as_ref()?;
        if params.command != review::MARK_REVIEWED {
            return None;
        }
        let data = params.arguments.first()?;
        let (key, hash) = (data["key"].as_str()?, data["hash"].as_str()?);
        if let Err(error) = self.store.set_review_mark(&review.session, key, hash, true) {
            eprintln!("huella: no se pudo marcar el paso: {error}");
        }
        None
    }

    /// La historia del símbolo bajo el cursor y las preguntas que se hicieron
    /// sobre esa línea.
    fn hover(&self, uri: &Uri, position: Position) -> Option<Hover> {
        let (path, source) = self.document(uri)?;
        let line = usize::try_from(position.line).ok()? + 1;
        let memory = huella_engine::memory_at(&self.store, &path, source, line)
            .inspect_err(|error| eprintln!("huella: hover en {}: {error}", path.display()))
            .ok()
            .flatten();
        let questions: Vec<_> = huella_engine::code_ask::questions_in(&self.store, &path, source)
            .inspect_err(|error| eprintln!("huella: preguntas de {}: {error}", path.display()))
            .unwrap_or_default()
            .into_iter()
            .filter(|located| located.lines.is_some_and(|(start, end)| (start..=end).contains(&line)))
            .collect();
        let mut value =
            memory.as_ref().map(|memory| huella_engine::render_markdown(memory, HOVER_ENTRIES)).unwrap_or_default();
        for located in &questions {
            if !value.is_empty() {
                value.push_str("\n\n---\n\n");
            }
            let question = &located.question;
            let _ = write!(
                value,
                "**Preguntaste:** {}\n\n{}\n\n_{} · {}_",
                question.question,
                clip(&question.answer, HOVER_ANSWER),
                question.model.split_once(':').map_or(question.model.as_str(), |(_, model)| model),
                question.created_at.get(..10).unwrap_or_default()
            );
        }
        if value.is_empty() {
            return None;
        }
        let range = match (&memory, questions.first().and_then(|located| located.lines)) {
            (Some(memory), _) => line_range(memory.symbol.start_line, memory.symbol.end_line),
            (None, Some((start, end))) => line_range(start, end),
            (None, None) => return None,
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value }),
            range: Some(range),
        })
    }

    fn code_lenses(&self, uri: &Uri) -> Vec<CodeLens> {
        let Some((path, source)) = self.document(uri) else { return Vec::new() };
        let lenses = huella_engine::lenses(&self.store, &path, source)
            .inspect_err(|error| eprintln!("huella: CodeLens en {}: {error}", path.display()))
            .unwrap_or_default();
        // Sin comando: el texto informa y el hover da el detalle.
        let lens = |line: usize, title: String| CodeLens {
            range: line_range(line, line),
            command: Some(Command { title, command: String::new(), arguments: None }),
            data: None,
        };
        let mut questions: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
        for located in huella_engine::code_ask::questions_in(&self.store, &path, source).unwrap_or_default() {
            if let Some((start, _)) = located.lines {
                *questions.entry(start).or_default() += 1;
            }
        }
        lenses
            .into_iter()
            .map(|found| lens(found.symbol.start_line, lens_title(&found.activity)))
            .chain(questions.into_iter().map(|(line, count)| {
                lens(
                    line,
                    if count == 1 { "Huella · 1 pregunta".into() } else { format!("Huella · {count} preguntas") },
                )
            }))
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

/// Una acción "marcar como revisado" por cada paso pendiente bajo el cursor.
fn code_actions(diagnostics: &[lsp_types::Diagnostic]) -> Vec<lsp_types::CodeActionOrCommand> {
    diagnostics
        .iter()
        .filter(|d| d.source.as_deref() == Some(review::SOURCE))
        .filter_map(|d| {
            let data = d.data.clone()?;
            let step = d.code.as_ref().map(|code| match code {
                lsp_types::NumberOrString::String(code) => code.replace('-', " "),
                lsp_types::NumberOrString::Number(number) => number.to_string(),
            });
            Some(lsp_types::CodeActionOrCommand::CodeAction(lsp_types::CodeAction {
                title: format!("Huella: marcar {} como revisado", step.unwrap_or_else(|| "el paso".into())),
                kind: Some(lsp_types::CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![d.clone()]),
                command: Some(Command {
                    title: "Marcar como revisado".into(),
                    command: review::MARK_REVIEWED.into(),
                    arguments: Some(vec![data]),
                }),
                ..lsp_types::CodeAction::default()
            }))
        })
        .collect()
}

fn parse<P: serde::de::DeserializeOwned>(request: Request) -> std::result::Result<P, serde_json::Error> {
    serde_json::from_value(request.params)
}

fn parse_notification<P: serde::de::DeserializeOwned>(
    notification: Notification,
) -> std::result::Result<P, serde_json::Error> {
    serde_json::from_value(notification.params)
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
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

/// `/Users/ana/mi repo/a.ts` → `file:///Users/ana/mi%20repo/a.ts`.
fn path_to_uri(path: &str) -> Option<Uri> {
    let mut encoded = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded.parse().ok()
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
