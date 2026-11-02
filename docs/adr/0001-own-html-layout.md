---
status: accepted
---

# Lay HTML out ourselves, over html5ever, for the terminal

muttrs parses a Message's HTML with html5ever (through `scraper`) and lays it out itself, into the same styled lines
ratatui draws everything else with. It applies the CSS that matters on a terminal, from `style` attributes and simple
`<style>` rules: colour, weight, slant, decoration, alignment and, above all, `display: none`. It numbers Links as w3m
does, and leaves slots of the right size for images, which the Pager fills with Pictures.

A terminal isn't a browser, and most HTML mail is a newsletter laid out with nested tables for a 600 pixel column.
Drawing those tables as grids, as a browser would, gives narrow columns of wrapped words. muttrs draws a table as a
grid only when it holds data (header cells, borders, short cells), and otherwise linearises it: its cells one after
the other, as the mobile layout of the same newsletter does.

## Considered options

- **html2text** (0.17): good text and grid tables, with annotations for styles and links. But images are only their
  alt text in the flow, with no way to reserve rows for a Picture, and every table is a grid.
- **w3m or lynx as a filter**, as mutt and aerc do: good text, but no images, colours flattened, and a dependency
  that isn't on every Mac.
- **A real browser engine** (servo, blitz): pixel layout for a pixel screen; far from cells.

## Consequences

- muttrs owns a layout engine: block flow, inline wrapping, lists, quotes, tables. It is the most tested crate.
- CSS beyond the subset is ignored, so some layouts degrade to a single column. That's the trade.
- Authors' colours assume a white page. They are kept only when they read on the theme's background, after
  adjusting their lightness, and backgrounds are dropped.
