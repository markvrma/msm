# Changelog

## 0.1.0 — unreleased

First public release: a Rust rewrite of the Python `msm-player`.

- Browse, search and queue YouTube Music (songs, albums, playlists), anonymous or signed in.
- Local library player (`~/Music`, or `MSM_MUSIC_DIR`) with tags and cover art via ffprobe/ffmpeg.
- Browser-cookie sign-in (`msm auth`): plays recorded to YouTube Music history, likes, personalized FOR YOU feed.
- Optional Last.fm scrobbling through `cmusfm`.
- Pixelated 256-colour album art and a full-screen audio visualizer.
- Repeat-all, left-ear-only mode, per-app volume, Ctrl-Z suspend.
- `msm --help` / `--version`.
- Per-user mpv socket and log (`$XDG_RUNTIME_DIR/msm`, else `$TMPDIR/msm-<uid>`, mode 0700); a second instance refuses to start.
- `yt-dlp` is checked at startup; `cmusfm`, `ffmpeg` and `ffprobe` are optional and degrade with a note.
