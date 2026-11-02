//! What bodies look like laid out: the text of their lines, and the styles that matter.

use muttrs_render::{
    CellSize, Document, ImageSource, Link, Options, PictureSlot, Rendering, Rgb, Styles, TextFormat,
};
use ratatui_core::style::{Color, Modifier, Style};

fn render(document: &Document, width: u16, pictures: &[Option<(u32, u32)>]) -> Rendering {
    document.render(&Options {
        width,
        cell: CellSize {
            width: 10,
            height: 20,
        },
        max_picture_rows: 20,
        pictures,
    })
}

fn text(rendering: &Rendering) -> Vec<String> {
    rendering
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn html(html: &str, width: u16) -> Vec<String> {
    text(&render(
        &Document::html(html, &Styles::default()),
        width,
        &[],
    ))
}

/// The style of the first span saying `needle`.
fn style_of(rendering: &Rendering, needle: &str) -> Style {
    rendering
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content.contains(needle))
        .unwrap_or_else(|| panic!("no span says {needle:?}"))
        .style
}

#[test]
fn paragraphs_wrap_and_stand_apart() {
    assert_eq!(
        html("<p>Hello world this is a test</p>\n  <p>Second</p>", 12),
        ["Hello world", "this is a", "test", "", "Second"]
    );
}

#[test]
fn white_space_collapses_across_elements() {
    let document = Document::html(
        "<div>  Some   <b>bold</b>,\n\t<i> slanted </i> text </div>",
        &Styles::default(),
    );
    let rendering = render(&document, 80, &[]);
    assert_eq!(text(&rendering), ["Some bold, slanted text"]);
    assert!(
        style_of(&rendering, "bold")
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        style_of(&rendering, "slanted")
            .add_modifier
            .contains(Modifier::ITALIC)
    );
}

#[test]
fn a_word_split_across_elements_wraps_whole() {
    assert_eq!(html("<p>aaaa <b>bbbb</b>, x</p>", 9), ["aaaa", "bbbb, x"]);
}

#[test]
fn words_longer_than_the_line_are_cut() {
    assert_eq!(
        html("<p>see https://example.com/a/long/path</p>", 12),
        ["see", "https://exam", "ple.com/a/lo", "ng/path"]
    );
}

#[test]
fn line_breaks_make_blank_lines_as_browsers_do() {
    assert_eq!(
        html("<div>a<br><br>b<br></div><div><br></div><div>c</div>", 20),
        ["a", "", "b", "", "c"]
    );
}

#[test]
fn hidden_content_stays_hidden() {
    let lines = html(
        r#"<html><head><style>
             .preheader { display: none !important }
             @media (max-width: 600px) { .shown { display: none } }
           </style></head><body>
           <div class="preheader">Preview text&zwnj;&nbsp;&#847;&zwnj;</div>
           <div style="display:none">none</div>
           <div style="font-size:0">zero <span style="font-size:14px">sized again</span></div>
           <div style="mso-hide:all">outlook only</div>
           <div style="max-height:0;overflow:hidden">squeezed</div>
           <p class="shown">Shown</p>
         </body></html>"#,
        40,
    );
    assert_eq!(lines, ["sized again", "", "Shown"]);
}

#[test]
fn style_rules_apply_by_specificity_then_order() {
    let rendering = render(
        &Document::html(
            r"<style>p.loud { font-weight: bold } p { font-weight: normal } #x { font-style: italic }</style>
              <p class=loud id=x>Hi</p>",
            &Styles::default(),
        ),
        20,
        &[],
    );
    let style = style_of(&rendering, "Hi");
    assert!(style.add_modifier.contains(Modifier::BOLD));
    assert!(style.add_modifier.contains(Modifier::ITALIC));
}

#[test]
fn links_are_numbered_once_per_url() {
    let document = Document::html(
        r##"<a href="https://a.example">A</a> and <a href=" https://a.example ">again</a>
           <a href="https://b.example"><img src="https://b.example/i.png" alt="B's logo"></a>
           <a href="#top">top</a> <a href="javascript:x()">js</a>"##,
        &Styles::default(),
    );
    assert_eq!(
        text(&render(&document, 80, &[])),
        ["A[1] and again[1] [image: B's logo][2] top js"]
    );
    assert_eq!(
        document.links(),
        [
            Link {
                url: "https://a.example".into(),
                text: "A".into()
            },
            Link {
                url: "https://b.example".into(),
                text: "B's logo".into()
            },
        ]
    );
}

#[test]
fn lists_nest_and_number_aligned() {
    assert_eq!(
        html(
            "<ul><li>one<ul><li>nested item</li></ul></li><li>two</li></ul>\
             <ol start=9><li>nine</li><li>ten, which wraps</li></ol>",
            16
        ),
        [
            "• one",
            "  ◦ nested item",
            "• two",
            "",
            " 9. nine",
            "10. ten, which",
            "    wraps",
        ]
    );
}

#[test]
fn quotes_nest_with_a_bar_per_level() {
    assert_eq!(
        html(
            "<p>Me</p><div class=gmail_quote><div class=gmail_attr>On Monday, Ann wrote:<br></div>\
             <blockquote><p>You</p><blockquote>Deeper</blockquote></blockquote></div><p>After</p>",
            30
        ),
        [
            "Me",
            "",
            "On Monday, Ann wrote:",
            "",
            "▎ You",
            "▎ ",
            "▎ ▎ Deeper",
            "",
            "After"
        ]
    );
}

#[test]
fn data_tables_are_grids() {
    assert_eq!(
        html(
            "<table><thead><tr><th>Item</th><th>Qty</th></tr></thead>\
             <tr><td>Apple</td><td align=right>2</td></tr><tr><td>Kiwi</td><td>12</td></tr></table>",
            40
        ),
        ["Item  │ Qty", "──────┼────", "Apple │   2", "Kiwi  │ 12"]
    );
}

#[test]
fn narrow_data_tables_share_the_width() {
    let lines = html(
        "<table border=1><tr><td>short</td><td>a much longer cell of text</td></tr>\
         <tr><td>x</td><td>y</td></tr></table>",
        20,
    );
    assert_eq!(
        lines,
        [
            "short │ a much",
            "      │ longer cell",
            "      │ of text",
            "x     │ y"
        ]
    );
}

#[test]
fn layout_tables_are_linearised() {
    assert_eq!(
        html(
            r#"<table role="presentation" width="600"><tr><td align="center">
                 <table><tr><td>Left column</td><td>Right column</td></tr></table>
               </td></tr></table>"#,
            30
        ),
        ["         Left column", "         Right column"]
    );
}

#[test]
fn alignment() {
    assert_eq!(
        html(
            "<p align=center>Hi</p><p style='text-align:right'>Yo</p>",
            10
        ),
        ["    Hi", "", "        Yo"]
    );
}

#[test]
fn preformatted_text_keeps_its_spaces_and_is_cut() {
    assert_eq!(
        html("<pre>fn main() {\n    let x = 1;\n}</pre>", 12),
        ["fn main() {", "    let x = ", "1;", "}"]
    );
}

#[test]
fn rules_cross_the_width() {
    assert_eq!(html("<p>a</p><hr><p>b</p>", 5), ["a", "", "─────", "", "b"]);
}

#[test]
fn images_are_drawn_when_their_size_is_known() {
    let document = Document::html(
        r#"<p>Before <img src="cid:logo@x" alt="Logo"> after</p>
           <img src="https://track.example/p.gif" width="1" height="1">
           <img src="https://example.com/banner.png" width="600" alt="Banner">"#,
        &Styles::default(),
    );
    assert_eq!(
        document
            .images()
            .iter()
            .map(|image| &image.source)
            .collect::<Vec<_>>(),
        [
            &ImageSource::Cid("logo@x".into()),
            &ImageSource::Remote("https://example.com/banner.png".into())
        ],
        "no Tracking Pixel"
    );

    let unknown = render(&document, 40, &[]);
    assert_eq!(
        text(&unknown),
        ["Before [image: Logo] after", "", "[image: Banner]"]
    );

    // 200 × 100 px on 10 × 20 px cells: 20 × 5 cells. The banner is 1200 px wide, shown at the 600 px the HTML
    // says, which the 40 cells (400 px) shrink to 40 × 3 cells.
    let known = render(&document, 40, &[Some((200, 100)), Some((1200, 200))]);
    assert_eq!(known.lines.len(), 1 + 5 + 1 + 1 + 3);
    assert_eq!(text(&known)[0], "Before");
    assert_eq!(text(&known)[6], "after");
    assert_eq!(
        known.pictures,
        [
            PictureSlot {
                image: 0,
                line: 1,
                column: 0,
                cols: 20,
                rows: 5
            },
            PictureSlot {
                image: 1,
                line: 8,
                column: 0,
                cols: 40,
                rows: 3
            }
        ]
    );
}

#[test]
fn pictures_in_quotes_shift_right() {
    let document = Document::html(
        r#"<blockquote><img src="cid:a" alt="A"></blockquote>"#,
        &Styles::default(),
    );
    let rendering = render(&document, 40, &[Some((100, 40))]);
    assert_eq!(
        rendering.pictures,
        [PictureSlot {
            image: 0,
            line: 0,
            column: 2,
            cols: 10,
            rows: 2
        }]
    );
    assert_eq!(text(&rendering)[0], format!("▎ {}", " ".repeat(10)));
}

#[test]
fn authors_colours_read_on_the_page_or_go() {
    let styles = Styles {
        page: Some(Rgb(0x22, 0x24, 0x36)),
        text: Style::new().fg(Color::Rgb(0xc8, 0xd3, 0xf5)),
        ..Styles::default()
    };
    let rendering = render(
        &Document::html(
            r##"<span style="color:#333333">grey</span> <font color="#1a0dab">navy</font>
               <span style="color: rgb(255, 150, 108)">orange</span>"##,
            &styles,
        ),
        80,
        &[],
    );
    assert_eq!(style_of(&rendering, "grey").fg, styles.text.fg);
    assert_ne!(
        style_of(&rendering, "navy").fg,
        Some(Color::Rgb(0x1a, 0x0d, 0xab))
    );
    assert_eq!(
        style_of(&rendering, "orange").fg,
        Some(Color::Rgb(255, 150, 108))
    );

    let without_page = render(
        &Document::html(r#"<span style="color:red">red</span>"#, &Styles::default()),
        80,
        &[],
    );
    assert_eq!(style_of(&without_page, "red").fg, None);
}

#[test]
fn plain_text_quotes_signatures_and_links() {
    let styles = Styles {
        signature: Style::new().fg(Color::Gray),
        link: Style::new().fg(Color::Cyan),
        ..Styles::default()
    };
    let document = Document::plain(
        "Hi,\r\n\r\nsee https://example.com.\r\n\r\nOn Mon, Ann wrote:\r\n> Lunch?\r\n>\r\n> > Really\r\n\r\n-- \r\nBob\r\n",
        TextFormat::Fixed,
        &styles,
    );
    let rendering = render(&document, 40, &[]);
    assert_eq!(
        text(&rendering),
        [
            "Hi,",
            "",
            "see https://example.com.",
            "",
            "On Mon, Ann wrote:",
            "",
            "▎ Lunch?",
            "▎ ",
            "▎ ▎ Really",
            "",
            "--",
            "Bob"
        ]
    );
    assert_eq!(style_of(&rendering, "https://").fg, Some(Color::Cyan));
    assert_eq!(style_of(&rendering, "Bob").fg, Some(Color::Gray));
    assert_eq!(document.links()[0].url, "https://example.com");
}

#[test]
fn plain_text_keeps_its_spacing_and_wraps() {
    let document = Document::plain(
        "Name:   Ann\nRole:   a role far too long for the line",
        TextFormat::Fixed,
        &Styles::default(),
    );
    assert_eq!(
        text(&render(&document, 20, &[])),
        [
            "Name:   Ann",
            "Role:   a role far",
            "too long for the",
            "line"
        ]
    );
}

#[test]
fn flowed_text_reflows_to_the_width() {
    let document = Document::plain(
        "This paragraph was \r\nwrapped by its sender \r\nat a narrow width.\r\n",
        TextFormat::Flowed {
            delete_space: false,
        },
        &Styles::default(),
    );
    assert_eq!(
        text(&render(&document, 40, &[])),
        [
            "This paragraph was wrapped by its sender",
            "at a narrow width."
        ]
    );
}

#[test]
fn a_newsletter_reads_as_one_column() {
    let document = Document::html(include_str!("fixtures/newsletter.html"), &Styles::default());
    let lines = text(&render(&document, 72, &[]));
    let all = lines.join("\n");
    assert!(!all.contains("crab-shaped"), "the preheader is hidden");
    assert!(
        lines.contains(&"Hi there,".to_owned()),
        "wrapped paragraphs go left"
    );
    assert!(
        lines.contains(&"• ratatui[4]: terminal UIs".to_owned()),
        "list items stay by their markers"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.trim() == "Take the survey[3]" && line.starts_with("   ")),
        "a button stays centred"
    );
    assert!(
        lines
            .iter()
            .all(|line| line.is_empty() || !line.trim().is_empty())
    );
    assert_eq!(document.links().len(), 7);
    assert_eq!(
        document.images().len(),
        1,
        "the logo, not the Tracking Pixel"
    );
}
