//! Raw ESC/POS control sequences. Pure byte builders — nothing opens a device.
//!
//! Only near-universally supported sequences: a command one printer in five ignores is worse than
//! no command.

/// Escape.
pub const ESC: u8 = 0x1B;
/// Group separator — prefix for the newer GS command family.
pub const GS: u8 = 0x1D;

/// Reset alignment, emphasis, font and line spacing.
///
/// A job that died mid-print leaves the next receipt inheriting its state.
#[must_use]
pub fn initialize() -> Vec<u8> {
    vec![ESC, b'@']
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[must_use]
pub fn align(alignment: Align) -> Vec<u8> {
    let mode = match alignment {
        Align::Left => 0,
        Align::Center => 1,
        Align::Right => 2,
    };
    vec![ESC, b'a', mode]
}

/// Bold on or off.
#[must_use]
pub fn emphasis(on: bool) -> Vec<u8> {
    vec![ESC, b'E', u8::from(on)]
}

/// Double width and/or height, for the total line.
#[must_use]
pub fn size(double_width: bool, double_height: bool) -> Vec<u8> {
    let mut mode = 0u8;
    if double_width {
        mode |= 0b0010_0000;
    }
    if double_height {
        mode |= 0b0001_0000;
    }
    vec![GS, b'!', mode]
}

/// Feed `lines` blank lines.
#[must_use]
pub fn feed(lines: u8) -> Vec<u8> {
    vec![ESC, b'd', lines]
}

/// Partial cut, so the receipt stays attached until the customer takes it.
#[must_use]
pub fn cut() -> Vec<u8> {
    vec![GS, b'V', 66, 0x00]
}

/// Which drawer connector to pulse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerPin {
    /// Pin 2 — the common wiring.
    Two,
    /// Pin 5 — used when a second drawer shares the printer.
    Five,
}

/// Fire the drawer solenoid — it is wired to the printer, so this goes in the print stream.
///
/// Durations are 2ms units; 50/50ms is the compatible default for cheap solenoids.
#[must_use]
pub fn open_drawer(pin: DrawerPin) -> Vec<u8> {
    let connector = match pin {
        DrawerPin::Two => 0,
        DrawerPin::Five => 1,
    };
    vec![ESC, b'p', connector, 25, 25]
}

/// Select a character code table. There is no Bengali page in ESC/POS at all — see [`raster`].
#[must_use]
pub fn code_page(page: u8) -> Vec<u8> {
    vec![ESC, b't', page]
}

/// CP437, the original IBM page. Universally supported and the safe default for ASCII.
pub const CODE_PAGE_CP437: u8 = 0;
/// CP864, Arabic. Supported on many but not all printers — verify against the actual device.
pub const CODE_PAGE_CP864: u8 = 22;

/// Print a QR code — the ZATCA simplified invoice is not compliant without one.
///
/// Model 2, error correction M. The payload is base64, so every byte is ASCII and no code page
/// applies: `GS ( k` takes raw bytes and the printer's own encoder builds the symbol.
///
/// # Errors
/// [`QrError`] if the payload is empty or longer than the store command can describe.
pub fn qr(payload: &[u8], module_size: u8) -> Result<Vec<u8>, QrError> {
    if payload.is_empty() {
        return Err(QrError::Empty);
    }
    // The store command's length covers three header bytes on top of the payload, and the whole
    // thing is described by two bytes.
    let described =
        u16::try_from(payload.len().saturating_add(3)).map_err(|_| QrError::TooLong {
            length: payload.len(),
        })?;
    if payload.len() > 7_089 {
        return Err(QrError::TooLong {
            length: payload.len(),
        });
    }

    let size = module_size.clamp(1, 16);
    let mut out = Vec::with_capacity(payload.len().saturating_add(20));

    // GS ( k — select model 2.
    out.extend_from_slice(&[GS, b'(', b'k', 4, 0, 49, 65, 50, 0]);
    // Module size in dots. Too small and a phone camera cannot read it off thermal paper.
    out.extend_from_slice(&[GS, b'(', b'k', 3, 0, 49, 67, size]);
    // Error correction M — 15%, the usual choice for a receipt that will be creased.
    out.extend_from_slice(&[GS, b'(', b'k', 3, 0, 49, 69, 49]);
    // Store the payload.
    out.extend_from_slice(&[
        GS,
        b'(',
        b'k',
        (described & 0xFF) as u8,
        (described >> 8) as u8,
        49,
        80,
        48,
    ]);
    out.extend_from_slice(payload);
    // Print what was stored.
    out.extend_from_slice(&[GS, b'(', b'k', 3, 0, 49, 81, 48]);

    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QrError {
    #[error("a QR code needs a payload")]
    Empty,

    #[error("a QR payload is at most 7089 bytes, got {length}")]
    TooLong { length: usize },
}

/// Print a 1-bit bitmap — the only way scripts the code tables miss reach paper.
///
/// `width_px` must be a multiple of 8: eight pixels per byte, MSB leftmost, set bit = black.
///
/// # Errors
/// [`RasterError`] on misaligned width, wrong buffer length, or dimensions over 16 bits.
pub fn raster(width_px: u32, height_px: u32, bits: &[u8]) -> Result<Vec<u8>, RasterError> {
    if width_px == 0 || height_px == 0 {
        return Err(RasterError::Empty);
    }
    if !width_px.is_multiple_of(8) {
        return Err(RasterError::WidthNotByteAligned { width_px });
    }

    let bytes_per_row = width_px / 8;
    let expected = bytes_per_row
        .checked_mul(height_px)
        .ok_or(RasterError::TooLarge)?;
    let actual = u32::try_from(bits.len()).map_err(|_| RasterError::TooLarge)?;
    if expected != actual {
        return Err(RasterError::LengthMismatch { expected, actual });
    }

    let width_bytes = u16::try_from(bytes_per_row).map_err(|_| RasterError::TooLarge)?;
    let height = u16::try_from(height_px).map_err(|_| RasterError::TooLarge)?;

    // GS v 0 m xL xH yL yH — m = 0 selects normal density.
    let mut out = vec![
        GS,
        b'v',
        b'0',
        0,
        (width_bytes & 0xFF) as u8,
        (width_bytes >> 8) as u8,
        (height & 0xFF) as u8,
        (height >> 8) as u8,
    ];
    out.extend_from_slice(bits);
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RasterError {
    #[error("a raster image cannot have a zero dimension")]
    Empty,

    #[error("raster width {width_px} is not a multiple of 8")]
    WidthNotByteAligned { width_px: u32 },

    #[error("raster buffer is {actual} bytes but the dimensions need {expected}")]
    LengthMismatch { expected: u32, actual: u32 },

    #[error("raster image exceeds the protocol's 16-bit dimension fields")]
    TooLarge,
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_qr_selects_model_two_stores_the_payload_and_prints_it() {
        let bytes = qr(b"HELLO", 5).expect("encodes");

        // Model 2, module size, error correction, store, print — in that order.
        assert!(bytes.starts_with(&[GS, b'(', b'k', 4, 0, 49, 65, 50, 0]));
        assert!(bytes.ends_with(&[GS, b'(', b'k', 3, 0, 49, 81, 48]));
        assert!(
            bytes.windows(5).any(|w| w == b"HELLO"),
            "the payload reaches the printer"
        );
    }

    #[test]
    fn the_store_length_covers_the_payload_and_its_three_header_bytes() {
        // Off by one here and the printer either truncates the payload or waits forever for
        // bytes that never come — and the QR on the paper is silently wrong either way.
        let bytes = qr(b"HELLO", 5).expect("encodes");
        let index = bytes
            .windows(8)
            .position(|w| w[5] == 49 && w[6] == 80 && w[7] == 48 && w[0] == GS)
            .expect("store command");
        let described = u16::from(bytes[index + 3]) | (u16::from(bytes[index + 4]) << 8);
        assert_eq!(described, 5 + 3);
    }

    #[test]
    fn an_empty_payload_is_refused() {
        assert_eq!(qr(b"", 5), Err(QrError::Empty));
    }

    #[test]
    fn a_payload_longer_than_the_symbol_holds_is_refused() {
        let long = vec![b'A'; 7_090];
        assert!(matches!(qr(&long, 5), Err(QrError::TooLong { .. })));
    }

    #[test]
    fn a_module_size_outside_the_printers_range_is_clamped_rather_than_rejected() {
        // A refused receipt over a cosmetic setting would stop a sale.
        let small = qr(b"HELLO", 0).expect("encodes");
        let large = qr(b"HELLO", 99).expect("encodes");
        assert_eq!(small[16], 1);
        assert_eq!(large[16], 16);
    }

    use super::*;

    #[test]
    fn initialize_is_the_two_byte_reset() {
        assert_eq!(initialize(), vec![0x1B, b'@']);
    }

    #[test]
    fn alignment_maps_to_the_documented_modes() {
        assert_eq!(align(Align::Left), vec![0x1B, b'a', 0]);
        assert_eq!(align(Align::Center), vec![0x1B, b'a', 1]);
        assert_eq!(align(Align::Right), vec![0x1B, b'a', 2]);
    }

    #[test]
    fn size_flags_combine() {
        assert_eq!(size(false, false), vec![0x1D, b'!', 0x00]);
        assert_eq!(size(true, false), vec![0x1D, b'!', 0x20]);
        assert_eq!(size(false, true), vec![0x1D, b'!', 0x10]);
        assert_eq!(size(true, true), vec![0x1D, b'!', 0x30]);
    }

    #[test]
    fn the_drawer_pulse_targets_the_right_connector() {
        assert_eq!(open_drawer(DrawerPin::Two), vec![0x1B, b'p', 0, 25, 25]);
        assert_eq!(open_drawer(DrawerPin::Five), vec![0x1B, b'p', 1, 25, 25]);
    }

    #[test]
    fn the_cut_is_partial_so_the_receipt_stays_attached() {
        assert_eq!(cut(), vec![0x1D, b'V', 66, 0x00]);
    }

    #[test]
    fn a_raster_header_encodes_dimensions_little_endian() {
        // 16px wide is 2 bytes per row; 3 rows is 6 bytes of data.
        let bits = vec![0xFFu8; 6];
        let encoded = raster(16, 3, &bits).expect("valid raster");

        assert_eq!(&encoded[..8], &[0x1D, b'v', b'0', 0, 2, 0, 3, 0]);
        assert_eq!(&encoded[8..], &bits[..]);
    }

    #[test]
    fn a_wide_raster_uses_both_dimension_bytes() {
        // 2048px is 256 bytes per row, which does not fit in the low byte alone.
        let bits = vec![0u8; 256];
        let encoded = raster(2048, 1, &bits).expect("valid raster");
        assert_eq!(&encoded[4..6], &[0x00, 0x01], "256 little-endian");
    }

    #[test]
    fn a_width_that_is_not_byte_aligned_is_refused() {
        // Silently padding would shear the image — every row would drift sideways.
        assert_eq!(
            raster(17, 1, &[0u8; 3]),
            Err(RasterError::WidthNotByteAligned { width_px: 17 })
        );
    }

    #[test]
    fn a_buffer_of_the_wrong_size_is_refused() {
        assert_eq!(
            raster(16, 3, &[0u8; 4]),
            Err(RasterError::LengthMismatch {
                expected: 6,
                actual: 4
            })
        );
    }

    #[test]
    fn an_empty_raster_is_refused() {
        assert_eq!(raster(0, 5, &[]), Err(RasterError::Empty));
        assert_eq!(raster(8, 0, &[]), Err(RasterError::Empty));
    }
}
