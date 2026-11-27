#!/bin/sh
# Removes lsd and its thumbnail cache:
#
#   curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/lsd/main/uninstall.sh | sh
#
# `lsd --uninstall` runs this same script, built into lsd, with
# LSD_INSTALL_DIR set to the folder of the lsd that ran it.
#
# It removes lsd from where install.sh puts it (~/.local/bin, or
# $LSD_INSTALL_DIR), from cargo's bin folder and from anywhere else on the
# PATH, but only a binary that really is this lsd: LSDeluxe also installs a
# command called lsd, and that one stays. Leaf, its library and your shell
# config are left alone; it says where lsd still comes up.
#
# Everything runs inside main(), called on the last line, so a download cut
# short never runs half a script.

set -eu

GREEN="" YELLOW="" DIM="" RESET=""

say() { printf '%s\n' "$*"; }
step() { say "  ${GREEN}✓${RESET} $*"; }
aside() { say "    ${DIM}$*${RESET}"; }

home() { # path → path, with ~ for the home folder
  case "$1" in
    "$HOME"/*) printf '~%s\n' "${1#"$HOME"}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

# Whether a file is this lsd, by the first line of its help.
is_ours() {
  [ -f "$1" ] && [ -x "$1" ] && "$1" --help 2>/dev/null | head -n 1 | grep -q '^ls + display'
}

remove_binaries() {
  cargo_bin=${CARGO_HOME:-$HOME/.cargo}/bin
  removed=""
  # Where the installer and cargo put it, then every folder on the PATH.
  dirs="${LSD_INSTALL_DIR:-$HOME/.local/bin}:$HOME/.local/bin:$cargo_bin:$PATH"
  old_ifs=$IFS
  IFS=:
  set -f
  set -- $dirs
  set +f
  IFS=$old_ifs
  for dir in "$@"; do
    bin="${dir:-.}/lsd"
    case ":$removed:" in *":$bin:"*) continue ;; esac
    is_ours "$bin" || continue
    if [ "$bin" = "$cargo_bin/lsd" ] && command -v cargo >/dev/null 2>&1 &&
      cargo uninstall lsd >/dev/null 2>&1; then
      : # cargo forgets it too
    else
      rm -f "$bin" || {
        say "  ${YELLOW}!${RESET} Couldn't remove $bin. Remove it yourself: rm $bin"
        continue
      }
    fi
    step "Removed $(home "$bin")"
    removed="$removed:$bin"
  done
  [ -n "$removed" ] || step "Nothing to remove: lsd isn't installed"
}

remove_cache() {
  if [ "$(uname -s)" = Darwin ]; then
    cache="$HOME/Library/Caches/lsd"
  else
    cache="${XDG_CACHE_HOME:-$HOME/.cache}/lsd"
  fi
  [ -d "$cache" ] || return 0
  size=$(du -sh "$cache" 2>/dev/null | cut -f1 | tr -d ' ')
  rm -rf "$cache"
  step "Removed the thumbnail cache at $(home "$cache")${size:+ ($size)}"
}

notes() {
  leaf=$(command -v leaf 2>/dev/null || true)
  [ -n "$leaf" ] || [ ! -x "$HOME/.local/bin/leaf" ] || leaf=$HOME/.local/bin/leaf
  if [ -n "$leaf" ]; then
    say ""
    say "  Leaf is still installed at $(home "$leaf"), for reading on its own."
    aside "To remove it too: rm $leaf"
  fi
  header=""
  for rc in "${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile" \
    "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"; do
    [ -f "$rc" ] || continue
    found=$(grep -n 'lsdcd' "$rc" 2>/dev/null | cut -d: -f1 | paste -sd, - || true)
    [ -n "$found" ] || continue
    if [ -z "$header" ]; then
      say ""
      say "  Your shell config still sets up lsd. Remove these lines when you like:"
      header=1
    fi
    case "$found" in
      *,*) aside "$(home "$rc"), lines $found" ;;
      *) aside "$(home "$rc"), line $found" ;;
    esac
  done
}

main() {
  if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
    GREEN=$(printf '\033[32m') YELLOW=$(printf '\033[33m')
    DIM=$(printf '\033[2m') RESET=$(printf '\033[0m')
  fi
  say ""
  remove_binaries
  remove_cache
  notes
  say ""
  say "  To install lsd again: curl -fsSL https://raw.githubusercontent.com/vkpdeveloper/lsd/main/install.sh | sh"
}

main "$@"
