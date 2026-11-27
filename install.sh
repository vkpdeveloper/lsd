#!/bin/sh
# Installs lsd, then Leaf, the reader lsd opens documents in:
#
#   curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/lsd/main/install.sh | sh
#
# lsd comes prebuilt from the latest GitHub release for macOS (Apple silicon
# and Intel) and Linux (x86_64 and ARM). The download is checked against its
# SHA-256 and goes to ~/.local/bin, or to $LSD_INSTALL_DIR when that is set.
# Anywhere else it is built from source, which needs Rust (https://rustup.rs).
#
# Leaf is a free, open source (MIT) terminal reader by Deepanshu Mishra,
# https://github.com/DeepanshuMishraa/leaf. It goes next to lsd, installed by
# its own script from https://leaf.dipxsy.app, which checks the download's
# SHA-256.
#
# In a terminal this shows lsd's logo, as a picture where lsd would show
# pictures, and a progress bar. Piped into a file it prints plain lines, and
# NO_COLOR turns the colors off.
#
# Everything runs inside main(), called on the last line, so a download cut
# short never runs half a script.

set -eu

REPO="https://github.com/vkpdeveloper/lsd"
RAW="https://raw.githubusercontent.com/vkpdeveloper/lsd/main"
LEAF_INSTALLER="https://leaf.dipxsy.app"
INSTALL_DIR="${LSD_INSTALL_DIR:-$HOME/.local/bin}"
LOGO_ROWS=6

# Set by style(): LIVE when someone is watching a terminal, then the colors.
LIVE="" BOLD="" DIM="" GREEN="" YELLOW="" RED="" ACCENT="" RESET=""
# The spinner's process while one runs.
SPIN=""

say() { printf '%s\n' "$*"; }
step() { # text [detail]: a finished step
  if [ -n "${2:-}" ]; then
    say "  ${GREEN}✓${RESET} $1  ${DIM}$2${RESET}"
  else
    say "  ${GREEN}✓${RESET} $1"
  fi
}
warn() { say "  ${YELLOW}!${RESET} $*"; }
aside() { say "    ${DIM}$*${RESET}"; }
fail() {
  stop_spinner
  printf '  %s✗%s %s\n' "$RED" "$RESET" "$*" >&2
  exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

style() {
  [ -t 1 ] && [ "${TERM:-dumb}" != dumb ] || return 0
  LIVE=1
  [ -z "${NO_COLOR:-}" ] || return 0
  BOLD=$(printf '\033[1m')
  DIM=$(printf '\033[2m')
  GREEN=$(printf '\033[32m')
  YELLOW=$(printf '\033[33m')
  RED=$(printf '\033[31m')
  # The coral at the bottom of the logo's sun.
  ACCENT=$(printf '\033[38;5;209m')
  RESET=$(printf '\033[0m')
}

fetch() { # url → stdout
  if has curl; then
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --retry-delay 2 "$1"
  elif has wget; then
    wget -q -O - "$1"
  else
    fail "Need curl or wget to download lsd."
  fi
}

# Runs a command with its output saved in $TMP/log, for the caller to show if
# it fails. On a terminal a spinner turns meanwhile, followed by whatever
# `draw` prints. The spinner runs in the background, so Ctrl-C still reaches
# the command.
busy() { # draw command...
  draw=$1
  shift
  if [ -n "$LIVE" ]; then
    rm -f "$TMP/stop"
    spin "$draw" &
    SPIN=$!
  fi
  status=0
  "$@" >"$TMP/log" 2>&1 || status=$?
  stop_spinner
  return "$status"
}

spin() { # draw
  set +e
  frame=0
  printf '\033[?25l'
  while [ ! -e "$TMP/stop" ]; do
    # Drawn in one write, so stopping never cuts a line in half.
    line=$("$1")
    printf '\r\033[K  %s%s%s %s' "$ACCENT" "$(spinner_frame "$frame")" "$RESET" "$line"
    frame=$((frame + 1))
    sleep 0.1 2>/dev/null || sleep 1
  done
}

spinner_frame() { # n
  n=$(($1 % 10))
  set -- ⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏
  shift "$n"
  printf '%s' "$1"
}

stop_spinner() {
  [ -n "$SPIN" ] || return 0
  : >"$TMP/stop"
  wait "$SPIN" 2>/dev/null || true
  SPIN=""
  printf '\r\033[K\033[?25h'
}

megabytes() { # bytes [total bytes]
  awk -v a="$1" -v b="${2:-0}" 'BEGIN {
    if (b > 0) printf "%.1f / %.1f MB", a / 1e6, b / 1e6
    else printf "%.1f MB", a / 1e6
  }'
}

bar() { # done total
  width=30
  filled=$(($1 * width / $2))
  printf '%s' "$ACCENT"
  i=0
  while [ "$i" -lt "$width" ]; do
    if [ "$i" -lt "$filled" ]; then
      printf '━'
    else
      [ "$i" -ne "$filled" ] || printf '%s%s' "$RESET" "$DIM"
      printf '─'
    fi
    i=$((i + 1))
  done
  printf '%s' "$RESET"
}

# The download's status line. curl saves the response headers, so once they
# are in, the bar knows the size.
download_line() {
  # 2>/dev/null first: it must be in place when opening the file fails.
  got=$(wc -c 2>/dev/null <"$TMP/lsd.tar.gz" | tr -d ' ')
  total=$(awk 'tolower($1) == "content-length:" { n = $2 + 0 } END { print n + 0 }' "$TMP/headers" 2>/dev/null)
  got=${got:-0} total=${total:-0}
  if [ "$total" -gt 0 ] && [ "$got" -le "$total" ]; then
    printf 'Downloading lsd  %s  %s' "$(bar "$got" "$total")" "$(megabytes "$got" "$total")"
  else
    printf 'Downloading lsd  %s' "$(megabytes "$got")"
  fi
}

leaf_line() { printf 'Installing Leaf'; }

# The picture protocol lsd would use here: kitty (kitty, Ghostty) or iterm
# (iTerm2, WezTerm). None in tmux, which drops pictures unless set up for them.
picture_protocol() {
  [ -z "${TMUX:-}" ] || return 0
  names=$(printf '%s %s %s' "${TERM:-}" "${TERM_PROGRAM:-}" "${LC_TERMINAL:-}" | tr '[:upper:]' '[:lower:]')
  case "$names" in
    *kitty* | *ghostty*) echo kitty ;;
    # Rex draws with Ghostty's engine.
    *" rex "*) echo kitty ;;
    *iterm* | *wezterm*) echo iterm ;;
    *)
      if [ -n "${KITTY_WINDOW_ID:-}${GHOSTTY_RESOURCES_DIR:-}" ]; then
        echo kitty
      elif [ -n "${WEZTERM_EXECUTABLE:-}${ITERM_SESSION_ID:-}" ]; then
        echo iterm
      fi
      ;;
  esac
}

# Whether the terminal's background is light, from its answer to OSC 11. A
# DA1 request follows, which every terminal answers, so the read ends as soon
# as the answers are in. lsd asks the same way.
light_background() {
  saved=$(stty -g </dev/tty 2>/dev/null) || return 1
  # Raw, so the answer isn't echoed. Each read waits half a second at most.
  stty raw -echo min 0 time 5 </dev/tty 2>/dev/null || return 1
  printf '\033]11;?\033\\\033[c' >/dev/tty
  reply=""
  while chunk=$(dd bs=64 count=1 </dev/tty 2>/dev/null) && [ -n "$chunk" ]; do
    reply=$reply$chunk
    case $reply in *"[?"*c*) break ;; esac
  done
  stty "$saved" </dev/tty
  # rgb:RRRR/GGGG/BBBB, of which the first two digits of each are plenty.
  rgb=${reply#*rgb:}
  [ "$rgb" != "$reply" ] || return 1
  r=$(printf '%.2s' "${rgb%%/*}")
  rgb=${rgb#*/}
  g=$(printf '%.2s' "${rgb%%/*}")
  b=$(printf '%.2s' "${rgb#*/}")
  case "$r$g$b" in *[!0-9a-fA-F]* | "" ) return 1 ;; esac
  [ "${#r}${#g}${#b}" = 222 ] || return 1
  [ $((2126 * 0x$r + 7152 * 0x$g + 722 * 0x$b)) -gt 1275000 ]
}

kitty_picture() { # png
  { base64 <"$1" | tr -d '\n'; echo; } | fold -w 4096 | {
    keys="a=T,f=100,C=1,r=$LOGO_ROWS,q=2"
    IFS= read -r chunk
    while IFS= read -r next; do
      printf '\033_G%s,m=1;%s\033\\' "$keys" "$chunk"
      keys="q=2"
      chunk=$next
    done
    printf '\033_G%s,m=0;%s\033\\' "$keys" "$chunk"
  }
}

iterm_picture() { # png
  printf '\033]1337;File=inline=1;height=%s;preserveAspectRatio=1;doNotMoveCursor=1:%s\a' \
    "$LOGO_ROWS" "$(base64 <"$1" | tr -d '\n')"
}

# lsd's logo: the picture where the terminal shows pictures, else the name.
show_logo() {
  protocol=$(picture_protocol)
  variant=dark
  if [ -n "$protocol" ] && light_background; then
    variant=light
  fi
  say ""
  if [ -z "$protocol" ] || ! has base64 ||
    ! fetch "$RAW/assets/lsd-logo-$variant.png" >"$TMP/logo.png" 2>/dev/null; then
    say "  ${BOLD}${ACCENT}lsd${RESET}  ${DIM}ls + display${RESET}"
    say ""
    return
  fi
  # Room first, then back up into it: the picture lands under the command
  # even at the bottom of the screen, and the cursor ends up below it.
  i=0
  while [ "$i" -lt "$LOGO_ROWS" ]; do
    say ""
    i=$((i + 1))
  done
  esc=$(printf '\033')
  printf '\033[%sA\033[3G%s7' "$LOGO_ROWS" "$esc"
  "${protocol}_picture" "$TMP/logo.png"
  printf '%s8\033[%sB\r' "$esc" "$LOGO_ROWS"
  say ""
}

# The release target for this machine, or nothing when there is no prebuilt lsd.
release_target() {
  arch=$(uname -m)
  case "$(uname -s)" in
    Darwin)
      # A shell running under Rosetta reports x86_64 on Apple silicon.
      if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
        arch=arm64
      fi
      case "$arch" in
        arm64) echo aarch64-apple-darwin ;;
        x86_64) echo x86_64-apple-darwin ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64 | amd64) echo x86_64-unknown-linux-musl ;;
        aarch64 | arm64) echo aarch64-unknown-linux-musl ;;
      esac
      ;;
  esac
}

platform() { # target → what people call it
  case "$1" in
    aarch64-apple-darwin) echo "macOS on Apple silicon" ;;
    x86_64-apple-darwin) echo "macOS on Intel" ;;
    x86_64-unknown-linux-musl) echo "Linux on x86_64" ;;
    aarch64-unknown-linux-musl) echo "Linux on ARM" ;;
  esac
}

home() { # path → path, with ~ for the home folder
  case "$1" in
    "$HOME"/*) printf '~%s\n' "${1#"$HOME"}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

sha256() {
  if has sha256sum; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

install_binary() { # target
  has sha256sum || has shasum || fail "Need sha256sum or shasum to check the download."
  has tar || fail "Need tar to unpack the download."
  target=$1
  url="$REPO/releases/latest/download/lsd-$target.tar.gz"
  if has curl; then
    set -- curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --retry-delay 2 -D "$TMP/headers" -o "$TMP/lsd.tar.gz" "$url"
  else
    set -- wget -q -O "$TMP/lsd.tar.gz" "$url"
  fi
  if ! busy download_line "$@" || ! fetch "$url.sha256" >"$TMP/lsd.sha256"; then
    fail "Couldn't download $url. Try again in a minute, or build it from source: cargo install --locked --git $REPO lsd"
  fi
  step "Downloaded lsd for $(platform "$target")" "$(megabytes "$(wc -c <"$TMP/lsd.tar.gz" | tr -d ' ')")"
  [ "$(cut -d' ' -f1 <"$TMP/lsd.sha256")" = "$(sha256 "$TMP/lsd.tar.gz")" ] ||
    fail "The download doesn't match its SHA-256, so nothing was installed."
  step "Checked its SHA-256"
  tar -xzf "$TMP/lsd.tar.gz" -C "$TMP"
  mkdir -p "$INSTALL_DIR"
  # Copied next to the old binary, then renamed over it, so replacing an lsd
  # that is running still works.
  cp "$TMP/lsd" "$INSTALL_DIR/.lsd.new"
  chmod 755 "$INSTALL_DIR/.lsd.new"
  mv -f "$INSTALL_DIR/.lsd.new" "$INSTALL_DIR/lsd"
  step "Installed $("$INSTALL_DIR/lsd" --version) to $(home "$INSTALL_DIR/lsd")"
}

install_from_source() {
  where="$(uname -s) on $(uname -m)"
  has cargo || fail "There's no prebuilt lsd for $where, and building it needs Rust. Install it from https://rustup.rs, then run this again."
  say "  There's no prebuilt lsd for $where, so it's building from source (this takes a minute or two)"
  cargo install --locked --git "$REPO" lsd
}

leaf_setup() { # bin dir
  fetch "$LEAF_INSTALLER" >"$TMP/leaf-install.sh" &&
    LEAF_INSTALL_DIR=$1 sh "$TMP/leaf-install.sh"
}

install_leaf() { # lsd's bin dir
  for leaf in "$(command -v leaf 2>/dev/null || true)" "$1/leaf" "$HOME/.local/bin/leaf"; do
    if [ -n "$leaf" ] && [ -x "$leaf" ]; then
      step "Leaf is already installed at $(home "$leaf")"
      return 0
    fi
  done
  if busy leaf_line leaf_setup "$1"; then
    version=$("$1/leaf" version 2>/dev/null || true)
    step "Installed Leaf ${version#leaf } to $(home "$1/leaf")"
    aside "Leaf is a free, open source reader by Deepanshu Mishra. lsd opens PDF,"
    aside "EPUB, Markdown, Word, PowerPoint, Excel, ODS, CSV and TSV files in it."
    return 0
  fi
  # Leaf's installer says why it stopped on a line of its own.
  reason=$(sed -n 's/^leaf install: //p' "$TMP/log" | tail -n 1)
  warn "Leaf didn't install, so lsd opens documents in your usual apps for now."
  aside "${reason:-$(tail -n 1 "$TMP/log")}"
  aside "To try again: curl -fsSL $LEAF_INSTALLER | sh"
}

check_path() { # bin dir
  case ":$PATH:" in
    *":$1:"*) ;;
    *)
      say ""
      warn "$(home "$1") isn't on your PATH yet. Add it to your shell's config:"
      say "    zsh:  echo 'export PATH=\"$1:\$PATH\"' >> ~/.zshrc"
      say "    bash: echo 'export PATH=\"$1:\$PATH\"' >> ~/.bashrc"
      say "    fish: fish_add_path $1"
      ;;
  esac
  found=$(command -v lsd 2>/dev/null || true)
  if [ -n "$found" ] && [ "$found" != "$1/lsd" ]; then
    say ""
    warn "Another lsd at $found comes first on your PATH, so typing lsd runs that one."
    aside "Remove it, or move $1 earlier in your PATH."
  fi
}

cleanup() {
  stop_spinner
  rm -rf "$TMP"
}

main() {
  case "$(uname -s)" in
    Darwin | Linux) ;;
    *) fail "lsd runs on macOS and Linux." ;;
  esac
  has curl || has wget || fail "Need curl or wget to download lsd."
  TMP=$(mktemp -d 2>/dev/null || mktemp -d -t lsd)
  trap cleanup EXIT
  trap 'exit 130' INT TERM
  style
  [ -z "$LIVE" ] || show_logo

  target=$(release_target)
  if [ -n "$target" ]; then
    install_binary "$target"
    bin_dir=$INSTALL_DIR
  else
    install_from_source
    bin_dir=${CARGO_HOME:-$HOME/.cargo}/bin
  fi
  install_leaf "$bin_dir"
  check_path "$bin_dir"
  say ""
  say "  Done. Run ${BOLD}lsd${RESET} to list a folder, or ${BOLD}lsd -i${RESET} to browse it."
}

main "$@"
