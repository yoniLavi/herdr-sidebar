//! Full-resolution image previews over the Kitty graphics protocol.
//!
//! The preview pane draws raster media as `▀` cells by default, which caps an
//! image at two pixels per cell. When the pane's terminal answers a Kitty
//! graphics query, the viewer instead leaves those cells blank and places the
//! real image over them.
//!
//! Inside herdr this has to be a **direct placement** (`a=T` at the cursor).
//! Measured against herdr 0.9.5 with `kitty_graphics = true`: a direct
//! placement is re-emitted to the outer terminal, while a Unicode-placeholder
//! (`U=1`) virtual placement is accepted by the pane and never forwarded, so
//! the image silently does not appear.

use std::io::{self, Write};
use std::sync::OnceLock;

use base64::Engine as _;
use image::ImageEncoder as _;
use ratatui::layout::Rect;

/// One image per viewer process, so one id. Ids are scoped to the pane's
/// own terminal (herdr remaps them on the way out), so no coordination is
/// needed between preview panes.
const IMAGE_ID: u32 = 0x4853;
/// Kitty requires a transmission split into payload chunks of at most 4096
/// base64 bytes.
const CHUNK: usize = 4096;

static ENABLED: OnceLock<bool> = OnceLock::new();

/// Whether previews use Kitty graphics. `false` until [`detect`] has run, so
/// tests and non-TTY use keep the portable renderer.
pub fn enabled() -> bool {
    ENABLED.get().copied().unwrap_or(false)
}

/// Decide once, before the TUI starts, whether this terminal shows Kitty
/// graphics. `HERDR_SIDEBAR_IMAGES=blocks` forces the portable renderer and
/// `=kitty` skips the query.
pub fn detect() {
    let on = match std::env::var("HERDR_SIDEBAR_IMAGES").ok().as_deref() {
        Some("blocks") => false,
        Some("kitty") => true,
        _ => query_terminal(),
    };
    let _ = ENABLED.set(on);
}

/// Ask the terminal directly: a 1×1 Kitty query followed by Primary Device
/// Attributes. Every terminal answers DA1, so its reply ends the wait; a
/// Kitty-capable one answers the query first.
#[cfg(unix)]
fn query_terminal() -> bool {
    use std::io::IsTerminal as _;
    use std::os::fd::AsRawFd as _;
    use std::time::{Duration, Instant};

    let stdin = io::stdin();
    if !stdin.is_terminal() || crossterm::terminal::enable_raw_mode().is_err() {
        return false;
    }
    let mut out = io::stdout();
    let _ = out.write_all(b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c");
    let _ = out.flush();
    let fd = stdin.as_raw_fd();
    let deadline = Instant::now() + Duration::from_millis(800);
    let mut reply = Vec::new();
    while !da1_complete(&reply) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, and a buffer we own for read(2).
        if unsafe { libc::poll(&mut poll, 1, left.as_millis() as libc::c_int) } <= 0 {
            break;
        }
        let mut buf = [0u8; 256];
        let got = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if got <= 0 {
            break;
        }
        reply.extend_from_slice(&buf[..got as usize]);
    }
    let _ = crossterm::terminal::disable_raw_mode();
    reply_supports_kitty(&reply)
}

#[cfg(not(unix))]
fn query_terminal() -> bool {
    false
}

fn da1_complete(reply: &[u8]) -> bool {
    reply
        .windows(3)
        .position(|w| w == b"\x1b[?")
        .is_some_and(|start| {
            reply[start + 3..]
                .iter()
                .find(|b| !(b.is_ascii_digit() || **b == b';'))
                == Some(&b'c')
        })
}

fn reply_supports_kitty(reply: &[u8]) -> bool {
    reply.windows(10).any(|w| w == b"\x1b_Gi=31;OK")
}

/// Pixel size of one cell, from the pane's window size. `None` when the
/// terminal does not report pixels (no attached client, or Windows); the
/// caller then falls back to the portable renderer.
pub fn cell_pixels() -> Option<(f64, f64)> {
    let size = crossterm::terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return None;
    }
    Some((
        f64::from(size.width) / f64::from(size.columns),
        f64::from(size.height) / f64::from(size.rows),
    ))
}

/// The cell rectangle an image of `source` pixels occupies when fitted into
/// `area`, aspect ratio preserved and centered.
pub fn fit(source: (u32, u32), area: Rect, cell: (f64, f64)) -> Option<Rect> {
    let (sw, sh) = (f64::from(source.0), f64::from(source.1));
    if area.width == 0 || area.height == 0 || sw == 0.0 || sh == 0.0 {
        return None;
    }
    let scale = (f64::from(area.width) * cell.0 / sw).min(f64::from(area.height) * cell.1 / sh);
    let cols = ((sw * scale / cell.0).round() as u16).clamp(1, area.width);
    let rows = ((sh * scale / cell.1).round() as u16).clamp(1, area.height);
    Some(Rect {
        x: area.x + (area.width - cols) / 2,
        y: area.y + (area.height - rows) / 2,
        width: cols,
        height: rows,
    })
}

/// What is currently placed on screen, so a redraw only re-sends the image
/// when the document or its rectangle changed.
#[derive(Default)]
pub struct Placement {
    shown: Option<(u64, Rect)>,
}

impl Placement {
    /// Bring the screen in line with `want`: `(document serial, pixels,
    /// cell rectangle, cell pixel size)`, or `None` for no image.
    pub fn sync(
        &mut self,
        out: &mut impl Write,
        want: Option<(u64, &image::RgbaImage, Rect, (f64, f64))>,
    ) -> io::Result<()> {
        let key = want.map(|(serial, _, at, _)| (serial, at));
        if key == self.shown {
            return Ok(());
        }
        if self.shown.take().is_some() {
            out.write_all(&delete_command())?;
        }
        if let Some((_, pixels, at, cell)) = want {
            out.write_all(&place_command(pixels, at, cell)?)?;
            self.shown = key;
        }
        out.flush()
    }

    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.sync(out, None)
    }
}

fn delete_command() -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\").into_bytes()
}

/// Cursor to `at`, transmit-and-display without moving the cursor, cursor
/// back. The image is sent no larger than the rectangle's pixel size: the
/// terminal scales it to `c`×`r` anyway, and a full-size screenshot would
/// otherwise cost megabytes per placement.
fn place_command(pixels: &image::RgbaImage, at: Rect, cell: (f64, f64)) -> io::Result<Vec<u8>> {
    let (sw, sh) = pixels.dimensions();
    let scale = (f64::from(at.width) * cell.0 / f64::from(sw))
        .min(f64::from(at.height) * cell.1 / f64::from(sh))
        .min(1.0);
    let resized;
    let image = if scale < 1.0 {
        let w = ((f64::from(sw) * scale).round() as u32).max(1);
        let h = ((f64::from(sh) * scale).round() as u32).max(1);
        resized = image::imageops::resize(pixels, w, h, image::imageops::FilterType::CatmullRom);
        &resized
    } else {
        pixels
    };
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(io::Error::other)?;
    let payload = base64::engine::general_purpose::STANDARD.encode(png);

    let mut command = format!("\x1b7\x1b[{};{}H", at.y + 1, at.x + 1).into_bytes();
    let chunks: Vec<&[u8]> = payload.as_bytes().chunks(CHUNK).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        let keys = if index == 0 {
            format!(
                "a=T,f=100,i={IMAGE_ID},c={},r={},C=1,q=2,m={more}",
                at.width, at.height
            )
        } else {
            format!("m={more}")
        };
        command.extend_from_slice(format!("\x1b_G{keys};").as_bytes());
        command.extend_from_slice(chunk);
        command.extend_from_slice(b"\x1b\\");
    }
    command.extend_from_slice(b"\x1b8");
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_reply_needs_the_ok_for_our_id_and_ends_at_da1() {
        assert!(!da1_complete(b"\x1b_Gi=31;OK\x1b\\"));
        assert!(da1_complete(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c"));
        assert!(reply_supports_kitty(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c"));
        assert!(!reply_supports_kitty(b"\x1b[?62;22c"));
        assert!(!reply_supports_kitty(
            b"\x1b_Gi=31;ENOTSUPPORTED\x1b\\\x1b[?1;2c"
        ));
    }

    #[test]
    fn fit_preserves_aspect_and_centers() {
        // 10x20 px cells: a square image in a 40x10 area is height-bound,
        // 200 px tall, so 20 columns wide, centered.
        let at = fit((100, 100), Rect::new(0, 1, 40, 10), (10.0, 20.0)).unwrap();
        assert_eq!(at, Rect::new(10, 1, 20, 10));
        // A wide image in the same area is width-bound.
        let at = fit((800, 100), Rect::new(0, 1, 40, 10), (10.0, 20.0)).unwrap();
        assert_eq!((at.width, at.height), (40, 3));
        assert!(fit((0, 10), Rect::new(0, 0, 4, 4), (10.0, 20.0)).is_none());
    }

    #[test]
    fn placement_resends_only_on_change_and_chunks_the_payload() {
        let pixels = image::RgbaImage::from_fn(300, 300, |x, y| {
            image::Rgba([
                (x * 7 % 256) as u8,
                (y * 13 % 256) as u8,
                (x ^ y) as u8,
                255,
            ])
        });
        let at = Rect::new(2, 3, 30, 15);
        let mut placement = Placement::default();
        let mut out = Vec::new();
        placement
            .sync(&mut out, Some((1, &pixels, at, (10.0, 20.0))))
            .unwrap();
        let first = String::from_utf8(out.clone()).unwrap();
        assert!(first.starts_with("\x1b7\x1b[4;3H\x1b_Ga=T,f=100,i=18515,c=30,r=15,C=1,q=2,m=1;"));
        assert!(first.ends_with("\x1b\\\x1b8"));
        assert!(first.contains("\x1b_Gm=0;"));

        out.clear();
        placement
            .sync(&mut out, Some((1, &pixels, at, (10.0, 20.0))))
            .unwrap();
        assert!(out.is_empty(), "an unchanged placement must not be re-sent");

        placement.clear(&mut out).unwrap();
        assert_eq!(out, delete_command());
    }
}
