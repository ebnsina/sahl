//! # sahl-escpos
//!
//! A completed sale as printer bytes. Pure, so the receipt layer is testable without hardware.
//!
//! ## Non-Latin scripts
//!
//! ESC/POS has no Bengali code page — Bangla cannot be sent as characters at all. Arabic has one
//! (CP864) but only on some printers.
//!
//! Urgency differs by market. Bangladesh: not blocking, since POS there is conventionally
//! English-default with Bangla as a switchable option, and only Bangla *product names* need it.
//! Saudi: ZATCA requires Arabic on a tax invoice, so it gates Gulf entry — confirm the exact
//! obligation when fiscalization lands.
//!
//! Either way the mechanism is [`command::raster`]: shape and rasterize on the host, send pixels.
//! The glyph pipeline itself is not built — Bangla conjuncts and Arabic joining need real shaping,
//! so the intended stack is `rustybuzz` plus `swash`.

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

pub mod command;
pub mod document;
pub mod receipt;

pub use command::{Align, DrawerPin, RasterError};
pub use document::{Document, ReceiptData, ReceiptLine, ReceiptTaxGroup};
pub use receipt::PaperWidth;
