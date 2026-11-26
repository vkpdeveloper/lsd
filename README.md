# lsd

`ls` + display. It lists files with real thumbnails, like a Finder window in
your terminal. `lsd -i` opens a full browser with vim keys, a preview pane,
Quick Look and file operations.

- Thumbnails for pictures, videos, PDFs, book covers and app icons, from
  QuickLook on macOS and the freedesktop thumbnailers on Linux. Code and
  JSON preview as colored text.
- Icons, list, columns and gallery views, Finder-style dates, sizes and kinds.
- Matches your terminal's colors on its own, light or dark.
- PDFs, EPUBs, Markdown and office files open in [Leaf](https://github.com/DeepanshuMishraa/leaf),
  right in the terminal. Quit Leaf and you are back where you were.
- Fast: a folder of pictures lists in under 10 ms, and the browser builds
  a frame in well under a millisecond.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/lsd/main/install.sh | sh
```

This builds lsd from source, so it needs [Rust](https://rustup.rs). It also
installs Leaf with Leaf's own installer, unless you already have it.

Or by hand:

```sh
cargo install --locked --git https://github.com/vkpdeveloper/lsd lsd
curl -fsSL https://leaf.dipxsy.app | sh   # optional, for documents
```

LSDeluxe, the `ls` replacement, also installs a command called `lsd`.
Whichever comes first on your `PATH` wins.

## Use

```sh
lsd                 # icon view of this folder
lsd -l ~/Downloads  # list view: date modified, size, kind
lsd -T -L 2         # tree, two levels deep
lsd -g photos/      # gallery: a large preview of each item
lsd data.json       # one file: pretty-printed JSON, colored code
lsd -i              # browse
```

Sort with `-t` (date), `-S` (size), `-X` (kind), `-r` to reverse, and
`--dirs-first`. `-a` shows hidden files, and `-s small|medium|large|huge` sets
the icon size. See `lsd --help` for the rest.

To end up in the folder you browsed to, add this to your shell config:

```sh
lsdcd() {
  local file; file=$(mktemp)
  lsd -i --cwd-file "$file" "$@" && cd "$(cat "$file")"
  rm -f "$file"
}
```

## Browser keys

The browser reads keys like vim. Counts work with moves and commands (`5j`,
`3dd`, `10G`), and `y` and `d` take any move (`yG`, `d3j`). Press `?` for the
list inside lsd.

| Keys | |
| --- | --- |
| `h` `j` `k` `l`, arrows | Move. In the list and columns, `h` and `l` go up and into folders |
| `gg` `G` | First, last (or the item a count names) |
| `Ctrl-d` `Ctrl-u`, `Ctrl-f` `Ctrl-b` | Half a page, a page |
| `H` `M` `L` | Top, middle, bottom of the screen |
| `0` `$` | Start, end of a row of icons |
| `f`x `F`x `;` `,` | Next, previous name starting with x |
| `/` | Filter. `n` and `N` find the last filter again |
| `Enter` | Open: folders in place, documents in Leaf, the rest in their app |
| `gx` | Open in the default app |
| `-` `Backspace` | Enclosing folder |
| `Ctrl-o` `Ctrl-i`, `[` `]` | Back, forward |
| `~` | Home |
| `yy` `dd` `p` | Copy, cut, paste. `x` cuts too |
| `v` `V` | Select a range, then `y`, `d` or `D`. `o` jumps to the other end |
| `m` | Mark. `yy`, `dd` and `D` act on marked items |
| `yp` `yn` | Copy paths, names to the clipboard |
| `r` `I` `A` | Rename, with the cursor at the extension, the start, the end |
| `cw` `cc` | Replace the name and keep the extension, replace all of it |
| `a` | New folder |
| `D` `Delete` | Move to the Trash |
| `Space` | Quick Look |
| `zi` `zl` `zc` `zg` | Icons, list, columns, gallery |
| `zt` `zz` `zb`, `Ctrl-e` `Ctrl-y` | Scroll |
| `zh` `.` | Hidden files |
| `zp` `zs` | Preview pane, sidebar |
| `s` `S` | Sort by, reverse |
| `O` | Show in Finder |
| `:cd path`, `:12` | Go to a folder, to item 12 |
| `q` `ZZ` `:q` | Quit |

The mouse works too: click, double click, scroll, and click the toolbar.

## Terminals

| Terminal | Pictures |
| --- | --- |
| Ghostty, kitty | kitty graphics |
| WezTerm, iTerm2 | iTerm2 images |
| Alacritty, Terminal.app, others | Colored half blocks and emoji icons |

In tmux, pictures need passthrough. Add this to `~/.tmux.conf`:

```
set -g allow-passthrough on
```

`LSD_IMAGES=kitty|iterm|text` picks the picture protocol yourself, and
`LSD_ICONS=nerd|none` (or `--icons`) picks the text icons.

## Credits

- [Leaf](https://github.com/DeepanshuMishraa/leaf) by Deepanshu Mishra (MIT)
  reads documents for lsd.
- Code coloring uses the syntax definitions from
  [bat](https://github.com/sharkdp/bat), through
  [two-face](https://github.com/CosmicHorrorDev/two-face).

## License

MIT
