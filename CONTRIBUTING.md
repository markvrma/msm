# Contributing to msm

Thanks for helping. Small PRs beat big ones; open an issue first for anything large.

## Prerequisites

- Rust (stable, via [rustup](https://rustup.rs)) and a C compiler
- `mpv` (playback, driven over its JSON IPC socket)
- `yt-dlp` (mpv uses it to stream YouTube Music)
- `ffmpeg` / `ffprobe` (optional: art decoding, local tag probing; some tests skip without them)
- `cmusfm` (optional: Last.fm scrobbling)

`install.sh` installs the runtime tools on macOS with Homebrew. On Linux, use your package manager.

## Build, run, test

```sh
cargo build --release
cargo run --release          # launches the TUI
cargo run --release -- auth  # check browser-cookie auth
cargo test                   # default suite: offline, needs ffmpeg on PATH
```

Some tests are `#[ignore]`d because they touch the network, your browser cookie jar, or your speakers. Run them by hand:

```sh
cargo test ytm::tests::live -- --ignored --nocapture   # live YouTube Music search/browse (authed one needs a logged-in browser)
cargo test auth::tests::live -- --ignored --nocapture  # cookie jar + visitor id
cargo test player::tests::check_playback -- --ignored --nocapture  # plays a few seconds of audio via mpv
cargo test art -- --ignored --nocapture                # compares against a Pillow reference; needs python3 + Pillow and cached covers
```

Never make a default test hit the network.

## Code layout

| File | What lives there |
|---|---|
| `src/main.rs` | Entry point, `msm auth` subcommand, shared types (`Track`, `Item`), `$HOME` paths, `on_path` |
| `src/tui.rs` | crossterm UI: browse/search screens, panes, key handling, cell-buffer renderer |
| `src/player.rs` | Background mpv over IPC, queue/repeat/volume/left-ear, cmusfm scrobbling |
| `src/ytm.rs` | YouTube Music client: `ytmapi-rs` for search/browse/like, hand-rolled InnerTube for home feed + history |
| `src/auth.rs` | Browser cookie-jar auth (`rookie`) and SAPISIDHASH signing |
| `src/local.rs` | `~/Music` library scan, play history, album-art cache (`~/.config/ymc/`) |
| `src/art.rs` | Album-art decode and pixelated terminal rendering |
| `src/visualizer.rs` | Real-time audio visualizer (mpv audio filter + ring/circle layouts) |

## Commit style

Lowercase `type : subject` with a space before the colon, imperative, no trailing period. Types in use: `feat`, `fix`, `docs`, `merge`.

```
feat : add full-screen visualizer on v, repeat pane rings to the edges
fix : lay out wide and zero-width chars like ncurses, close drill-in on any other key
docs : one-line curl install via install.sh
```

## Pull request checklist

- [ ] `cargo fmt --all` (CI runs `cargo fmt --all --check`)
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test` passes
- [ ] New logic has a test; new tests are offline unless marked `#[ignore]`
- [ ] Commit messages follow the style above
- [ ] README/keys docs updated if user-visible behaviour changed

## Conduct

See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
