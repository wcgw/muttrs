//! The CSS muttrs reads: declarations, from `style` attributes and `<style>` rules, and colours.
//!
//! Only what a terminal can show is used (see `docs/adr/0001-own-html-layout.md`), so the parsing is forgiving rather
//! than complete: anything it doesn't understand is skipped, never an error.

use crate::color::Rgb;

/// One `name: value` pair, its name in lower case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Declaration {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) important: bool,
}

/// The declarations of a `style` attribute, or of a rule's block, in order.
pub(crate) fn declarations(block: &str) -> Vec<Declaration> {
    split_outside_parens(block, ';')
        .filter_map(|declaration| {
            let (name, value) = declaration.split_once(':')?;
            let name = name.trim().to_ascii_lowercase();
            let mut value = value.trim();
            let important = match value.to_ascii_lowercase().rfind("!important") {
                Some(at) => {
                    value = value[..at].trim_end();
                    true
                }
                None => false,
            };
            (!name.is_empty() && !value.is_empty()).then(|| Declaration {
                name,
                value: value.to_owned(),
                important,
            })
        })
        .collect()
}

/// A style rule of a style sheet: one selector, as written, and its declarations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) selector: String,
    pub(crate) specificity: Specificity,
    pub(crate) declarations: Vec<Declaration>,
}

/// How specific a selector is: ids, then classes, attributes and pseudo-classes, then element names.
pub(crate) type Specificity = (u16, u16, u16);

/// The style rules of a `<style>` element, one per selector of a group, in order. At-rules (`@media`, `@font-face`,
/// `@import`) are skipped: what a mobile layout changes is no better for a terminal than the desktop layout.
pub(crate) fn stylesheet(css: &str) -> Vec<Rule> {
    let css = strip_comments(css);
    let mut rules = Vec::new();
    let mut rest = css.as_str();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if rest.starts_with('@') {
            // An at-rule ends at its `;`, or after its block, whichever comes first.
            match (rest.find(';'), rest.find('{')) {
                (Some(semicolon), Some(brace)) if semicolon < brace => {
                    rest = &rest[semicolon + 1..];
                }
                (_, Some(brace)) => rest = skip_block(&rest[brace..]),
                (Some(semicolon), None) => rest = &rest[semicolon + 1..],
                (None, None) => break,
            }
            continue;
        }
        let Some(brace) = rest.find('{') else { break };
        let selectors = &rest[..brace];
        let after = skip_block(&rest[brace..]);
        let block_end = rest.len() - after.len();
        let block = rest[brace + 1..block_end].trim_end_matches('}');
        let declarations = declarations(block);
        if !declarations.is_empty() {
            for selector in split_outside_parens(selectors, ',') {
                let selector = selector.trim();
                if !selector.is_empty() {
                    rules.push(Rule {
                        selector: selector.to_owned(),
                        specificity: specificity(selector),
                        declarations: declarations.clone(),
                    });
                }
            }
        }
        rest = after;
    }
    rules
}

/// `css` without its comments, and without the `<!--` and `-->` that mail often wraps style sheets in.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..]
            .find("*/")
            .map_or("", |end| &rest[start + 2 + end + 2..]);
    }
    out.push_str(rest);
    out.replace("<!--", " ").replace("-->", " ")
}

/// What follows the block `css` starts with (at its `{`), braces balanced.
fn skip_block(css: &str) -> &str {
    let mut depth = 0usize;
    for (at, character) in css.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &css[at + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

/// The parts of `text` between `separator`s that aren't within parentheses or quotes, as in `url(data:a;b)`.
fn split_outside_parens(text: &str, separator: char) -> impl Iterator<Item = &str> {
    let mut parts = Vec::new();
    let (mut depth, mut quote, mut start) = (0usize, None, 0);
    for (at, character) in text.char_indices() {
        match (character, quote) {
            ('"' | '\'', None) => quote = Some(character),
            (c, Some(q)) if c == q => quote = None,
            ('(', None) => depth += 1,
            (')', None) => depth = depth.saturating_sub(1),
            (c, None) if c == separator && depth == 0 => {
                parts.push(&text[start..at]);
                start = at + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts.into_iter()
}

/// The specificity of one selector, counted from its text: close enough to order the rules of mail's style sheets.
fn specificity(selector: &str) -> Specificity {
    let (mut ids, mut classes, mut elements) = (0u16, 0u16, 0u16);
    let mut chars = selector.chars().peekable();
    // Whether the next identifier starts a compound selector, where it's an element name.
    let mut at_compound_start = true;
    while let Some(character) = chars.next() {
        match character {
            '#' => {
                ids += 1;
                skip_identifier(&mut chars);
                at_compound_start = false;
            }
            '.' => {
                classes += 1;
                skip_identifier(&mut chars);
                at_compound_start = false;
            }
            '[' => {
                classes += 1;
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
                at_compound_start = false;
            }
            ':' => {
                if chars.peek() == Some(&':') {
                    chars.next();
                    elements += 1;
                } else {
                    classes += 1;
                }
                skip_identifier(&mut chars);
                at_compound_start = false;
            }
            ' ' | '>' | '+' | '~' => at_compound_start = true,
            '*' => at_compound_start = false,
            c if at_compound_start && (c.is_ascii_alphabetic() || c == '_' || c == '-') => {
                elements += 1;
                skip_identifier(&mut chars);
                at_compound_start = false;
            }
            _ => {}
        }
    }
    (ids, classes, elements)
}

fn skip_identifier(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while chars
        .peek()
        .is_some_and(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '\\'))
    {
        chars.next();
    }
    // A pseudo-class's argument, as in `:not(.a)`.
    if chars.peek() == Some(&'(') {
        let mut depth = 0;
        for c in chars.by_ref() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
}

/// A length in CSS pixels, or a percentage, as `width` and `height` take: `600`, `600px`, `100%`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    /// CSS pixels.
    Pixels(f32),
    /// A percentage of the containing block.
    Percent(f32),
}

impl Length {
    /// Reads a length from an attribute or a declaration; `None` for `auto` and anything else it can't use.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        if let Some(percent) = value.strip_suffix('%') {
            return percent.trim().parse().ok().map(Self::Percent);
        }
        let number = value
            .strip_suffix("px")
            .or_else(|| value.strip_suffix("pt").map(|_| ""))
            .unwrap_or(&value);
        if number.is_empty() {
            // Points: 1pt is 4/3 px.
            let points: f32 = value.strip_suffix("pt")?.trim().parse().ok()?;
            return Some(Self::Pixels(points * 4.0 / 3.0));
        }
        number
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map(Self::Pixels)
    }

    /// Whether this length is nothing at all: `0`, `0px`, `0%`.
    pub(crate) fn is_zero(self) -> bool {
        match self {
            Self::Pixels(n) | Self::Percent(n) => n <= 0.0,
        }
    }
}

/// Whether a `font-size` makes text invisible: `0`, `0px`, `0pt`, `0em`, `0rem`, `0%`.
pub(crate) fn is_zero_size(value: &str) -> bool {
    let number = value
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
    number.parse::<f32>().is_ok_and(|n| n <= 0.0)
}

/// The colour a `color` or `background-color` value, or the first colour of a `background` shorthand, names; `None`
/// for `transparent`, anything mostly transparent, and anything not a colour.
pub(crate) fn color(value: &str) -> Option<Rgb> {
    let value = value.trim();
    let paint = parse_color(value).or_else(|| {
        // A shorthand, as in `background: #fff url(…) no-repeat`: the first word that's a colour.
        split_outside_parens(value, ' ')
            .filter(|word| !word.is_empty())
            .find_map(parse_color)
    });
    match paint? {
        Paint::Color(rgb) => Some(rgb),
        Paint::Transparent => None,
    }
}

/// A value that's a colour.
enum Paint {
    Color(Rgb),
    /// `transparent`, or a colour too transparent to show.
    Transparent,
}

/// The colour `value` is, if it's one.
fn parse_color(value: &str) -> Option<Paint> {
    let lower = value.trim().to_ascii_lowercase();
    if lower == "transparent" {
        return Some(Paint::Transparent);
    }
    if let Some(hex) = lower.strip_prefix('#') {
        return hex_color(hex).map(Paint::Color);
    }
    if let Some(arguments) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return rgb_function(arguments);
    }
    named_color(&lower).map(Paint::Color)
}

fn hex_color(hex: &str) -> Option<Rgb> {
    let digit = |index: usize| u8::from_str_radix(hex.get(index..=index)?, 16).ok();
    let pair = |index: usize| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok();
    match hex.len() {
        3 | 4 => Some(Rgb(digit(0)? * 17, digit(1)? * 17, digit(2)? * 17)),
        6 | 8 => Some(Rgb(pair(0)?, pair(2)?, pair(4)?)),
        _ => None,
    }
}

/// The arguments of `rgb()` or `rgba()`, separated by commas or spaces, alpha after a comma or a `/`.
fn rgb_function(arguments: &str) -> Option<Paint> {
    let parts: Vec<&str> = arguments
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    let channel = |part: &str| -> Option<u8> {
        let value = if let Some(percent) = part.strip_suffix('%') {
            percent.parse::<f32>().ok()? * 2.55
        } else {
            part.parse::<f32>().ok()?
        };
        // In range by the clamp; rounding is what's wanted.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(value.round().clamp(0.0, 255.0) as u8)
    };
    let rgb = Rgb(channel(parts[0])?, channel(parts[1])?, channel(parts[2])?);
    let alpha = match parts.get(3) {
        Some(alpha) => match alpha.strip_suffix('%') {
            Some(percent) => percent.parse::<f32>().ok()? / 100.0,
            None => alpha.parse::<f32>().ok()?,
        },
        None => 1.0,
    };
    Some(if alpha >= 0.5 {
        Paint::Color(rgb)
    } else {
        Paint::Transparent
    })
}

/// CSS's named colours.
fn named_color(name: &str) -> Option<Rgb> {
    let at = NAMED_COLORS
        .binary_search_by(|(named, _)| named.cmp(&name))
        .ok()?;
    let [_, r, g, b] = NAMED_COLORS[at].1.to_be_bytes();
    Some(Rgb(r, g, b))
}

/// CSS Color Module Level 4's named colours, sorted by name for searching.
#[rustfmt::skip]
#[allow(clippy::unreadable_literal)]
const NAMED_COLORS: [(&str, u32); 148] = [
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
    ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
    ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700),
    ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000), ("greenyellow", 0xadff2f),
    ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585), ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5), ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6),
    ("olive", 0x808000), ("olivedrab", 0x6b8e23), ("orange", 0xffa500), ("orangered", 0xff4500),
    ("orchid", 0xda70d6), ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093), ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f),
    ("pink", 0xffc0cb), ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080),
    ("rebeccapurple", 0x663399), ("red", 0xff0000), ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513), ("salmon", 0xfa8072), ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee), ("sienna", 0xa0522d), ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd), ("slategray", 0x708090), ("slategrey", 0x708090), ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f), ("steelblue", 0x4682b4), ("tan", 0xd2b48c), ("teal", 0x008080),
    ("thistle", 0xd8bfd8), ("tomato", 0xff6347), ("turquoise", 0x40e0d0), ("violet", 0xee82ee),
    ("wheat", 0xf5deb3), ("white", 0xffffff), ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colors_are_sorted_for_searching() {
        assert!(NAMED_COLORS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(named_color("rebeccapurple"), Some(Rgb(0x66, 0x33, 0x99)));
    }

    #[test]
    fn declarations_split_outside_urls_and_take_importance() {
        assert_eq!(
            declarations(
                "Color: #FFF ; background:url(data:image/png;base64,AA) red;display:none !important;;x"
            ),
            [
                Declaration {
                    name: "color".into(),
                    value: "#FFF".into(),
                    important: false
                },
                Declaration {
                    name: "background".into(),
                    value: "url(data:image/png;base64,AA) red".into(),
                    important: false
                },
                Declaration {
                    name: "display".into(),
                    value: "none".into(),
                    important: true
                },
            ]
        );
    }

    #[test]
    fn colors_in_every_notation() {
        assert_eq!(color("#abc"), Some(Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(color("#A1B2C3"), Some(Rgb(0xa1, 0xb2, 0xc3)));
        assert_eq!(color("rgb(10, 20, 30)"), Some(Rgb(10, 20, 30)));
        assert_eq!(color("rgb(10 20 30 / 50%)"), Some(Rgb(10, 20, 30)));
        assert_eq!(color("rgba(10,20,30,0.1)"), None, "mostly transparent");
        assert_eq!(color("transparent"), None);
        assert_eq!(color("#fff url(x.png) no-repeat"), Some(Rgb(255, 255, 255)));
        assert_eq!(color("inherit"), None);
    }

    #[test]
    fn stylesheets_skip_comments_and_at_rules() {
        let rules = stylesheet(
            "<!-- /* a comment { } */ @import url(x.css); @media (max-width: 600px) { .a { display: none } }
             .hide, td.preheader { display: none } #main p:first-child { color: red } -->",
        );
        let summary: Vec<(&str, Specificity)> = rules
            .iter()
            .map(|rule| (rule.selector.as_str(), rule.specificity))
            .collect();
        assert_eq!(
            summary,
            [
                (".hide", (0, 1, 0)),
                ("td.preheader", (0, 1, 1)),
                ("#main p:first-child", (1, 1, 1))
            ]
        );
    }

    #[test]
    fn lengths() {
        assert_eq!(Length::parse("600"), Some(Length::Pixels(600.0)));
        assert_eq!(Length::parse(" 600px "), Some(Length::Pixels(600.0)));
        assert_eq!(Length::parse("100%"), Some(Length::Percent(100.0)));
        assert_eq!(Length::parse("12pt"), Some(Length::Pixels(16.0)));
        assert_eq!(Length::parse("auto"), None);
        assert!(is_zero_size("0px") && is_zero_size("0") && !is_zero_size("12px"));
    }
}
