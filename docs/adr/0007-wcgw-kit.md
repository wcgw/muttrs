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
  over each thread's rows, so that `za`, `zM` and `zR` are the kit's. The row under the cursor is drawn as before,
  as the kit has no `'cursorline'` yet.
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
- `config.toml` keeps what's muttrs' own. Themes (`themes/*.toml`) and keys (`[keys.*]`) are no longer read:
  `init.vim` says them, as `:hi` and `:map` lines, which `scripts/convert-config.py` writes from them. `theme`,
  `icons`, `images` and `mouse` still say how muttrs starts, as `:colorscheme` and `:set` lines before `init.vim`.
- `:` commands are named as vim's are, letters only: `:not-junk` is `:notjunk`, `:delete-mailbox` `:delmailbox`,
  `:forward-attached` `:forward attached`, and `:source` (the Message as it came) `:raw`, as `:source` is vim's.
- What the kit can't do yet, muttrs does without, until it can:
  - `!` and `|` run their command without the terminal, showing what it writes in a buffer: the kit has no way to
    hand the terminal to a command but `$EDITOR`.
  - The History is the kit's, kept while muttrs runs, not on disk.
  - Prompts don't complete: `:compose ADDRESS` completes addresses, `c`'s To: prompt doesn't.
  - Quitting with Tasks left waits for them on `q` only: `:q` and `:qa` are the kit's, and quit at once.
  - The kit puts a window's cursor on a block only the first time it shows a buffer: to go back to the Index on the
    Message read last, or keep the mini-index on it, muttrs opens a window in place of the one it had.
  - Pickers have one verb: copying a Link and saving an Attachment are pickers of their own.
- Building needs wcgw-kit checked out beside muttrs until it's published, as voight is pinned.
