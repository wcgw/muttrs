---
status: accepted
---

# Draw Pictures as cells: kitty Unicode placeholders, or half blocks

Every Picture muttrs draws is made of ordinary cells in ratatui's buffer, so that it scrolls, clips and redraws with
the text of the Pager like any line, and nothing needs deleting when it moves off screen.

- **kitty Unicode placeholders**, where the terminal has them (Ghostty, kitty): each image is transmitted once, then
  drawn by writing placeholder characters whose foreground colour names it. Full resolution. On Linux, pixels go
  through a file in `/dev/shm`, which the terminal reads and deletes; elsewhere, such as macOS, in the escape sequence
  itself. This is slackers' approach (its ADR 0002), ported.
- **Half blocks** everywhere else: `▀` with the upper pixel in the foreground and the lower in the background, two
  pixels a cell, in truecolor (or the nearest of the 256 colours). Low resolution, but in any terminal, under tmux,
  and over SSH.

The terminal is picked from its environment (`TERM`, `TERM_PROGRAM`, `KITTY_WINDOW_ID`; tmux and zellij get half
blocks, as placeholders need passthrough), and the configuration can force either, or none.

## Considered options

- **ratatui-image** (Sixel, iTerm2, kitty): one image per rectangle, drawn outside the buffer, and scrolling a long
  Message means re-placing every image.
- **Sixel**: outside the buffer too, and quantised.
- **iTerm2's inline images**: no placement by cells; maybe later, for iTerm2 alone.

## Consequences

- Terminals that speak kitty graphics without placeholders (WezTerm, Konsole) get half blocks.
- Placeholder images are uploaded at the size they're drawn: resizing the terminal uploads them again.
- The terminal's cell size in pixels, which sizes Pictures, comes from `TIOCGWINSZ`. Where it isn't reported, 10 × 20
  pixels is assumed.
