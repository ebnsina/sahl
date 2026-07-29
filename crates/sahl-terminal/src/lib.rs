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
pub mod enrollment;
pub mod store;
pub mod sync;
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

            // An un-enrolled till still opens — the sell screen shows an enrollment prompt rather
            // than a crash — but it gets no identity and no sync until a token is redeemed.
            let credentials = enrollment::load(&data_dir)
                .map_err(|error| format!("device credentials are unreadable: {error}"))?;

            let Some(credentials) = credentials else {
                eprintln!("this till is not enrolled; redeem an enrollment token to start trading");
                return Ok(());
            };

            let identity = credentials.identity;
            let store = EventStore::open(&data_dir.join("till.db"), identity.device_id)
                .map_err(|error| format!("cannot open the local event log: {error}"))?;

            let terminal = Terminal::load(store, identity)
                .map_err(|error| format!("the local event log is unusable: {error}"))?;

            let shared = std::sync::Arc::new(std::sync::Mutex::new(terminal));

            // Sync runs only when a server is configured. A shop with no SAHL_SERVER_URL is a
            // perfectly valid single-till deployment that simply never syncs.
            if let Ok(base_url) = std::env::var("SAHL_SERVER_URL") {
                match credentials.signing_key() {
                    Ok(key) => match sync::HttpTransport::new(base_url, identity.device_id, key) {
                        Ok(transport) => {
                            // Seed the jitter from the device id so a shop's tills do not retry in
                            // lockstep after an area outage. Truncating to 64 bits is fine — this
                            // only needs to differ between devices, not to be unguessable.
                            let (seed, _) = identity.device_id.as_u64_pair();
                            let handle =
                                sync::spawn(std::sync::Arc::clone(&shared), transport, seed);
                            tauri::Manager::manage(app, handle);
                        }
                        Err(error) => eprintln!("sync disabled: {error}"),
                    },
                    Err(error) => eprintln!("sync disabled: {error}"),
                }
            }

            tauri::Manager::manage(app, TerminalState::from_shared(shared));
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
            commands::sync_status,
            commands::open_shift,
            commands::move_cash,
            commands::count_drawer,
            commands::shift_report,
            commands::blind_count_sheet,
            commands::close_shift,
            commands::receive_stock,
            commands::count_stock,
            commands::issue_stock,
            commands::stock_position,
            commands::blind_stock_sheet,
            commands::staff_list,
            commands::sign_in,
            commands::enrol_staff,
            commands::audit_feed,
        ])
        .run(tauri::generate_context!())
        .expect("the till could not start");
}
