//! QR codes for the join URL, generated locally.
//!
//! PRD section 23 requires the QR to be generated on the Host machine. Nothing
//! is fetched, no image service is called, and the application works with no
//! internet at all (PRD section 4).
//!
//! # A matrix, not an image
//!
//! This returns the raw module grid - one boolean per square - and the Host UI
//! draws it as SVG rectangles. Three reasons, in order:
//!
//! 1. **No markup injection.** Returning an SVG string would mean the UI
//!    injecting it with `dangerouslySetInnerHTML`, which is a habit worth not
//!    starting in an application whose whole premise is that content is
//!    untrusted (architecture rules section 13.1).
//! 2. **No new CSP allowance.** A `data:` image URI would work under the current
//!    policy, but a grid of `<rect>` elements needs nothing at all.
//! 3. **The smallest dependency.** Only the encoder is used, so `qrcode` is
//!    taken with `default-features = false` and its `image` dependency never
//!    enters the build (ADR-0017).

use qrcode::{EcLevel, QrCode};
use serde::Serialize;

/// A QR code as a square grid of modules.
///
/// `modules` is row-major and exactly `size * size` long. `true` is a dark
/// square.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QrMatrix {
    pub size: u32,
    pub modules: Vec<bool>,
}

/// Encode `text` as a QR code.
///
/// Error correction level **M** (about 15% recoverable): a join URL is short, so
/// the extra redundancy costs only a slightly denser grid, and it is what makes
/// the code still scan from a projector or a phone camera at an angle.
pub fn encode(text: &str) -> Result<QrMatrix, QrError> {
    let code = QrCode::with_error_correction_level(text, EcLevel::M).map_err(|error| QrError {
        detail: error.to_string(),
    })?;

    let colors = code.to_colors();
    let size = code.width();

    // `to_colors` is row-major and square by construction; asserted rather than
    // assumed, because the UI indexes into this and a silent mismatch would be
    // an unreadable code rather than an error.
    debug_assert_eq!(colors.len(), size * size);

    Ok(QrMatrix {
        size: u32::try_from(size).map_err(|_| QrError {
            detail: format!("a QR code of {size} modules is too large to describe"),
        })?,
        modules: colors
            .into_iter()
            .map(|color| color == qrcode::Color::Dark)
            .collect(),
    })
}

/// The text could not be encoded as a QR code.
#[derive(Debug, thiserror::Error)]
#[error("could not generate a QR code: {detail}")]
pub struct QrError {
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "http://192.168.1.42:8765/join/\
                       3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b";

    #[test]
    fn a_join_url_encodes_to_a_square_grid() {
        let matrix = encode(URL).expect("encodes");

        // Square, and the length matches the declared size - the UI relies on
        // both to index the grid.
        assert_eq!(
            matrix.modules.len(),
            (matrix.size * matrix.size) as usize,
            "the grid must be exactly size * size"
        );
        // Every QR version is at least 21 modules across.
        assert!(matrix.size >= 21, "{}", matrix.size);
        assert_eq!(matrix.size % 4, 1, "a QR side is always 4n + 1 modules");
    }

    #[test]
    fn the_finder_patterns_are_where_a_scanner_looks_for_them() {
        // A cheap structural check that this is a real QR code rather than an
        // arbitrary grid: all three corners carry a 7x7 finder pattern whose
        // outer ring is dark and whose next ring is light.
        let matrix = encode(URL).expect("encodes");
        let at = |row: u32, column: u32| matrix.modules[(row * matrix.size + column) as usize];
        let last = matrix.size - 1;

        for (row, column) in [(0, 0), (0, last - 6), (last - 6, 0)] {
            assert!(at(row, column), "finder corner at {row},{column}");
            assert!(at(row, column + 6), "finder ring at {row},{column}");
            assert!(at(row + 6, column), "finder ring at {row},{column}");
            assert!(!at(row + 1, column + 1), "finder gap at {row},{column}");
        }
    }

    #[test]
    fn encoding_is_deterministic() {
        // The same URL must produce the same code: a QR that changed between
        // renders would flicker on screen and could not be photographed.
        assert_eq!(encode(URL).unwrap(), encode(URL).unwrap());
    }

    #[test]
    fn different_urls_produce_different_codes() {
        let one = encode("http://192.168.1.42:8765/join/aaaa").unwrap();
        let two = encode("http://192.168.1.42:8765/join/bbbb").unwrap();
        assert_ne!(one.modules, two.modules);
    }

    #[test]
    fn a_longer_url_needs_a_larger_grid() {
        // Confirms the payload is really being encoded rather than a fixed
        // placeholder being returned.
        let short = encode("http://10.0.0.1:8765/join/a").unwrap();
        let long = encode(URL).unwrap();
        assert!(long.size > short.size, "{} vs {}", long.size, short.size);
    }
}
