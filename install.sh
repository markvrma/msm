#!/bin/sh
# Install msm (YouTube Music TUI): checks runtime tools, then builds from git.
#   curl -fsSL https://raw.githubusercontent.com/markvrma/msm/master/install.sh | sh
set -eu

REPO=https://github.com/markvrma/msm
TOOLS="mpv yt-dlp ffmpeg"   # ffmpeg also provides ffprobe
OPTIONAL="cmusfm"           # Last.fm scrobbling only

have() { command -v "$1" >/dev/null 2>&1; }
say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) OS=macos ;;
  Linux) OS=linux ;;
  *) die "unsupported OS '$(uname -s)': msm supports macOS and Linux" ;;
esac

# 1. runtime tools
missing=
for t in $TOOLS; do have "$t" || missing="$missing $t"; done
missing=${missing# }

if [ -n "$missing" ]; then
  if [ "$OS" = macos ] && have brew; then
    say "installing missing tools with brew: $missing"
    # shellcheck disable=SC2086
    brew install $missing
  else
    say "missing tools: $missing"
    say "install them, then re-run this script:"
    if [ "$OS" = macos ]; then
      say "  install Homebrew (https://brew.sh), then: brew install $missing"
    elif have apt-get; then
      say "  sudo apt-get install -y mpv yt-dlp ffmpeg"
    elif have dnf; then
      say "  sudo dnf install -y mpv yt-dlp ffmpeg   # ffmpeg needs RPM Fusion"
    elif have pacman; then
      say "  sudo pacman -S mpv yt-dlp ffmpeg"
    else
      say "  use your package manager to install: $missing"
    fi
    exit 1
  fi
fi

for t in $OPTIONAL; do
  have "$t" || say "note: optional tool '$t' not found: no Last.fm scrobbling (macOS: brew install cmusfm; Linux: https://github.com/Arkq/cmusfm)"
done

# 2. Rust toolchain and a C compiler (some dependencies build C code)
if ! have cargo; then
  say "cargo not found. Install Rust with rustup, then re-run this script:"
  say "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  exit 1
fi
if ! have cc; then
  if [ "$OS" = macos ]; then
    die "no C compiler: run 'xcode-select --install' and re-run"
  fi
  die "no C compiler: install gcc (apt: build-essential, dnf: gcc, pacman: base-devel) and re-run"
fi

# 3. build from master
say "building msm from $REPO (this takes a few minutes)"
cargo install --locked --git "$REPO" msm ||
  { say "retrying without --locked"; cargo install --git "$REPO" msm; }

# 4. next steps
bindir=${CARGO_HOME:-$HOME/.cargo}/bin
case ":$PATH:" in
  *":$bindir:"*) ;;
  *) say "warning: $bindir is not on your PATH; add this to your shell profile:"
     say "  export PATH=\"$bindir:\$PATH\"" ;;
esac

say "installed msm."
say "next: log in to music.youtube.com in Chrome, then run 'msm auth' to check the session"
say "      (MSM_COOKIE_BROWSER=firefox|safari|brave|... for another browser), then run 'msm'."
say "      Optional Last.fm scrobbling: install cmusfm, then run 'cmusfm init' once."
