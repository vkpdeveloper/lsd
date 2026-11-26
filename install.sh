#!/bin/sh
# Installs lsd, then Leaf, the reader lsd opens documents in:
#
#   curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/lsd/main/install.sh | sh
#
# Leaf is a free, open source (MIT) terminal reader by Deepanshu Mishra,
# https://github.com/DeepanshuMishraa/leaf. It installs with its own script
# from https://leaf.dipxsy.app, which checks the download's SHA-256.
#
# lsd is built from source for now, so it needs Rust (https://rustup.rs).
#
# Everything runs inside main(), called on the last line, so a download cut
# short never runs half a script.

set -eu

REPO="https://github.com/vkpdeveloper/lsd"
LEAF_INSTALLER="https://leaf.dipxsy.app"

say() { printf '%s\n' "$*"; }
fail() {
  printf 'lsd install: %s\n' "$*" >&2
  exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

fetch() { # url → stdout
  if has curl; then
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --retry-delay 2 "$1"
  elif has wget; then
    wget -q -O - "$1"
  else
    return 1
  fi
}

install_lsd() {
  has cargo || fail "lsd is built from source for now and needs Rust. Install it from https://rustup.rs, then run this again."
  say "Building lsd from $REPO (this takes a minute or two)"
  cargo install --locked --git "$REPO" lsd
}

install_leaf() {
  for leaf in "$(command -v leaf 2>/dev/null || true)" "$HOME/.local/bin/leaf"; do
    if [ -n "$leaf" ] && [ -x "$leaf" ]; then
      say "Leaf is already installed at $leaf."
      return 0
    fi
  done
  say ""
  say "Installing Leaf, a free and open source terminal reader by Deepanshu Mishra"
  say "(https://github.com/DeepanshuMishraa/leaf). lsd opens PDF, EPUB, Markdown,"
  say "Word, PowerPoint, Excel, ODS, CSV and TSV files in it."
  say ""
  # Saved first, so a failed download is caught instead of running nothing.
  script=$(mktemp 2>/dev/null || mktemp -t leaf-install)
  trap 'rm -f "$script"' EXIT INT TERM
  if fetch "$LEAF_INSTALLER" >"$script" && sh "$script"; then
    return 0
  fi
  say ""
  say "Leaf didn't install, so lsd will open documents in your usual apps for now."
  say "To try again: curl -fsSL $LEAF_INSTALLER | sh"
}

main() {
  case "$(uname -s)" in
    Darwin | Linux) ;;
    *) fail "lsd runs on macOS and Linux." ;;
  esac
  install_lsd
  install_leaf
  say ""
  say "Done. Run lsd to list a folder, or lsd -i to browse it."
}

main "$@"
