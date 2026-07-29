//! Getting bytes to a printer.
//!
//! The rendering is `sahl-escpos`, which is pure and fully tested. This module is the part that
//! touches the world, and it is deliberately thin: choose a destination, write bytes, report
//! whether it worked.
//!
//! **None of this has been run against a physical printer.** The byte stream is verified against
//! the ESC/POS command set and can be dumped to a file for inspection, which is worth something and
//! is not the same thing. Real printers disagree about code pages, cut commands, drawer pulses and
//! how much they buffer, and the plan has always expected that to be where the schedule hurts.
//!
//! A failed print never fails a sale. The sale is already in the log and the money is already in
//! the drawer; paper is a courtesy to the customer and a legal artefact that can be reprinted. A
//! till that refused to complete because a printer was out of paper would be worse than useless at
//! a counter.

mod transport;

pub use transport::{PrintError, PrinterTarget, print};
