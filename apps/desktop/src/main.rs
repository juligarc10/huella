//! Huella como aplicación de escritorio.
//!
//! La ventana carga la misma interfaz que `huella ui` por un protocolo propio
//! (`huella://`): cada petición, de la página o de su API, la responde la
//! biblioteca de Huella en este mismo proceso. No hay servidor ni puerto.
//!
//! Cerrar la ventana la oculta: la aplicación sigue en la barra de menús
//! contando las sesiones por revisar.

// En Windows, sin consola detrás de la ventana.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::time::Duration;

use huella_cli::desktop::Desktop;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry};

const MAIN_WINDOW: &str = "main";
/// Cada cuánto se recuenta lo pendiente para la barra de menús.
const REFRESH: Duration = Duration::from_secs(60);

fn main() {
    // Si la configuración o la base de datos fallan, la ventana lo explica en
    // lugar de cerrarse sin decir nada.
    let desktop = Desktop::open().map(Arc::new).map_err(|error| format!("{error:#}"));
    if let Ok(desktop) = &desktop {
        desktop.index_in_background();
    }
    let protocol = desktop.clone();

    let app = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("huella", move |_context, request, responder| {
            let desktop = protocol.clone();
            // Algunas peticiones (cargar una revisión) leen las transcripciones:
            // fuera del hilo de la ventana.
            std::thread::spawn(move || responder.respond(respond(desktop.as_deref(), &request)));
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(move |app| {
            open_window(app.handle())?;
            if let Ok(desktop) = &desktop {
                tray(app.handle(), Arc::clone(desktop))?;
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("no se pudo iniciar Huella");

    app.run(on_event);
}

/// macOS: pulsar el icono del Dock con la ventana oculta la vuelve a mostrar.
/// El evento solo existe allí; en Windows y Linux la ventana se recupera desde
/// la bandeja.
#[cfg(target_os = "macos")]
#[allow(clippy::needless_pass_by_value, reason = "la firma la impone el bucle de eventos de Tauri")]
fn on_event(app: &AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::Reopen { .. } = event {
        show_window(app);
    }
}

#[cfg(not(target_os = "macos"))]
#[allow(clippy::needless_pass_by_value, reason = "la firma la impone el bucle de eventos de Tauri")]
fn on_event(_app: &AppHandle, _event: tauri::RunEvent) {}

/// Respuesta del protocolo `huella://` a una petición de la ventana.
fn respond(desktop: Result<&Desktop, &String>, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let desktop = match desktop {
        Ok(desktop) => desktop,
        Err(error) => return startup_error(error),
    };
    let path = request.uri().path_and_query().map_or("/", |path| path.as_str());
    let reply = desktop.respond(request.method().as_str(), path, request.body());
    if cfg!(debug_assertions) {
        eprintln!("{} {path} → {}", request.method(), reply.status);
    }
    let mut response = Response::builder().status(reply.status).header(header::CONTENT_TYPE, reply.content_type);
    if let Some(cache) = reply.cache_control {
        response = response.header(header::CACHE_CONTROL, cache);
    }
    response.body(reply.body).unwrap_or_else(|_| Response::new(Vec::new()))
}

fn startup_error(error: &str) -> Response<Vec<u8>> {
    let escaped = error.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let page = format!(
        "<!doctype html><meta charset=utf-8><title>Huella</title>\
         <body style=\"font:15px system-ui;margin:3rem;max-width:40rem;line-height:1.5\">\
         <h1 style=\"font-size:1.3rem\">Huella no pudo arrancar</h1><pre style=\"white-space:pre-wrap\">{escaped}</pre>\
         <p>Comprueba la instalación con <code>huella doctor</code> en una terminal y vuelve a abrir la aplicación.</p>"
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(page.into_bytes())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// La dirección de la interfaz: en Windows, los protocolos propios se sirven
/// como `http://<protocolo>.localhost`.
fn start_url() -> Url {
    let url = if cfg!(windows) { "http://huella.localhost/" } else { "huella://localhost/" };
    Url::parse(url).expect("dirección válida")
}

fn open_window(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::CustomProtocol(start_url()))
        .title("Huella")
        .inner_size(1280.0, 840.0)
        .min_inner_size(900.0, 600.0)
        .build()?;
    Ok(())
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Icono en la barra de menús (bandeja en Windows y Linux) con lo pendiente.
fn tray(app: &AppHandle, desktop: Arc<Desktop>) -> tauri::Result<()> {
    let pending = MenuItem::with_id(app, "pending", "Comprobando sesiones…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Abrir Huella", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Salir de Huella", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&pending, &separator, &open, &quit])?;
    let mut builder =
        TrayIconBuilder::with_id("main").tooltip("Huella").menu(&menu).show_menu_on_left_click(true).on_menu_event(
            |app, event| match event.id.as_ref() {
                "open" | "pending" => show_window(app),
                "quit" => app.exit(0),
                _ => {}
            },
        );
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;

    std::thread::spawn(move || {
        loop {
            refresh_pending(&desktop, &pending, &tray);
            std::thread::sleep(REFRESH);
        }
    });
    Ok(())
}

fn refresh_pending(desktop: &Desktop, item: &MenuItem<Wry>, tray: &tauri::tray::TrayIcon) {
    let Ok(count) = desktop.pending_reviews() else { return };
    let text = match count {
        0 => "Nada por revisar".to_owned(),
        1 => "1 sesión por revisar".to_owned(),
        n => format!("{n} sesiones por revisar"),
    };
    let _ = item.set_text(&text);
    let _ = tray.set_tooltip(Some(format!("Huella · {text}")));
    // En macOS, el número junto al icono de la barra de menús.
    let _ = tray.set_title((count > 0).then(|| count.to_string()));
}
