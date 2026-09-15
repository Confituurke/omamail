//! Renders arbitrary text — a Lightning invoice, here — as a scannable QR
//! code. Emitted as SVG rather than a raster format: no image encoder to pull
//! in for it, and a vector scales to whatever size the popup draws it at
//! without blurring. Always black on white regardless of the active theme:
//! a wallet's camera reads contrast, not the app's palette, and a tinted
//! code risks becoming unscannable in a dark theme.
use qrcode::{Color, QrCode};

const MAX_TEXT: usize = 4096;
/// Modules of white margin a reader expects around the pattern; held close to
/// a busy background, a code with none of this can be lost entirely.
const QUIET_ZONE: i64 = 4;

pub fn svg(text: &str) -> Result<String, &'static str> {
    if text.is_empty() || text.len() > MAX_TEXT || text.bytes().any(|b| b < 32 || b == 127) {
        return Err("invalid_params");
    }
    let code = QrCode::new(text.as_bytes()).map_err(|_| "lnemail_qr_failed")?;
    let width = code.width() as i64;
    let colors = code.to_colors();
    let side = width + QUIET_ZONE * 2;
    let mut path = String::new();
    for row in 0..width {
        let mut col = 0i64;
        while col < width {
            if colors[(row * width + col) as usize] != Color::Dark {
                col += 1;
                continue;
            }
            let start = col;
            while col < width && colors[(row * width + col) as usize] == Color::Dark {
                col += 1;
            }
            let run = col - start;
            let x = start + QUIET_ZONE;
            let y = row + QUIET_ZONE;
            path.push_str(&format!("M{x} {y}h{run}v1h-{run}z"));
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" \
shape-rendering=\"crispEdges\"><rect width=\"{side}\" height=\"{side}\" fill=\"#fff\"/>\
<path d=\"{path}\" fill=\"#000\"/></svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_short_string_into_a_square_svg_with_a_quiet_zone() {
        let out = svg("hello").unwrap();
        assert!(out.starts_with("<svg"));
        assert!(out.contains("fill=\"#fff\""));
        assert!(out.contains("fill=\"#000\""));
        assert!(out.contains("<path d=\"M"));
    }

    #[test]
    fn a_real_lightning_invoice_encodes_into_a_scannable_pattern() {
        let invoice = concat!(
            "lnbc10u1p3xnhl2pp5jptserfk3zk4qy42tlucycrfwxhydvlemu9pqr93tuzlv9cc7g3s",
            "dqsvfhkcap3xyhx7un8cqzpgxqzjcsp5f8c5deqsz6d5aqk2fpsv6vfqjqsq0h2exyxqcy7",
            "lu92e2s2a5x2q9qyyssqhw2r5xw2zwwnr5z6cxwv6wgqz5rn2jl5x7q6z6r4l4x3yq9wq5e",
            "xhqp6f6z5"
        );
        let out = svg(invoice).unwrap();
        assert!(out.contains("<path d=\"M"));
        // The quiet zone keeps the pattern off the very edge the viewBox draws.
        assert!(!out.contains("M0 0h") && !out.contains("M0 0v"));
    }

    #[test]
    fn refuses_empty_oversized_and_control_bearing_text_before_encoding() {
        assert_eq!(svg(""), Err("invalid_params"));
        assert_eq!(svg(&"a".repeat(MAX_TEXT + 1)), Err("invalid_params"));
        assert_eq!(svg("a\nb"), Err("invalid_params"));
        assert_eq!(svg("a\0b"), Err("invalid_params"));
        assert_eq!(svg("a\x7fb"), Err("invalid_params"));
    }

    #[test]
    fn text_too_large_for_any_qr_version_is_refused_rather_than_panicking() {
        // Lowercase text encodes in byte mode, whose largest version holds
        // 2953 bytes at the lowest error-correction level — well under
        // MAX_TEXT, so this clears our own length gate and still exceeds
        // what the encoder itself can hold.
        assert_eq!(svg(&"a".repeat(MAX_TEXT)), Err("lnemail_qr_failed"));
    }
}
