//! The Heatmap: the prose of an Assessment, as the Pager draws it, each window voight scored shaded by the share of
//! it an LLM wrote (see `CONTEXT.md`), as ghd shades its Previews.

use std::ops::Range;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::assessment::Window;

/// The Heatmap's styles, from voight's highlight groups: prose voight scored 0%, 50% and 100%.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Heat {
    pub(crate) human: Style,
    pub(crate) even: Style,
    pub(crate) llm: Style,
}

impl Heat {
    /// The style of prose voight scored `share`, between 0 and 1: `human`'s at 0, `even`'s at 0.5 and `llm`'s at 1.
    /// In between, the nearer one's, with a background blended from both's when they're RGB colours.
    pub(crate) fn style(&self, share: f64) -> Style {
        let share = share.clamp(0.0, 1.0);
        let (from, to, toward) = if share < 0.5 {
            (self.human, self.even, share * 2.0)
        } else {
            (self.even, self.llm, share * 2.0 - 1.0)
        };
        let nearer = if toward < 0.5 { from } else { to };
        let (Some(Color::Rgb(r0, g0, b0)), Some(Color::Rgb(r1, g1, b1))) = (from.bg, to.bg) else {
            return nearer;
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "rounded, between two `u8`s"
        )]
        let blend =
            |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * toward).round() as u8;
        nearer.bg(Color::Rgb(blend(r0, r1), blend(g0, g1), blend(b0, b1)))
    }
}

/// How far past the text found last the next is looked for: far enough for a line's worth of what the drawing left
/// out, as a quote between two paragraphs, not so far that a word found again much later is taken for it.
const LOOKAHEAD: usize = 512;

/// Shades `lines`, which draw `text`, by `windows` of it, in `heat`'s styles; what's outside them, like
/// what's quoted, stays as it is. Nothing is shaded if `windows` aren't of `text`.
///
/// What each line draws is found in `text` in order, span by span. What isn't found nearby, like a list's markers or
/// a quote's bar, is shaded as the text before it on its line, or else as the text after it.
pub(crate) fn shade(lines: &mut [Line<'static>], text: &str, windows: &[Window], heat: &Heat) {
    if windows.is_empty()
        || windows
            .iter()
            .any(|window| text.get(window.range.clone()).is_none())
    {
        return;
    }
    let mut finder = Finder::new(text);
    for line in lines {
        finder.next_line();
        let mut pieces = Vec::with_capacity(line.spans.len());
        for span in line.spans.drain(..) {
            match finder.find(&span.content) {
                Some(start) => cut(span, start, windows, &mut pieces),
                None => pieces.push((span, Place::Added)),
            }
        }
        place_added(&mut pieces);
        line.spans = pieces
            .into_iter()
            .map(|(span, place)| match place {
                Place::Found(Some(share)) => span.patch_style(heat.style(share)),
                Place::Found(None) | Place::Added => span,
            })
            .collect();
    }
}

/// Where a piece of a line is in the text it draws.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Place {
    /// Found: in a window an LLM wrote this share of, or outside any.
    Found(Option<f64>),
    /// Not found: something the drawing added.
    Added,
}

/// Pushes `span`, found at `start` in the text, onto `pieces`: cut where `windows` start or end in it, each piece placed
/// in its window, if any.
fn cut(
    span: Span<'static>,
    start: usize,
    windows: &[Window],
    pieces: &mut Vec<(Span<'static>, Place)>,
) {
    let end = start + span.content.len();
    let mut from = start;
    while from < end {
        let next = windows.partition_point(|window| window.range.end <= from);
        let (to, share) = match windows.get(next) {
            Some(window) if window.range.start <= from => {
                (window.range.end.min(end), Some(window.share))
            }
            Some(window) => (window.range.start.min(end), None),
            None => (end, None),
        };
        if from == start && to == end {
            pieces.push((span, Place::Found(share)));
            return;
        }
        // The windows are of the text, so they start and end between characters.
        let piece = span.content[from - start..to - start].to_owned();
        pieces.push((Span::styled(piece, span.style), Place::Found(share)));
        from = to;
    }
}

/// Places what the drawing added to a line as the piece found before it, or else after it; outside any window if
/// nothing on the line was found.
fn place_added(pieces: &mut [(Span<'static>, Place)]) {
    let mut before = None;
    for (_, place) in pieces.iter_mut() {
        match *place {
            Place::Found(share) => before = Some(share),
            Place::Added => {
                if let Some(share) = before {
                    *place = Place::Found(share);
                }
            }
        }
    }
    // Only what comes before anything found is left.
    let mut after = None;
    for (_, place) in pieces.iter_mut().rev() {
        match *place {
            Place::Found(share) => after = Some(share),
            Place::Added => *place = Place::Found(after.flatten()),
        }
    }
}

/// Finds the text lines draw in the text they draw, in order.
struct Finder<'t> {
    text: &'t str,
    /// Just past the text found last.
    at: usize,
    /// Where the text found last was looked for from. The next is looked for from there again if it isn't found past
    /// the last: what was found last may have been something the drawing added, found further on.
    looked_from: usize,
    /// The text found last on the line; empty on a new line.
    last: Range<usize>,
}

impl<'t> Finder<'t> {
    fn new(text: &'t str) -> Self {
        Self {
            text,
            at: 0,
            looked_from: 0,
            last: 0..0,
        }
    }

    /// Notes that what's drawn next is on a new line.
    fn next_line(&mut self) {
        self.last = 0..0;
    }

    /// Where `drawn`, drawn next, is in the text; `None` if it's taken for something the drawing added: what isn't
    /// found nearby, what has no letters or digits to tell it by, like a quote's bar, and what's the text found last
    /// on its line again.
    fn find(&mut self, drawn: &str) -> Option<usize> {
        if !drawn.chars().any(char::is_alphanumeric)
            || self.text.get(self.last.clone()) == Some(drawn)
        {
            return None;
        }
        let start = match self.find_from(self.at, drawn) {
            Some(start) => {
                self.looked_from = self.at;
                start
            }
            None => self.find_from(self.looked_from, drawn)?,
        };
        self.at = start + drawn.len();
        self.last = start..self.at;
        Some(start)
    }

    /// Where `drawn` is in the text, from `from` to not far past it.
    fn find_from(&self, from: usize, drawn: &str) -> Option<usize> {
        let end = self
            .text
            .floor_char_boundary(from.saturating_add(LOOKAHEAD + drawn.len()));
        self.text
            .get(from..end)?
            .find(drawn)
            .map(|offset| from + offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A heatmap that tells apart text an LLM wrote none, half and all of.
    fn theme() -> Heat {
        Heat {
            human: Style::new().bg(Color::Green),
            even: Style::new().bg(Color::Yellow),
            llm: Style::new().bg(Color::Red),
        }
    }

    /// The window over `part` of `text`, which an LLM wrote `share` of.
    fn window(text: &str, part: &str, share: f64) -> Window {
        let start = text.find(part).expect("part of the text");
        Window {
            range: start..start + part.len(),
            share,
        }
    }

    /// Each line's pieces, with the background each is shaded with.
    fn shades(lines: &[Line<'_>]) -> Vec<Vec<(String, Option<Color>)>> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| (span.content.to_string(), span.style.bg))
                    .collect()
            })
            .collect()
    }

    fn piece(text: &str, bg: Option<Color>) -> (String, Option<Color>) {
        (text.to_owned(), bg)
    }

    #[test]
    fn windows_are_shaded_and_cut_where_they_meet() {
        let text = "One sentence. Another one.\n\nA last paragraph.";
        let windows = [
            window(text, "One sentence.", 0.0),
            window(text, "Another one.", 1.0),
            window(text, "A last paragraph.", 0.5),
        ];
        let mut lines = vec![
            Line::from("One sentence. Another one."),
            Line::default(),
            Line::from("A last paragraph."),
        ];
        shade(&mut lines, text, &windows, &theme());
        assert_eq!(
            shades(&lines),
            [
                vec![
                    piece("One sentence.", Some(Color::Green)),
                    piece(" ", None),
                    piece("Another one.", Some(Color::Red)),
                ],
                vec![],
                vec![piece("A last paragraph.", Some(Color::Yellow))],
            ]
        );
    }

    #[test]
    fn wrapped_lines_are_found_and_quotes_left_as_they_are() {
        // The Pager's lines: a paragraph wrapped, a quote the prose leaves out, then another paragraph.
        let text = "Thursday works for me, see you there.\n\nAnd bring the report.";
        let windows = [Window {
            range: 0..text.len(),
            share: 1.0,
        }];
        let mut lines = vec![
            Line::from("Thursday works for me,"),
            Line::from("see you there."),
            Line::from(vec![Span::raw("▎ "), Span::raw("Lunch on Thursday?")]),
            Line::from("And bring the report."),
        ];
        shade(&mut lines, text, &windows, &theme());
        let shades = shades(&lines);
        assert_eq!(
            shades[0],
            [piece("Thursday works for me,", Some(Color::Red))]
        );
        assert_eq!(shades[1], [piece("see you there.", Some(Color::Red))]);
        assert!(
            shades[2].iter().all(|(_, bg)| bg.is_none()),
            "the quote: {:?}",
            shades[2]
        );
        assert_eq!(
            shades[3],
            [piece("And bring the report.", Some(Color::Red))]
        );
    }

    #[test]
    fn shading_keeps_the_text_style_but_its_colors() {
        let text = "Bold words.";
        let mut lines = vec![Line::from(Span::styled(text, Style::new().blue().bold()))];
        let theme = Heat {
            llm: Style::new().black().on_red(),
            ..theme()
        };
        shade(&mut lines, text, &[window(text, text, 1.0)], &theme);
        assert_eq!(
            lines[0].spans[0].style,
            Style::new().black().on_red().bold()
        );
    }

    #[test]
    fn windows_not_of_the_text_shade_nothing() {
        let mut lines = vec![Line::from("Short.")];
        let windows = [Window {
            range: 0..100,
            share: 1.0,
        }];
        shade(&mut lines, "Short.", &windows, &theme());
        assert_eq!(shades(&lines), [vec![piece("Short.", None)]]);
    }

    #[test]
    fn heat_blends_rgb_backgrounds_and_steps_between_others() {
        let blended = Heat {
            human: Style::new().bg(Color::Rgb(0, 0xff, 0)),
            even: Style::new().bg(Color::Rgb(0xff, 0xff, 0)),
            llm: Style::new().bg(Color::Rgb(0xff, 0, 0)),
        };
        assert_eq!(blended.style(0.0), blended.human);
        assert_eq!(blended.style(0.5), blended.even);
        assert_eq!(blended.style(1.0), blended.llm);
        assert_eq!(blended.style(-1.0), blended.human, "clamped");
        assert_eq!(
            blended.style(0.25),
            Style::new().bg(Color::Rgb(0x80, 0xff, 0))
        );
        let stepped = theme();
        assert_eq!(stepped.style(0.24), stepped.human);
        assert_eq!(stepped.style(0.26), stepped.even);
        assert_eq!(stepped.style(0.76), stepped.llm);
    }
}
