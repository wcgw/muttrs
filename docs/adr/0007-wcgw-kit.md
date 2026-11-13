---
status: accepted
---

# Run in wcgw-kit, as the other wcgw apps do

muttrs runs in [wcgw-kit](https://github.com/wcgw/wcgw-kit), the editor shell devnull, ghd and slackers run in too.
The kit owns the loop, the terminal, the keys and the keymap, the `:` line and its commands, windows, the chrome
(tabline, winbars, one statusline, the message line), pickers and prompts, Pictures, the mouse, highlight groups and
colorschemes, and `init.vim`. muttrs owns its domain (muttrs-mail: Accounts, IMAP and SMTP, OAuth2, threads, Labels,
Filters, Tasks, Drafts, contacts, the cache) and says what it shows as the kit's buffers:

- **The sidebar**: an unlisted buffer in a window at the left, as wide as before; an Account's Mailboxes fold into
  its row, and a Mailbox holding others into its own. `\` (`:sidebar`) shows or hides it.
- **An Index per open Mailbox**: a listed buffer, its rows laid out for the window's width as before. A block per
  thread (per Message, out of threads), so that the cursor stays on its thread as Envelopes come and go, and a fold
  over each thread's rows, so that `za`, `zM` and `zR` are the kit's; the kit highlights the row under the cursor
  (its `'cursorline'`).
- **A Pager per Message read**: headers, then the body as wcgw-kit-text (once muttrs-render) lays it out for the
  width, its Pictures lines the kit draws over; Links refer to their URL, so `<CR>` on one opens it.
- **The Draft**, as it will be sent, until it's sent, postponed or discarded; it's written in `$EDITOR`, which the
  kit hands the terminal.

Every action is a `:` command, so `:map` reaches anything a key does, and the keys follow the kit's (wcgw-kit's D12):
vim's motions pure, and mail's verbs on the keys the other apps use for theirs (`c` new, `r` reply, `s` flag, `dd`
delete, `K` details, `gx` open a Link, `<C-l>` refresh); the rest under `<leader>`. Lists that were overlays (Links,
Attachments, Mailboxes to move to, Labels, what a search of every Mailbox found) are the kit's pickers; To, Subject,
what to search the server for, what to pipe to, are its prompts. Colours are highlight groups, `Muttrs…`, linked to
the standard ones, with tokyonight-moon's own as muttrs' theme had them. voight's Assessor is voight's own now, shared
with ghd.

## Considered options

- **Keeping muttrs' own UI**: what it had worked, but four apps each with their own keys, prompt, History, overlays and
  themes drifted apart, muttrs' and ghd's voight code too, and every fix was made four times.
- **The kit's widgets only**, muttrs keeping its loop and keys: the feel (counts, maps, completion, which-key) is what
  drifted, and it's in the dispatch.

## Consequences

- Keys changed: `g` is a prefix now (`gg`), `?` searches backwards (help is `:help`, `F1`), `<Space>` is `<leader>`
  (selecting is `x`), `s` flags (moving is `<Space>m`), `$` is the end of the line (refreshing is `<C-l>`), `J` and
  `K` don't step through Messages (`<C-n>` and `<C-p>` do), `h` is a motion (every header is `K`), `H` is a motion
  (the Heatmap is `<Space>h`), and a Draft's keys are commands (`:send`, `:postpone`, `:attach`, `:detach`).
- `gx` is `:browse link`, and `gy` yanks what it opens, as in the other apps, where `:browse` alone opens the thing
  itself on the web: a Message has no page there, so `:browse` alone says so.
- Selecting is as in ghd: `x` selects, or unselects, and moves down, as `<Space>` did and ghd's `x` does; `A` selects
  all listed, or none, as ghd's; `<Esc>` (`:clear`) unselects every Message, or else lists them all again, as ghd's
  clears its Selection, or else its filter. Before either, it hides the search's matches, as the kit's `<Esc>`
  (`:nohlsearch`) does in every app. It no longer goes back from what the server found (`q` does).
- `\` is slackers': from the sidebar, it hides it; from anywhere else, it goes to it, bringing it back if hidden.
- `config.toml` keeps what's muttrs' own. Themes (`themes/*.toml`) and keys (`[keys.*]`) are no longer read:
  `init.vim` says them, as `:hi` and `:map` lines, which `scripts/convert-config.py` writes from them. `theme`,
  `icons`, `images` and `mouse` are read no more either, as ghd's `theme` and `icons`: a warning says so, and the
  script writes them as `:colorscheme` and `:set` lines.
- `:` commands are named as vim's are, letters only: `:not-junk` is `:notjunk`, `:delete-mailbox` `:delmailbox`,
  `:forward-attached` `:forward attached`, and `:source` (the Message as it came) `:raw`, as `:source` is vim's.
- Asking for what's new is `<C-l>` (`:e!`), as in every app: `:refresh` goes, and `:sync` lists the open Mailbox
  anew, as `:sync!` did.
- A Mailbox opens by name with `:e NAME`, as a buffer does in every app: `:cd` goes.
- `:limit` is `:filter`, as ghd's: it narrows the Index to what a Filter picks, and the winbar says `filter …`.
- `<leader>f` is the kit's find prefix, as fzf-lua's in the author's Neovim: `<leader>ff` finds a Mailbox to open
  (`:pick mailboxes`), as every app finds its own, so forwarding is `<leader>F`, and moving is `:pick move`.
- `:voight on` and `off` start and stop assessing, as ghd's do; they showed and hid the Heatmap, which
  `:voight heatmap` (`<leader>h`) does.
- The History is the kit's, in the file muttrs kept it in: commands and searches; what `!` and `|` ran isn't kept.
- Quitting with Tasks left waits for them, through the kit's quit hook; `:qa!` doesn't.
- In pickers, `<CR>` opens; `<C-y>` yanks a Link, `<C-s>` saves an Attachment, as the kit's picker verbs. In the
  Labels', `<CR>` adds or takes off a Label and the list stays open, as muttrs' did.
- `!` and `|` hand the terminal to their command, as the kit does, waiting for Enter after; `|` pipes the Messages
  byte for byte. `!` and `:shell` (and `:!CMD`) are the kit's now, in every app, as muttrs had them.
- The Draft is edited in an `.eml` file, in the configuration's `editor`, else the kit's (`$EDITOR`, `$VISUAL`).
- Building needs wcgw-kit checked out beside muttrs until it's published, as voight is pinned.
