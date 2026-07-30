//! A development-only HTTP door onto the till.
//!
//! ## Why this is not the browser mock the architecture forbids
//!
//! The rule is that TypeScript never computes a total, and the reason there is no browser fallback
//! is that a fallback would be a *second implementation of the money rules* — the exact drift the
//! Rust core exists to prevent. This is not that. It forwards a request to the same
//! `sahl-terminal` code the Tauri commands call, over a socket instead of an IPC channel. There is
//! no second implementation of anything; the till is the till.
//!
//! What it buys is that every screen becomes verifiable outside the Tauri shell. macOS uses
//! WKWebView, which speaks Safari's inspector protocol rather than Chrome's, so a browser cannot
//! attach to the real window at all — and layout is the only thing that can be checked without one.
//! Several real bugs in this product were found by measuring a rendered page, so being able to do
//! that against real data is worth a small, tightly fenced surface.
//!
//! ## The fence
//!
//! Three independent conditions, all required:
//!
//! 1. `#[cfg(debug_assertions)]` — the module does not exist in a release build.
//! 2. `SAHL_DEV_BRIDGE=1` — even a debug build opens no port unless asked.
//! 3. Bound to `127.0.0.1` — never a routable interface.
//!
//! It is also deliberately unauthenticated, which is exactly why it must never ship: anything that
//! can reach the port can ring a sale. The three conditions above are what make that acceptable on
//! a developer's own machine and unacceptable anywhere else.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use crate::terminal::Terminal;

/// The loopback port. Fixed so a dev script need not discover it.
pub const PORT: u16 = 4573;

/// Start the bridge if the environment asks for one.
///
/// Never returns an error to the caller: a bridge that fails to bind must not stop a till from
/// opening. It logs and the app carries on without it.
pub fn spawn(terminal: Arc<Mutex<Terminal>>) {
    if std::env::var("SAHL_DEV_BRIDGE").as_deref() != Ok("1") {
        return;
    }

    let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("dev bridge not started: {error}");
            return;
        }
    };

    eprintln!(
        "dev bridge listening on http://127.0.0.1:{PORT} — DEBUG BUILDS ONLY, unauthenticated"
    );

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let terminal = Arc::clone(&terminal);
                    // One thread per request. A dev tool serving one browser does not need a pool,
                    // and a thread that panics on a malformed request takes nothing else with it.
                    std::thread::spawn(move || {
                        if let Err(error) = handle(stream, &terminal) {
                            eprintln!("dev bridge request failed: {error}");
                        }
                    });
                }
                Err(error) => eprintln!("dev bridge accept failed: {error}"),
            }
        }
    });
}

/// Read one request, dispatch it, write one response.
fn handle(mut stream: TcpStream, terminal: &Arc<Mutex<Terminal>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();

    let mut content_length = 0_usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let trimmed = header.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed
            .strip_prefix("Content-Length:")
            .or_else(|| trimmed.strip_prefix("content-length:"))
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        std::io::Read::read_exact(&mut reader, &mut body)?;
    }

    // The browser will preflight, because the page is on Vite's origin and this is another.
    if method == "OPTIONS" {
        return write_response(&mut stream, 204, "");
    }

    let payload = String::from_utf8_lossy(&body).into_owned();
    let (status, response) = dispatch(&path, &payload, terminal);
    write_response(&mut stream, status, &response)
}

/// Route a path to the same read-only views the Tauri commands expose.
///
/// Read-only on purpose. Mutating through here would mean maintaining a second copy of every
/// command's argument handling, and the two drifting is precisely the failure this whole design
/// avoids elsewhere — so the bridge shows state and the real app changes it.
fn dispatch(path: &str, _body: &str, terminal: &Arc<Mutex<Terminal>>) -> (u16, String) {
    let Ok(till) = terminal.lock() else {
        return (500, r#"{"error":"the till is poisoned"}"#.to_owned());
    };

    let json = match path.split('?').next().unwrap_or(path) {
        "/health" => serde_json::json!({ "ok": true, "identity": till.identity() }),

        "/outlet" => serde_json::to_value(till.outlet()).unwrap_or_default(),

        "/products" => serde_json::json!({
            "sellable": till.catalogue().sellable(),
            "all": till.catalogue().all(),
        }),

        "/floor" => serde_json::json!({
            "tables": till.floor().all(),
            "occupied": till.occupied_tables(),
            "capacity": till.floor().capacity(),
        }),

        "/staff" => serde_json::to_value(till.staff().active()).unwrap_or_default(),

        "/sales" => serde_json::json!({
            "open": till.book().open().map(sale_summary).collect::<Vec<_>>(),
            "unsynced": till.unsynced_count().unwrap_or_default(),
            "fiscalCounter": till.fiscal_tip().counter,
            "regime": till.regime(),
        }),

        "/stock" => serde_json::json!({
            "levels": till.stock().levels(),
            "variances": till.stock().variances(),
        }),

        _ => {
            return (
                404,
                r#"{"error":"try /health /outlet /products /floor /staff /sales /stock"}"#
                    .to_owned(),
            );
        }
    };

    (200, json.to_string())
}

/// Enough of a sale to check a screen against.
fn sale_summary(sale: &sahl_core::sale::Sale) -> serde_json::Value {
    serde_json::json!({
        "id": sale.id(),
        "status": format!("{:?}", sale.status()),
        "lines": sale.active_lines().count(),
        "totalMinor": sale.totals().ok().map(|totals| totals.total.minor()),
        "seating": sale.seating(),
    })
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    // Permissive CORS because the whole point is to be read from a page served by Vite on another
    // origin. Acceptable only under the three conditions in the module note.
    let response = format!(
        "HTTP/1.1 {status} OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Headers: content-type\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
