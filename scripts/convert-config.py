#!/usr/bin/env python3
"""Writes the init.vim lines that say what muttrs' themes, keys and look said before wcgw-kit.

muttrs no longer reads `[keys.*]`, `theme`, `icons`, `images` and `mouse` in config.toml, nor themes in
themes/*.toml: the kit's init.vim says them, as `:map`, `:colorscheme`, `:set` and `:hi` lines (wcgw-kit's D6). Run it once, and put what it prints in ~/.config/muttrs/init.vim:

    scripts/convert-config.py ~/.config/muttrs/config.toml ~/.config/muttrs/themes/mine.toml >> init.vim

Keys are mapped to the commands that do now what the keys they typed did; what it can't say so is left as a
comment. A theme's styles go on the groups that draw the same now; those without one are comments too.
"""

import sys
import tomllib

# What a key typed did, by where, as the command that does it now.
COMMANDS = {
    "m": "compose",
    "r": "reply",
    "R": "reply all",
    "L": "reply list",
    "f": "forward",
    "F": "flag",
    "U": "unread",
    "Y": "pick labels",
    "d": "delete",
    "a": "archive",
    "s": "pick move",
    "u": "undo",
    "|": "pipe",
    "!": "shell",
    "e": "resume",
    "S": "grep",
    "T": "threads",
    "$": "edit!",
    "?": "help",
    "q": "exit",
    "\\": "sidebar",
    "<Enter>": "open",
    "<Space>": "toggle",
}
PAGER = {
    "o": "pick links",
    "v": "pick attachments",
    "h": "details",
    "T": "body",
    "I": "remote",
    "H": "voight heatmap",
    "J": "next",
    "K": "previous",
    "<Space>": None,
}
DRAFT = {
    "y": "send",
    "e": "draft",
    "a": "attach",
    "D": "detach",
    "P": "postpone",
    "q": "discard",
}

# A theme's styles, by the group that draws the same now.
UI = {
    "dim": "MuttrsDim",
    "accent": "MuttrsAccent",
    "cursor": "CursorLine",
    "cursor_unfocused": "MuttrsOpen",
    "unread": "MuttrsUnread",
    "read": "MuttrsRead",
    "flagged": "MuttrsFlagged",
    "label": "MuttrsLabel",
    "subject": "MuttrsSubject",
    "attachment": "MuttrsAttachment",
    "banner": "MuttrsBanner",
    "found": "Search",
    "found_current": "CurSearch",
    "status": "StatusLine",
    "error": "MuttrsError",
    "voight": "MuttrsVoight",
    "working": "MuttrsWorking",
    "heatmap_human": "VoightHeatHuman",
    "heatmap_even": "VoightHeatEven",
    "heatmap_llm": "VoightHeatLlm",
}
BODY = {
    "text": "Normal",
    "link": "@markup.link",
    "heading": "@markup.heading",
    "code": "@markup.raw",
    "rule": "NonText",
    "table_header": "Bold",
}
ATTRIBUTES = {
    "bold": "bold",
    "italic": "italic",
    "underlined": "underline",
    "reversed": "reverse",
    "crossed_out": "strikethrough",
}


def settings(config):
    """The `:colorscheme` and `:set` lines of config.toml's `theme`, `icons`, `images` and `mouse`."""
    lines = []
    theme = config.get("theme")
    if theme and theme != "tokyonight-moon":
        lines.append(f"colorscheme {'default' if theme == 'terminal' else theme}")
    icons = {True: "nerd", False: "plain"}.get(config.get("icons"))
    if icons:
        lines.append(f"set icons={icons}")
    graphics = {"kitty": "kitty", "blocks": "halfblocks", "off": "off"}.get(config.get("images"))
    if graphics:
        lines.append(f"set graphics={graphics}")
    if config.get("mouse") is False:
        lines.append("set mouse=")
    return lines


def keys(config):
    """The `:nnoremap` lines of config.toml's `[keys.*]`."""
    lines = []
    for where, mapped in config.get("keys", {}).items():
        lines.append(f'" [keys.{where}]: keys are the same everywhere now')
        known = {**COMMANDS}
        if where == "pager":
            known.update(PAGER)
        if where == "draft":
            known = DRAFT
        for key, typed in mapped.items():
            if key in ("<Space>", " "):
                lines.append(f'" {key} = "{typed}": <Space> is <leader> now; map another key')
            elif typed.startswith(":"):
                lines.append(f"nnoremap {key} {typed.replace('<Enter>', '<CR>')}")
            elif known.get(typed):
                lines.append(f"nnoremap {key} :{known[typed]}<CR>")
            else:
                lines.append(f'" {key} = "{typed}": no command does that now; see :help')
    return lines


def spec(style):
    """A style as `:hi` takes it."""
    parts = []
    for side in ("fg", "bg"):
        if side in style:
            parts.append(f"gui{side}={style[side]}")
    attributes = [ATTRIBUTES[name] for name in ATTRIBUTES if style.get(name)]
    if attributes:
        parts.append("gui=" + ",".join(attributes))
    return " ".join(parts) or "NONE"


def theme(text):
    """The `:hi` lines of a theme's file."""
    lines = []
    base = text.get("base", "tokyonight-moon")
    lines.append(f"colorscheme {'default' if base == 'terminal' else base}")
    if "page" in text:
        page = text["page"]
        lines.append(f"hi MuttrsPage {'NONE' if page == 'none' else 'guibg=' + page}")
    for section, groups in (("ui", UI), ("body", BODY)):
        for name, style in text.get(section, {}).items():
            group = groups.get(name)
            if name == "quotes" and style:
                lines.append(f"hi @markup.quote {spec(style[0])}")
            elif group:
                lines.append(f"hi {group} {spec(style)}")
            else:
                lines.append(f'" {section}.{name}: no group of its own now')
    return lines


def main(paths):
    if not paths:
        sys.exit(__doc__)
    for path in paths:
        with open(path, "rb") as file:
            text = tomllib.load(file)
        print(f'" From {path}')
        if any(section in text for section in ("base", "page", "ui", "body")):
            print("\n".join(theme(text)))
        else:
            print("\n".join(settings(text) + keys(text)))


if __name__ == "__main__":
    main(sys.argv[1:])
