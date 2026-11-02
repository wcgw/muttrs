//! Pictures in the terminal's cells: kitty Unicode placeholders where the terminal has them, half blocks elsewhere
//! (see `docs/adr/0002-pictures-as-cells.md`).
//!
//! Either way, a [`Picture`] is drawn as rows of ratatui [`Span`]s, so it scrolls and clips with the text around it.
//! Getting one takes two steps: [`prepare`] decodes and sizes the image and stages it for the terminal, which is slow
//! and belongs off the UI thread; [`Prepared::send`] then hands it to the terminal between two draws.

mod blocks;
mod kitty;

use std::io::{self, Cursor, Write};
use std::path::PathBuf;
use std::{env, fs, mem};

use image::imageops::{self, FilterType};
use image::{ImageReader, RgbaImage};
use ratatui_core::style::Style;
use ratatui_core::text::Span;

pub use crate::kitty::Ids;
use crate::kitty::ImageId;

/// How Pictures are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// kitty's Unicode placeholders: full resolution, in Ghostty and kitty.
    Kitty,
    /// Half blocks, two pixels a cell, in these colours: anywhere.
    HalfBlocks(Colors),
}

/// The colours a terminal draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colors {
    /// 24-bit colour.
    TrueColor,
    /// xterm's 256-colour palette.
    Indexed,
}

/// What muttrs knows of the terminal it runs in, from its environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terminal {
    /// How it draws Pictures best.
    pub protocol: Protocol,
    /// Whether it draws Nerd Font icons without the user installing a Nerd Font: Ghostty, kitty and WezTerm have
    /// the symbols built in.
    pub has_nerd_font_symbols: bool,
}

impl Terminal {
    /// The terminal this process runs in, from its environment variables.
    #[must_use]
    pub fn detect() -> Self {
        Self::from_env(|name| env::var(name).ok().filter(|value| !value.is_empty()))
    }

    fn from_env(var: impl Fn(&str) -> Option<String>) -> Self {
        let term = var("TERM").unwrap_or_default();
        let program = var("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();
        // Placeholders need the multiplexer to pass them through, which takes setting up.
        let multiplexed = var("TMUX").is_some()
            || var("ZELLIJ").is_some()
            || term.starts_with("screen")
            || term.starts_with("tmux");
        let kitty = var("KITTY_WINDOW_ID").is_some() || term == "xterm-kitty";
        let ghostty = program == "ghostty"
            || term == "xterm-ghostty"
            || var("GHOSTTY_RESOURCES_DIR").is_some();
        let wezterm = program == "wezterm" || var("WEZTERM_EXECUTABLE").is_some();
        let truecolor = matches!(
            var("COLORTERM").unwrap_or_default().as_str(),
            "truecolor" | "24bit"
        ) || kitty
            || ghostty
            || wezterm
            || matches!(program.as_str(), "iterm.app" | "vscode")
            || term.ends_with("-direct");
        let protocol = if (kitty || ghostty) && !multiplexed {
            Protocol::Kitty
        } else if truecolor {
            Protocol::HalfBlocks(Colors::TrueColor)
        } else {
            Protocol::HalfBlocks(Colors::Indexed)
        };
        Self {
            protocol,
            has_nerd_font_symbols: kitty || ghostty || wezterm,
        }
    }
}

/// The pixel size of one terminal cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSize {
    /// Width, in pixels.
    pub width: u32,
    /// Height, in pixels.
    pub height: u32,
}

/// Why an image can't be drawn.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// It isn't an image in a format muttrs reads (PNG, JPEG, GIF, WebP), or it's corrupt.
    #[error("couldn't decode the image: {0}")]
    Decode(#[from] image::ImageError),
    /// Reading it failed.
    #[error("couldn't read the image: {0}")]
    Read(#[from] io::Error),
}

/// The pixel size of the image in `bytes`, read from its header without decoding it.
///
/// # Errors
///
/// If it isn't an image muttrs reads.
pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32), Error> {
    Ok(ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()?)
}

/// A Picture decoded, sized and staged for the terminal, but not sent yet. Dropping it unsent deletes what it
/// staged.
#[derive(Debug)]
pub struct Prepared {
    commands: Vec<u8>,
    /// Files holding pixels, which the terminal deletes once it's read them.
    staged: Vec<PathBuf>,
    picture: Picture,
}

/// Decodes `bytes` (a GIF's first frame) and readies it to draw in `cols` × `rows` cells of `cell` pixels, as
/// `protocol` draws, taking a fresh id from `ids` for kitty.
///
/// Slow: it decodes, resizes and writes files. Call it off the UI thread.
///
/// # Errors
///
/// If `bytes` isn't an image muttrs reads.
pub fn prepare(
    bytes: &[u8],
    cols: u16,
    rows: u16,
    cell: CellSize,
    protocol: Protocol,
    ids: &Ids,
) -> Result<Prepared, Error> {
    let (cols, rows) = (
        cols.clamp(1, kitty::MAX_CELLS),
        rows.clamp(1, kitty::MAX_CELLS),
    );
    let pixels = image::load_from_memory(bytes)?.into_rgba8();
    match protocol {
        Protocol::Kitty => {
            let pixels = shrink(pixels, cols, rows, cell);
            let id = ids.next();
            let mut commands = Vec::new();
            let staged = kitty::transmit(&mut commands, id, &pixels)
                .into_iter()
                .collect();
            kitty::place(&mut commands, id, cols, rows);
            Ok(Prepared {
                commands,
                staged,
                picture: Picture {
                    cols,
                    rows,
                    kind: Kind::Kitty(id),
                },
            })
        }
        Protocol::HalfBlocks(colors) => Ok(Prepared {
            commands: Vec::new(),
            staged: Vec::new(),
            picture: Picture {
                cols,
                rows,
                kind: Kind::Blocks(blocks::rows(
                    &pixels,
                    cols,
                    rows,
                    (cell.width, cell.height),
                    colors,
                )),
            },
        }),
    }
}

/// `pixels` shrunk to what a box of `cols` × `rows` cells shows of them, keeping the aspect ratio; never enlarged,
/// as the terminal scales the rest of the way, and more pixels than the box shows only cost time and memory.
fn shrink(pixels: RgbaImage, cols: u16, rows: u16, cell: CellSize) -> RgbaImage {
    let (width, height) = pixels.dimensions();
    let scale = f64::min(
        f64::from(u32::from(cols) * cell.width) / f64::from(width),
        f64::from(u32::from(rows) * cell.height) / f64::from(height),
    );
    if scale >= 1.0 {
        return pixels;
    }
    // At least one pixel, and fewer than the original's.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let scaled = |pixels: u32| ((f64::from(pixels) * scale).round() as u32).max(1);
    imageops::resize(&pixels, scaled(width), scaled(height), FilterType::Triangle)
}

impl Prepared {
    /// Sends the Picture to the terminal, and returns it to draw. Write it between draws, so that it doesn't
    /// interleave with ratatui's output.
    ///
    /// # Errors
    ///
    /// If writing to `out` fails; the Picture is lost.
    pub fn send(mut self, out: &mut impl Write) -> io::Result<Picture> {
        out.write_all(&self.commands)?;
        // The terminal owns the staged files now: it deletes them once read.
        self.staged.clear();
        Ok(mem::replace(
            &mut self.picture,
            Picture {
                cols: 0,
                rows: 0,
                kind: Kind::Blocks(Vec::new()),
            },
        ))
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        for path in &self.staged {
            // Best effort: /dev/shm is cleared at reboot anyway.
            let _ = fs::remove_file(path);
        }
    }
}

/// A picture the terminal can draw, as rows of cells.
///
/// It isn't `Clone`: once [forgotten](Picture::forget), nothing should draw it.
#[derive(Debug, PartialEq, Eq)]
pub struct Picture {
    cols: u16,
    rows: u16,
    kind: Kind,
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Kitty(ImageId),
    Blocks(Vec<Vec<Span<'static>>>),
}

impl Picture {
    /// Its width, in cells.
    #[must_use]
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// Its height, in cells.
    #[must_use]
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Row `row` (from 0) of it, as spans `cols` cells wide.
    ///
    /// # Panics
    ///
    /// If `row` isn't below [`Picture::rows`].
    #[must_use]
    pub fn row(&self, row: u16) -> Vec<Span<'static>> {
        assert!(
            row < self.rows,
            "row {row} outside a {}-row picture",
            self.rows
        );
        match &self.kind {
            Kind::Kitty(id) => vec![Span::styled(
                kitty::placeholder_row(row, self.cols),
                Style::new().fg(id.color()),
            )],
            Kind::Blocks(rows) => rows[usize::from(row)].clone(),
        }
    }

    /// Deletes it from the terminal, freeing its memory; anything still drawing it shows blank. Write it between
    /// draws.
    ///
    /// # Errors
    ///
    /// If writing to `out` fails.
    pub fn forget(self, out: &mut impl Write) -> io::Result<()> {
        if let Kind::Kitty(id) = self.kind {
            let mut commands = Vec::new();
            kitty::delete(&mut commands, id);
            out.write_all(&commands)?;
        }
        Ok(())
    }
}

/// Deletes every Picture from the terminal: this run's, and any an earlier run left behind, as one that crashed
/// does. Call it on entering the alternate screen, before sending any Picture, and before leaving it.
///
/// # Errors
///
/// If writing to `out` fails.
pub fn forget_all(out: &mut impl Write) -> io::Result<()> {
    out.write_all(kitty::DELETE_ALL)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use image::{ImageFormat, Rgba};

    use super::*;

    fn terminal(vars: &[(&str, &str)]) -> Terminal {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Terminal::from_env(|name| vars.get(name).cloned())
    }

    #[test]
    fn terminals_by_their_environment() {
        let ghostty = terminal(&[("TERM", "xterm-ghostty"), ("TERM_PROGRAM", "ghostty")]);
        assert_eq!(ghostty.protocol, Protocol::Kitty);
        assert!(ghostty.has_nerd_font_symbols);
        assert_eq!(
            terminal(&[
                ("TERM", "xterm-kitty"),
                ("TMUX", "/tmp/tmux-1000/default,1,0")
            ])
            .protocol,
            Protocol::HalfBlocks(Colors::TrueColor),
            "placeholders don't pass through tmux by themselves"
        );
        assert_eq!(
            terminal(&[("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]).protocol,
            Protocol::HalfBlocks(Colors::TrueColor)
        );
        let plain = terminal(&[
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "Apple_Terminal"),
        ]);
        assert_eq!(plain.protocol, Protocol::HalfBlocks(Colors::Indexed));
        assert!(!plain.has_nerd_font_symbols);
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        RgbaImage::from_pixel(width, height, Rgba([200, 10, 10, 255]))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn dimensions_come_from_the_header() {
        assert_eq!(dimensions(&png(37, 12)).unwrap(), (37, 12));
        assert!(dimensions(b"not an image").is_err());
    }

    const CELL: CellSize = CellSize {
        width: 10,
        height: 20,
    };

    #[test]
    fn kitty_pictures_are_placeholders_naming_their_image() {
        let ids = Ids::default();
        let prepared = prepare(&png(100, 40), 10, 2, CELL, Protocol::Kitty, &ids).unwrap();
        let staged = prepared.staged.clone();
        let mut out = Vec::new();
        let picture = prepared.send(&mut out).unwrap();
        for path in staged {
            let _ = fs::remove_file(path);
        }
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("a=p,U=1,i=1,c=10,r=2")
        );
        let row = picture.row(1);
        assert_eq!(
            row[0]
                .content
                .chars()
                .filter(|&c| c == '\u{10EEEE}')
                .count(),
            10
        );
        let mut forgotten = Vec::new();
        picture.forget(&mut forgotten).unwrap();
        assert_eq!(forgotten, b"\x1b_Ga=d,d=I,i=1,q=2\x1b\\");
    }

    #[test]
    fn half_block_pictures_need_nothing_sent() {
        let prepared = prepare(
            &png(20, 20),
            2,
            1,
            CELL,
            Protocol::HalfBlocks(Colors::TrueColor),
            &Ids::default(),
        )
        .unwrap();
        let mut out = Vec::new();
        let picture = prepared.send(&mut out).unwrap();
        assert_eq!(out, b"");
        assert_eq!(picture.row(0)[0].content, "▀▀");
    }
}
