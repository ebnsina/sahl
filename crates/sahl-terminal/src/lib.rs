//! # sahl-terminal
//!
//! The till. A Tauri shell around the SvelteKit sell screen, with the event log, the hash chain and
//! every calculation living here in Rust rather than in the webview.
//!
//! That split is the point. The UI calls typed commands and renders what it is given; it has no
//! path to the database and no arithmetic of its own. "TypeScript never computes a total" is
//! therefore a property of the architecture rather than a rule someone has to remember.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
    )
)]

pub mod commands;
pub mod store;
pub mod terminal;

pub use terminal::{DeviceIdentity, Terminal, TerminalError};

use std::path::PathBuf;

use commands::TerminalState;
use store::EventStore;

/// Boot the till.
///
/// Fails loudly rather than starting degraded. A register that opens with a broken store would let
/// a cashier ring sales into nothing, which is worse than a register that plainly refuses to open.
///
/// # Panics
/// If the local store cannot be opened or its chain does not verify. There is no useful degraded
/// mode here — see [`terminal::TerminalError::CorruptLog`].
#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[expect(
    clippy::expect_used,
    reason = "this is the process entry point: if the window system or the local event log is \
              unusable there is no caller to return an error to, and a till that opens in a broken \
              state would let a cashier ring sales into nothing"
)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir: PathBuf = tauri::Manager::path(app)
                .app_data_dir()
                .map_err(|error| format!("no writable data directory: {error}"))?;
            std::fs::create_dir_all(&data_dir)?;

            let store = EventStore::open(&data_dir.join("till.db"))
                .map_err(|error| format!("cannot open the local event log: {error}"))?;

            // TODO(P1): read the enrolled identity from the keychain. Fixed for now so the sell
            // screen can be built and driven; enrollment lands with the sync client in P2.
            let identity = DeviceIdentity {
                tenant_id: uuid::Uuid::nil(),
                outlet_id: uuid::Uuid::nil(),
                device_id: uuid::Uuid::nil(),
            };

            let terminal = Terminal::load(store, identity)
                .map_err(|error| format!("the local event log is unusable: {error}"))?;

            tauri::Manager::manage(app, TerminalState::new(terminal));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_sale,
            commands::add_line,
            commands::change_quantity,
            commands::void_line,
            commands::discount_order,
            commands::record_tender,
            commands::complete_sale,
            commands::abandon_sale,
            commands::get_sale,
            commands::till_status,
        ])
        .run(tauri::generate_context!())
        .expect("the till could not start");
}
