//! Pictures as half blocks: `▀` with one pixel in the foreground and the one below in the background, which any
//! terminal draws, at two pixels a cell.

use image::RgbaImage;
use image::imageops::{self, FilterType};
use ratatui_core::style::{Color, Style};
use ratatui_core::text::Span;

use crate::Colors;

/// Below this alpha, a pixel is transparent: the terminal's background shows.
const OPAQUE: u8 = 128;

/// The rows of spans that draw `pixels` in a box of `cols` × `rows` cells, shrunk or grown to fit keeping its aspect
/// ratio on cells of `cell` pixels, and centred.
pub(crate) fn rows(
    pixels: &RgbaImage,
    cols: u16,
    rows: u16,
    cell: (u32, u32),
    colors: Colors,
) -> Vec<Vec<Span<'static>>> {
    let (width, height) = pixels.dimensions();
    let (cell_width, cell_height) = (f64::from(cell.0.max(1)), f64::from(cell.1.max(2)));
    // How big the picture shows, in pixels on screen, then in half-block pixels: a cell is one wide and two high.
    let scale = f64::min(
        f64::from(cols) * cell_width / f64::from(width),
        f64::from(rows) * cell_height / f64::from(height),
    );
    let fit = |pixels: u32, per: f64, max: u32| {
        // In range by the clamp.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let fitted = (f64::from(pixels) * scale / per)
            .round()
            .clamp(1.0, f64::from(max)) as u32;
        fitted
    };
    let across = fit(width, cell_width, u32::from(cols));
    let down = fit(height, cell_height / 2.0, u32::from(rows) * 2);
    let resized = imageops::resize(pixels, across, down, FilterType::Triangle);
    let (left, top) = (
        (u32::from(cols) - across) / 2,
        (u32::from(rows) * 2 - down) / 2,
    );
    let pixel = |col: u32, half_row: u32| -> Option<[u8; 3]> {
        let (col, half_row) = (col.checked_sub(left)?, half_row.checked_sub(top)?);
        if col >= across || half_row >= down {
            return None;
        }
        let [red, green, blue, alpha] = resized.get_pixel(col, half_row).0;
        (alpha >= OPAQUE).then_some([red, green, blue])
    };
    (0..u32::from(rows))
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for col in 0..u32::from(cols) {
                let (upper, lower) = (pixel(col, row * 2), pixel(col, row * 2 + 1));
                let (symbol, style) = match (upper, lower) {
                    (Some(upper), Some(lower)) => (
                        "▀",
                        Style::new()
                            .fg(color(upper, colors))
                            .bg(color(lower, colors)),
                    ),
                    (Some(upper), None) => ("▀", Style::new().fg(color(upper, colors))),
                    (None, Some(lower)) => ("▄", Style::new().fg(color(lower, colors))),
                    (None, None) => (" ", Style::new()),
                };
                match spans.last_mut() {
                    Some(last) if last.style == style => last.content.to_mut().push_str(symbol),
                    _ => spans.push(Span::styled(symbol, style)),
                }
            }
            spans
        })
        .collect()
}

fn color([r, g, b]: [u8; 3], colors: Colors) -> Color {
    match colors {
        Colors::TrueColor => Color::Rgb(r, g, b),
        Colors::Indexed => Color::Indexed(nearest_indexed(r, g, b)),
    }
}

/// The xterm 256-colour palette's nearest colour: in its 6 × 6 × 6 cube, or on its grey ramp.
fn nearest_indexed(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |c: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, level)| level.abs_diff(c))
            .map_or(0, |(index, _)| index)
    };
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let distance = |(r2, g2, b2): (u8, u8, u8)| {
        let d = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
        d(r, r2) + d(g, g2) + d(b, b2)
    };
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    // The ramp's 24 greys: 8, 18, …, 238.
    let grey_index = (average.saturating_sub(3) / 10).min(23);
    // Below 24 by the min.
    #[allow(clippy::cast_possible_truncation)]
    let grey_level = (8 + grey_index * 10) as u8;
    #[allow(clippy::cast_possible_truncation)]
    let cube_index = (16 + 36 * ri + 6 * gi + bi) as u8;
    if distance((grey_level, grey_level, grey_level)) < distance(cube) {
        // Below 24 by the min.
        #[allow(clippy::cast_possible_truncation)]
        let grey = 232 + grey_index as u8;
        grey
    } else {
        cube_index
    }
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

    fn symbols(rows: &[Vec<Span<'static>>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn two_pixels_a_cell_upper_in_the_foreground() {
        // Red on top, blue below: one cell high, square cells of 10 × 20 px fit 2 × 2 pixels per 2 cells.
        let pixels = RgbaImage::from_fn(2, 2, |_, y| if y == 0 { RED } else { BLUE });
        let drawn = rows(&pixels, 2, 1, (10, 20), Colors::TrueColor);
        assert_eq!(symbols(&drawn), ["▀▀"]);
        assert_eq!(drawn[0][0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(drawn[0][0].style.bg, Some(Color::Rgb(0, 0, 255)));
    }

    #[test]
    fn transparency_shows_the_terminal() {
        let pixels = RgbaImage::from_fn(1, 2, |_, y| if y == 0 { Rgba([0, 0, 0, 0]) } else { RED });
        let drawn = rows(&pixels, 1, 1, (10, 20), Colors::TrueColor);
        assert_eq!(symbols(&drawn), ["▄"]);
        assert_eq!(drawn[0][0].style.bg, None);
    }

    #[test]
    fn a_wide_picture_is_centred_in_a_tall_box() {
        // 4 × 1 px in a 4 × 2 box: 4 cells wide, so 1 cell (2 half-block pixels) high, centred down.
        let pixels = RgbaImage::from_pixel(4, 1, RED);
        let drawn = rows(&pixels, 4, 3, (10, 20), Colors::TrueColor);
        assert_eq!(symbols(&drawn), ["    ", "▀▀▀▀", "    "]);
    }

    #[test]
    fn indexed_colours_are_the_palettes_nearest() {
        assert_eq!(nearest_indexed(255, 0, 0), 196);
        assert_eq!(nearest_indexed(0, 0, 0), 16);
        assert_eq!(nearest_indexed(128, 128, 128), 244);
    }
}
