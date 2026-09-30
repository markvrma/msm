# Security policy

## Reporting a vulnerability

Please report security issues privately through GitHub: **Security → Report a
vulnerability** on this repository. Don't open a public issue for them.

I'll acknowledge a report within a few days and keep you updated until it's fixed.

## Scope

msm reads your YouTube/Google session cookies from your local browser store and
sends them to `music.youtube.com` / `s.youtube.com` (and, through yt-dlp, to
YouTube). Issues that could leak, persist, or misdirect those cookies are the
most important. Also in scope: the mpv IPC socket and log files under
`$XDG_RUNTIME_DIR/msm` (or `$TMPDIR/msm-<uid>`), and the shell installer
`install.sh`.

Only the latest release is supported.
