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
# https://github.com/DeepanshuMishraa/leaf. It installs with its own script
# from https://leaf.dipxsy.app, which checks the download's SHA-256.
#
# Everything runs inside main(), called on the last line, so a download cut
# short never runs half a script.

set -eu

REPO="https://github.com/vkpdeveloper/lsd"
LEAF_INSTALLER="https://leaf.dipxsy.app"
INSTALL_DIR="${LSD_INSTALL_DIR:-$HOME/.local/bin}"

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
    fail "Need curl or wget to download lsd."
  fi
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
  url="$REPO/releases/latest/download/lsd-$1.tar.gz"
  say "Downloading lsd for $1"
  if ! fetch "$url" >"$TMP/lsd.tar.gz" || ! fetch "$url.sha256" >"$TMP/lsd.sha256"; then
    fail "Couldn't download $url. Try again in a minute, or build it from source: cargo install --locked --git $REPO lsd"
  fi
  [ "$(cut -d' ' -f1 <"$TMP/lsd.sha256")" = "$(sha256 "$TMP/lsd.tar.gz")" ] ||
    fail "The download doesn't match its SHA-256, so nothing was installed."
  tar -xzf "$TMP/lsd.tar.gz" -C "$TMP"
  mkdir -p "$INSTALL_DIR"
  # Copied next to the old binary, then renamed over it, so replacing an lsd
  # that is running still works.
  cp "$TMP/lsd" "$INSTALL_DIR/.lsd.new"
  chmod 755 "$INSTALL_DIR/.lsd.new"
  mv -f "$INSTALL_DIR/.lsd.new" "$INSTALL_DIR/lsd"
  say "Installed $("$INSTALL_DIR/lsd" --version) to $INSTALL_DIR/lsd"
}

install_from_source() {
  where="$(uname -s) on $(uname -m)"
  has cargo || fail "There's no prebuilt lsd for $where, and building it needs Rust. Install it from https://rustup.rs, then run this again."
  say "There's no prebuilt lsd for $where, so it's building from source (this takes a minute or two)"
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
  if fetch "$LEAF_INSTALLER" >"$TMP/leaf-install.sh" && sh "$TMP/leaf-install.sh"; then
    return 0
  fi
  say ""
  say "Leaf didn't install, so lsd will open documents in your usual apps for now."
  say "To try again: curl -fsSL $LEAF_INSTALLER | sh"
}

check_path() { # bin dir
  case ":$PATH:" in
    *":$1:"*) ;;
    *)
      say ""
      say "$1 isn't on your PATH yet. Add it to your shell's config:"
      say "  zsh:  echo 'export PATH=\"$1:\$PATH\"' >> ~/.zshrc"
      say "  bash: echo 'export PATH=\"$1:\$PATH\"' >> ~/.bashrc"
      say "  fish: fish_add_path $1"
      ;;
  esac
  found=$(command -v lsd 2>/dev/null || true)
  if [ -n "$found" ] && [ "$found" != "$1/lsd" ]; then
    say ""
    say "Another lsd at $found comes first on your PATH, so typing lsd runs that one."
    say "Remove it, or move $1 earlier in your PATH."
  fi
}

main() {
  case "$(uname -s)" in
    Darwin | Linux) ;;
    *) fail "lsd runs on macOS and Linux." ;;
  esac
  TMP=$(mktemp -d 2>/dev/null || mktemp -d -t lsd)
  trap 'rm -rf "$TMP"' EXIT
  trap 'exit 130' INT TERM

  target=$(release_target)
  if [ -n "$target" ]; then
    install_binary "$target"
    bin_dir=$INSTALL_DIR
  else
    install_from_source
    bin_dir=${CARGO_HOME:-$HOME/.cargo}/bin
  fi
  install_leaf
  check_path "$bin_dir"
  say ""
  say "Done. Run lsd to list a folder, or lsd -i to browse it."
  say "For tab completion, see $REPO#tab-completion"
}

main "$@"
