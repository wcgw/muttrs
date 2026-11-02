//! Authors' colours, made to read on the theme's background.
//!
//! Mail is written for a white page. On a dark terminal, its dark grey text would vanish and its navy links sink, so
//! a colour is kept only when it reads on the page muttrs draws on, after moving its lightness as far as it takes.
//! Greys aren't kept at all: the theme's own text colour reads better than any shade of them.

/// A colour, in sRGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// The contrast ratio text needs with its background, as WCAG asks of body text.
const READABLE: f64 = 4.5;

/// How saturated, in HSL, a colour must be to be kept rather than drawn in the theme's text colour.
const GREY: f64 = 0.18;

impl Rgb {
    /// WCAG's relative luminance, from 0 (black) to 1 (white).
    fn luminance(self) -> f64 {
        let linear = |channel: u8| {
            let c = f64::from(channel) / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(self.0) + 0.7152 * linear(self.1) + 0.0722 * linear(self.2)
    }

    /// WCAG's contrast ratio between two colours, from 1 (the same) to 21 (black and white).
    fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn is_dark(self) -> bool {
        self.luminance() < 0.18
    }

    /// Hue (0 to 1), saturation and lightness.
    fn hsl(self) -> (f64, f64, f64) {
        let [r, g, b] = [self.0, self.1, self.2].map(|c| f64::from(c) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let lightness = f64::midpoint(max, min);
        if (max - min).abs() < f64::EPSILON {
            return (0.0, 0.0, lightness);
        }
        let delta = max - min;
        let saturation = if lightness > 0.5 {
            delta / (2.0 - max - min)
        } else {
            delta / (max + min)
        };
        let hue = if (max - r).abs() < f64::EPSILON {
            (g - b) / delta + if g < b { 6.0 } else { 0.0 }
        } else if (max - g).abs() < f64::EPSILON {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        (hue / 6.0, saturation, lightness)
    }

    fn from_hsl(hue: f64, saturation: f64, lightness: f64) -> Self {
        let channel = |offset: f64| {
            let q = if lightness < 0.5 {
                lightness * (1.0 + saturation)
            } else {
                lightness + saturation - lightness * saturation
            };
            let p = 2.0 * lightness - q;
            let mut t = hue + offset;
            if t < 0.0 {
                t += 1.0;
            }
            if t > 1.0 {
                t -= 1.0;
            }
            let value = if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            };
            // In range by the clamp.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = (value * 255.0).round().clamp(0.0, 255.0) as u8;
            byte
        };
        Self(channel(1.0 / 3.0), channel(0.0), channel(-1.0 / 3.0))
    }
}

/// `color` as text on `page`: as it is if it reads there, lighter or darker if that makes it read, or `None` to draw
/// the theme's text colour instead, which greys always are.
pub(crate) fn readable(color: Rgb, page: Rgb) -> Option<Rgb> {
    let (hue, saturation, lightness) = color.hsl();
    if saturation < GREY {
        return None;
    }
    if color.contrast(page) >= READABLE {
        return Some(color);
    }
    // Towards white on a dark page, black on a light one, until it reads.
    let toward: f64 = if page.is_dark() { 1.0 } else { 0.0 };
    (1..=50)
        .map(|step| {
            let lightness = lightness + (toward - lightness) * f64::from(step) / 50.0;
            Rgb::from_hsl(hue, saturation, lightness)
        })
        .find(|candidate| candidate.contrast(page) >= READABLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: Rgb = Rgb(0x22, 0x24, 0x36);
    const WHITE: Rgb = Rgb(255, 255, 255);

    #[test]
    fn greys_take_the_theme_text_color() {
        assert_eq!(readable(Rgb(0x33, 0x33, 0x33), DARK), None);
        assert_eq!(readable(Rgb(0, 0, 0), WHITE), None);
    }

    #[test]
    fn colors_that_read_stay() {
        let orange = Rgb(0xff, 0x96, 0x6c);
        assert_eq!(readable(orange, DARK), Some(orange));
    }

    #[test]
    fn dark_colors_lighten_on_a_dark_page_keeping_their_hue() {
        let navy_link = Rgb(0x1a, 0x0d, 0xab);
        let adapted = readable(navy_link, DARK).expect("a blue that reads");
        assert!(adapted.contrast(DARK) >= READABLE);
        let (hue, _, _) = adapted.hsl();
        assert!((hue - navy_link.hsl().0).abs() < 0.02, "still blue");
    }

    #[test]
    fn light_colors_darken_on_a_light_page() {
        let yellow = Rgb(0xff, 0xee, 0x58);
        let adapted = readable(yellow, WHITE).expect("a yellow that reads");
        assert!(adapted.contrast(WHITE) >= READABLE);
    }

    #[test]
    fn hsl_round_trips() {
        for color in [
            Rgb(0x1a, 0x73, 0xe8),
            Rgb(0xc3, 0xe8, 0x8d),
            Rgb(200, 10, 90),
        ] {
            let (h, s, l) = color.hsl();
            let back = Rgb::from_hsl(h, s, l);
            assert!(
                [
                    back.0.abs_diff(color.0),
                    back.1.abs_diff(color.1),
                    back.2.abs_diff(color.2)
                ]
                .iter()
                .all(|&d| d <= 1),
                "{color:?} came back as {back:?}"
            );
        }
    }
}
