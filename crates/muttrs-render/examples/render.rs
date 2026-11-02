//! Lays out an HTML or plain text file and prints its lines, to try the renderer out:
//! `cargo run -p muttrs-render --example render -- FILE [WIDTH]`.

use muttrs_render::{CellSize, Document, Options, Styles, TextFormat};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a file to render");
    let width = arguments
        .next()
        .map_or(80, |width| width.parse().expect("a width"));
    let body = std::fs::read_to_string(&path).expect("a readable file");
    let styles = Styles::default();
    let is_html = std::path::Path::new(&path)
        .extension()
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
        });
    let document = if is_html {
        Document::html(&body, &styles)
    } else {
        Document::plain(&body, TextFormat::Fixed, &styles)
    };
    let rendering = document.render(&Options {
        width,
        cell: CellSize::default(),
        pixel_ratio: 1.0,
        max_picture_rows: 20,
        pictures: &[],
    });
    for line in &rendering.lines {
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        println!("│{text}");
    }
    for (number, link) in document.links().iter().enumerate() {
        println!("[{}] {} — {}", number + 1, link.url, link.text);
    }
}
