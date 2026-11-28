<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/lsd-logo-dark.svg">
    <img src="assets/lsd-logo-light.svg" alt="lsd" width="200">
  </picture>
</h1>

`ls` + display. It lists files with real thumbnails, like a Finder window in
your terminal. `lsd -i` opens a full browser with vim keys, a preview pane,
Quick Look and file operations.

- Thumbnails for pictures, videos, PDFs, book covers and app icons, from
  QuickLook on macOS and the freedesktop thumbnailers on Linux. Code and
  JSON preview as colored text, and Markdown as formatted text.
- `.env` files preview with their values hidden. Press `zv` to show them.
- Web pages preview as the page itself, drawn by WebKit on macOS and by
  Chrome, Chromium, Brave or Edge on Linux (`chrome-headless-shell` is
  fastest). They load only files from the page's own folder and never touch
  the network. Until the picture is ready, or with no browser, you see the
  source.
- Icons, list, columns and gallery views, Finder-style dates, sizes and kinds.
- Git aware: changed files are marked, and the preview pane shows a changed
  file's diff.
- Matches your terminal's colors on its own, light or dark.
- PDFs, EPUBs, Markdown, office files and tables open in
  [Leaf](https://github.com/DeepanshuMishraa/leaf), right in the terminal.
  Quit Leaf and you are back where you were. See [Leaf](#leaf) for the list.
- Fast: a folder of pictures lists in under 10 ms, and the browser builds
  a frame in well under a millisecond.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/lsd/main/install.sh | sh
```

It works on macOS (Apple silicon and Intel) and Linux (x86_64 and ARM, any
distro). It downloads lsd from the latest release, checks it against its
SHA-256 and puts it in `~/.local/bin`. It also installs Leaf next to it with
Leaf's own installer, unless you already have it. To update lsd later, run
`lsd --update`.

To put lsd somewhere else, set `LSD_INSTALL_DIR`:

```sh
curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/lsd/main/install.sh | LSD_INSTALL_DIR=~/bin sh
```

To uninstall lsd and its thumbnail cache, run `lsd --uninstall`, or:

```sh
curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/lsd/main/uninstall.sh | sh
```

Either way, it leaves Leaf and your shell config alone, and tells you what's
left.

Or by hand: download lsd for your machine from the
[latest release](https://github.com/vkpdeveloper/lsd/releases/latest), or
build it with [Rust](https://rustup.rs):

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
lsd page.html       # one file: the page as a picture
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
| `gd` `gD` | Focus the diff in the preview pane, show it full screen. See [Git](#git) |
| `zi` `zl` `zc` `zg` | Icons, list, columns, gallery |
| `zt` `zz` `zb`, `Ctrl-e` `Ctrl-y` | Scroll |
| `zh` `.` | Hidden files |
| `zv` | Values in `.env` files |
| `zp` `zs` | Preview pane, sidebar |
| `s` `S` | Sort by, reverse |
| `O` | Show in Finder |
| `:cd path`, `:12` | Go to a folder, to item 12 |
| `q` `ZZ` `:q` | Quit |

The mouse works too: click, double click, scroll, and click the toolbar.

## Git

Inside a git repository, the browser marks what changed and shows the
branch in the bottom bar, or the short commit when HEAD is detached, with
`↑` and `↓` for commits ahead of and behind its upstream.

| Mark | |
| --- | --- |
| `M` | Modified |
| `A` | Added |
| `U` | Untracked |
| `R` | Renamed |
| `!` | Conflict |
| `•` | A folder with changes inside |
| dimmed name | Ignored |

When the selected file has changes, the preview pane shows its diff instead
of the file: the file as it is now against the last commit, so staged and
unstaged changes both show. Untracked files show as all added, a renamed file
says where it came from, and a conflict shows git's conflict markers as added
lines. Binary files keep their usual preview. Deleted files are not in the
folder anymore, so only the folder's mark shows them.

`gd` moves the keys into the diff and `gD` shows it full screen:

| Keys | |
| --- | --- |
| `j` `k`, `Ctrl-d` `Ctrl-u`, `Ctrl-f` `Ctrl-b` | Scroll a line, half a page, a page |
| `gg` `G` | Top, bottom |
| `n` `N`, `]` `[` | Next, previous change |
| `h` `l`, `0` | Scroll sideways, back to the start |
| `Enter` | Full screen, and back to the pane |
| `Esc` `q` `gd` | Back to the files |

Diffs longer than 20,000 lines show their first 20,000. Plain `lsd` and
`lsd -l` don't ask git, so they stay as fast as `ls`.

## Leaf

lsd opens documents with [Leaf](https://github.com/DeepanshuMishraa/leaf), a
terminal reader by Deepanshu Mishra, shared under the MIT license. Leaf does
the reading, and it does it well. Thank you, Deepanshu.

lsd still draws everything else itself: the listing, the thumbnails, the
preview pane and Quick Look. Leaf starts only when you open a file. In the
browser, `Enter` on one of these files opens it in Leaf, in the same terminal:

| Files | Extensions |
| --- | --- |
| PDF | `.pdf` |
| Books | `.epub` |
| Markdown | `.md` `.markdown` `.mdown` `.mkd` |
| Word, PowerPoint, Excel | `.docx` `.pptx` `.xlsx` |
| OpenDocument spreadsheets | `.ods` |
| Tables | `.csv` `.tsv` |

Press `Esc` in Leaf to come back to lsd. `gx` opens any of these in
its usual app instead, and so does `Enter` when Leaf isn't installed.
Everything else, including pictures, videos, code and older `.doc`, `.xls`
and `.ppt` files, opens in its usual app.

## Terminals

| Terminal | Pictures |
| --- | --- |
| Ghostty, kitty, Rex | kitty graphics |
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
  reads documents for lsd. See [Leaf](#leaf) for which ones.
- Code coloring uses the syntax definitions from
  [bat](https://github.com/sharkdp/bat), through
  [two-face](https://github.com/CosmicHorrorDev/two-face).

## License

MIT. See [LICENSE](LICENSE).
