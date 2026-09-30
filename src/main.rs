//! msm — YouTube Music + local library terminal player with cmusfm scrobbling.
//!
//! mpv runs in the background driven over its JSON IPC socket, so the TUI owns
//! the terminal. cmusfm is fed the same way cmus feeds it as
//! status_display_program.

mod art;
mod auth;
mod local;
mod player;
mod tui;
mod visualizer;
mod ytm;

use serde::{Deserialize, Deserializer, Serialize};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::Arc;

pub const AUDIO_EXT: &[&str] = &[
    ".mp3", ".flac", ".m4a", ".opus", ".ogg", ".wav", ".aac", ".wma",
];

/// $HOME, else the passwd entry (cron, systemd, `env -i`); exits if neither,
/// so nothing is ever written relative to the cwd.
pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return h.into();
    }
    // SAFETY: getpwuid returns null or a pointer into static storage, copied
    // out before any other passwd call.
    let dir = unsafe {
        let pw = libc::getpwuid(libc::getuid());
        (!pw.is_null() && !(*pw).pw_dir.is_null())
            .then(|| std::ffi::CStr::from_ptr((*pw).pw_dir).to_bytes().to_vec())
    };
    match dir.filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(std::ffi::OsString::from_vec(d)),
        None => {
            eprintln!("HOME not set");
            std::process::exit(1);
        }
    }
}
/// Non-empty env var as a path. Ignored under test so the HOME-repointing
/// tests never touch a developer's real config or library.
fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty() && !cfg!(test))
        .map(PathBuf::from)
}
/// $XDG_CONFIG_HOME/ymc, else ~/.config/ymc
pub fn config_dir() -> PathBuf {
    env_dir("XDG_CONFIG_HOME").map_or_else(|| home().join(".config/ymc"), |d| d.join("ymc"))
}
/// config_dir()/history.json
pub fn hist_path() -> PathBuf {
    config_dir().join("history.json")
}
/// config_dir()/art
pub fn art_cache() -> PathBuf {
    config_dir().join("art")
}
/// $MSM_MUSIC_DIR, else ~/Music
pub fn local_music() -> PathBuf {
    env_dir("MSM_MUSIC_DIR").unwrap_or_else(|| home().join("Music"))
}
/// Per-user dir for mpv's IPC socket and verbose log (inspect it on playback
/// failures): $XDG_RUNTIME_DIR/msm, else $TMPDIR/msm-<uid> ($TMPDIR is
/// already per-user on macOS). Created 0700 and checked to be ours, so other
/// local users can't read the log (listening history), block the socket, or
/// plant a symlink for mpv to write through.
pub fn run_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let d = match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(r) => PathBuf::from(r).join("msm"),
        None => std::env::temp_dir().join(format!("msm-{uid}")),
    };
    let _ = std::fs::DirBuilder::new().mode(0o700).create(&d);
    let m = std::fs::symlink_metadata(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    if !m.is_dir() || m.uid() != uid || m.mode() & 0o077 != 0 {
        return Err(format!(
            "{}: not a private directory owned by you",
            d.display()
        ));
    }
    Ok(d)
}
/// $MSM_COOKIE_BROWSER, default "chrome"
pub fn cookie_browser() -> String {
    std::env::var("MSM_COOKIE_BROWSER").unwrap_or_else(|_| "chrome".into())
}

/// One playable track. `url` is a music.youtube.com watch url or a local path.
/// `thumb` is the album art (http url or local path) stamped on so the cover
/// can follow the playing track across queued albums. Serialized shape matches
/// the Python history.json exactly.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(deserialize_with = "de_secs")]
    pub duration: u64,
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thumb: String,
}

/// Accept ints or floats for durations (old history files, ffprobe output).
fn de_secs<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_f64().map(|f| f as u64).unwrap_or(0))
}

/// Local album folder: ~/Music/<name>/ with its audio files (sorted).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalDir {
    pub dir: PathBuf,
    pub files: Vec<String>,
}

/// An album-like list: a history entry, a local folder, a YT rec, or an
/// ad-hoc list built by the TUI. `tracks == None` means lazy / not loaded yet.
/// Only title/tracks/thumb are serialized (history.json shape).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub title: String,
    #[serde(default)]
    pub tracks: Option<Vec<Track>>,
    #[serde(default)]
    pub thumb: String,
    #[serde(skip)]
    pub local: Option<LocalDir>,
    #[serde(skip)]
    pub rec: Option<Item>,
}

/// A YouTube Music search result or home-feed item — the subset of
/// ytmusicapi's parsed dict that msm reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    /// "song" / "album" / "playlist" / "video"...; None for home items.
    pub result_type: Option<String>,
    pub title: String,
    /// raw artist names in order (artists_str filters type words / play counts)
    pub artists: Vec<String>,
    pub video_id: Option<String>,
    pub browse_id: Option<String>,
    pub playlist_id: Option<String>,
    /// album name for song results
    pub album: Option<String>,
    pub duration_seconds: u64,
    /// largest thumbnail url, "" if none
    pub thumb: String,
}

const USAGE: &str = "\
usage: msm          start the player
       msm auth     check YouTube Music sign-in
       msm -h|--help, -V|--version

env:
  MSM_COOKIE_BROWSER  browser to read YouTube cookies from (default chrome)
  MSM_MUSIC_DIR       local library (default ~/Music)
  XDG_CONFIG_HOME     config/history/art under $XDG_CONFIG_HOME/ymc (default ~/.config/ymc)
  XDG_RUNTIME_DIR     mpv socket + log under $XDG_RUNTIME_DIR/msm (default $TMPDIR/msm-<uid>)
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None => {}
        Some("auth") => {
            auth::check_auth();
            return;
        }
        Some("-h" | "--help") => {
            print!("{USAGE}");
            return;
        }
        Some("-V" | "--version") => {
            println!("msm {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some(a) => {
            eprint!("msm: unknown argument '{a}'\n\n{USAGE}");
            std::process::exit(2);
        }
    }
    for tool in ["mpv", "yt-dlp"] {
        if !on_path(tool) {
            eprintln!("missing required tool: {tool}");
            std::process::exit(1);
        }
    }
    for (tool, lost) in [
        ("ffmpeg", "local album art"),
        ("ffprobe", "local track tags and durations"),
        ("cmusfm", "Last.fm scrobbling"),
    ] {
        if !on_path(tool) {
            eprintln!("note: {tool} not found; no {lost}");
        }
    }
    if !on_path("cmusfm") {
        player::SCROBBLE.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    let yt = Arc::new(ytm::get_yt());
    let player = match player::Player::new(yt.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    tui::run(yt, &player);
    player.quit();
}

/// crate::*_path() read $HOME; tests that repoint it or read the real files
/// serialize on this.
#[cfg(test)]
pub static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// shutil.which equivalent: a regular file with an exec bit somewhere on PATH.
pub fn on_path(tool: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p).any(|d| {
                std::fs::metadata(d.join(tool))
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_run_dir_is_private_and_rejects_loose_modes() {
        // only run_dir reads XDG_RUNTIME_DIR
        let base = std::env::temp_dir().join(format!("msm-rundir-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &base);
        let d = super::run_dir().unwrap();
        assert_eq!(d, base.join("msm"));
        let mode = std::fs::metadata(&d).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        // a dir others can enter (pre-planted, or chmod'd) is refused
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(super::run_dir().is_err());
        std::env::remove_var("XDG_RUNTIME_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }
}
