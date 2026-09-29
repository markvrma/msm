//! Audio visualizer for the FOR YOU pane: one ring of particles per frequency
//! band, bass at the center and highs outward (a speaker cone, not a target),
//! the last ring filling out to the pane edges. A particle climbs a glyph
//! ladder as its band gets louder -- it "jumps out" of the screen -- and sinks
//! back down the hollow ladder as it decays.
//!
//! Only msm's own audio drives it: mpv measures the bands itself. `af()` is a
//! labelled lavfi filter that splits a mono copy into one bandpass per band,
//! merges those as extra channels in front of the untouched stereo, tags each
//! frame with per-channel RMS (astats), then pans the stereo back out
//! bit-exact. The tags are read as the `af-metadata/msmviz` property over the
//! IPC socket the TUI already polls: no FIFO, no second decoder, no FFT here.

use crate::art::Grid;
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const LABEL: &str = "msmviz";
pub const META: &str = "af-metadata/msmviz";

/// Band centers (Hz), 1.3 octaves wide each, innermost ring first. Six is
/// what a ~45x20 pane can still show as separate rings. The top one stays
/// under Nyquist even at 22.05kHz, where a bandpass past it would break the
/// graph -- and with it the audio.
const BANDS: [u32; 6] = [60, 150, 400, 1000, 2500, 6300];
const N: usize = BANDS.len();
/// mpv filters this far ahead of the speaker (its buffer + the device), so
/// the tags are held back by it. Measured 0.38s with coreaudio on mpv 0.41.
const LEAD: Duration = Duration::from_millis(380);
const PUNCH: f32 = 12.0; // dB over the band's running average that reads as full
const FLOOR: f32 = -50.0; // dB; quieter reads as silence, so hiss stays hiss
const SHOW: f32 = 0.12; // particles below this height are not drawn

// ⚫ U+26AB dropped from the filled ladder: wcwidth 2 and emoji presentation,
// so it renders as a color emoji that ignores the band color.
const FILLED: [&str; 5] = [".", "·", "•", "●", "⬤"];
const HOLLOW: [&str; 6] = [".", "◦", "｡", "⚬", "○", "◯"];
const FALLBACK: [u8; 2] = [141, 177]; // theme purples when there is no cover

/// The `--af` entry for mpv; channel c{N}/c{N+1} of the merge is the music.
pub fn af() -> String {
    let taps: String = (0..N).map(|i| format!("[s{i}]")).collect();
    let bands: String = BANDS
        .iter()
        .enumerate()
        .map(|(i, f)| format!("[s{i}]bandpass=f={f}:width_type=o:w=1.3[b{i}];"))
        .collect();
    let outs: String = (0..N).map(|i| format!("[b{i}]")).collect();
    format!(
        "@{LABEL}:lavfi=[asplit[m][x];[x]aformat=channel_layouts=mono,asplit={N}{taps};{bands}\
         [m]aformat=channel_layouts=stereo[mm];{outs}[mm]amerge=inputs={},\
         astats=metadata=1:reset=1:measure_perchannel=RMS_level:measure_overall=none,\
         pan=stereo|c0=c{N}|c1=c{}]",
        N + 1,
        N + 1
    )
}

/// dB per band out of an af-metadata reply; "-inf" (silence) parses as such.
fn db(meta: &Value) -> [f32; N] {
    std::array::from_fn(|i| {
        meta[format!("lavfi.astats.{}.RMS_level", i + 1)]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(f32::NEG_INFINITY)
    })
}

/// Approximate RGB of an xterm-256 index (art only ever yields 16..=255).
fn rgb(c: u8) -> [i32; 3] {
    const CUBE: [i32; 6] = [0, 95, 135, 175, 215, 255];
    match c {
        16..=231 => {
            let i = (c - 16) as usize;
            [CUBE[i / 36], CUBE[i / 6 % 6], CUBE[i % 6]]
        }
        232..=255 => [8 + 10 * (c - 232) as i32; 3],
        _ => [128; 3],
    }
}

fn luma(p: [i32; 3]) -> i32 {
    (299 * p[0] + 587 * p[1] + 114 * p[2]) / 1000
}

/// Up to N clearly different cover colors, most-used first, then sorted dim
/// to bright so the rings shade outward. Near-black is skipped: it would
/// vanish on a dark terminal.
fn palette(grid: &Grid) -> Vec<u8> {
    let mut uses: HashMap<u8, usize> = HashMap::new();
    for &(a, b) in grid.iter().flatten() {
        *uses.entry(a).or_default() += 1;
        *uses.entry(b).or_default() += 1;
    }
    let mut by_use: Vec<(u8, usize)> = uses.into_iter().collect();
    by_use.sort_by_key(|&(c, n)| (Reverse(n), c));
    let dist = |a: [i32; 3], b: [i32; 3]| (0..3).map(|i| (a[i] - b[i]).pow(2)).sum::<i32>();
    let mut out: Vec<u8> = Vec::new();
    for (c, _) in by_use {
        let p = rgb(c);
        if out.len() < N && luma(p) >= 60 && out.iter().all(|&o| dist(rgb(o), p) >= 80 * 80) {
            out.push(c);
        }
    }
    out.sort_by_key(|&c| luma(rgb(c)));
    out
}

/// One particle per pane cell.
#[derive(Clone, Copy, Default)]
struct Particle {
    band: usize,
    y: i64,
    x: i64,
    shape: f32, // 1 on its ring's midline, less toward the ring's edges
    h: f32,     // height off the screen, 0..1
    rising: bool,
}

pub struct Visualizer {
    art: Option<PathBuf>,
    colors: Vec<u8>,                    // cover palette, stretched over the bands
    lag: VecDeque<(Instant, [f32; N])>, // dB per band as filtered, oldest first
    avg: [f32; N],                      // running per-band average (dB): the auto-gain
    level: [f32; N],                    // 0..1 per band, as heard now
    parts: Vec<Particle>,
    dims: (i64, i64),
    last: Instant,
    rng: u32,
    live: bool,
}

impl Visualizer {
    pub fn new() -> Visualizer {
        Visualizer {
            art: None,
            colors: FALLBACK.to_vec(),
            lag: VecDeque::new(),
            avg: [FLOOR; N],
            level: [0.0; N],
            parts: Vec::new(),
            dims: (0, 0),
            last: Instant::now(),
            rng: 0x9e37_79b9,
            live: false,
        }
    }

    /// Worth animating: playing, or particles still falling. False while
    /// paused/hidden, so the caller can drop back to its idle refresh rate.
    pub fn live(&self) -> bool {
        self.live
    }

    /// Band colors from the cover at `path`. Pass the art pane's own size:
    /// art_grid caches per size, so that is a cache hit. Recomputed only when
    /// the cover changes.
    pub fn set_palette(&mut self, path: Option<&Path>, cols: usize, rows: usize) {
        if self.art.as_deref() == path {
            return;
        }
        self.art = path.map(Path::to_path_buf);
        let pal = path
            .and_then(|p| crate::art::art_grid(p, cols, rows).ok())
            .map_or(Vec::new(), |g| palette(&g));
        self.colors = if pal.is_empty() {
            FALLBACK.to_vec()
        } else {
            pal
        };
    }

    /// Advance one frame. `meta` = mpv's af-metadata reply (None when idle or
    /// between tracks: the rings decay). `paused` freezes everything.
    pub fn step(&mut self, meta: Option<&Value>, paused: bool) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        if paused {
            self.live = false;
            return;
        }
        self.lag
            .push_back((now, meta.map_or([f32::NEG_INFINITY; N], db)));
        while self.lag.len() > 1 && self.lag[1].0 + LEAD <= now {
            self.lag.pop_front();
        }
        if let Some(&(_, db)) = self.lag.front().filter(|f| f.0 + LEAD <= now) {
            // loudness over the band's own recent average, so every band
            // jumps on its own beats however loud it sits in the mix
            let k = 1.0 - (-1.5 * dt).exp();
            for ((avg, level), d) in self.avg.iter_mut().zip(&mut self.level).zip(db) {
                *avg += (d.max(FLOOR) - *avg) * k;
                *level = if d > FLOOR {
                    ((d - *avg) / PUNCH + 0.35).clamp(0.0, 1.0)
                } else {
                    0.0
                };
            }
        }
        let fall = (-6.0 * dt).exp();
        for p in &mut self.parts {
            // xorshift: each particle jumps to its own random share of the band
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 17;
            self.rng ^= self.rng << 5;
            let jitter = 0.5 + 0.5 * (self.rng >> 8) as f32 / (1 << 24) as f32;
            let target = self.level[p.band] * p.shape * jitter;
            (p.h, p.rising) = if target > p.h {
                (target, true)
            } else {
                (p.h * fall, false)
            };
        }
        self.live = meta.is_some() || self.parts.iter().any(|p| p.h >= SHOW);
    }

    /// Draw an h x w pane through `cell(y, x, glyph, xterm-256 fg)`.
    pub fn render(&mut self, h: i64, w: i64, mut cell: impl FnMut(i64, i64, &str, u8)) {
        if self.dims != (h, w) {
            self.layout(h, w);
        }
        for p in &self.parts {
            if p.h < SHOW {
                continue;
            }
            let ladder: &[&str] = if p.rising { &FILLED } else { &HOLLOW };
            let g = ladder[((p.h * ladder.len() as f32) as usize).min(ladder.len() - 1)];
            let color = self.colors[p.band * self.colors.len() / N];
            cell(p.y, p.x, g, color);
        }
    }

    /// Place particles: bands split the largest circle that fits into rings,
    /// each ring's particles sit along its midline, and the last band also
    /// scatters over everything outside it, out to the edges and corners.
    fn layout(&mut self, h: i64, w: i64) {
        self.dims = (h, w);
        let aspect = crate::tui::CELL_ASPECT;
        let rmax = (w as f64 / 2.0).min(h as f64 / 2.0 * aspect).max(1.0);
        self.parts = (0..h.max(0) * w.max(0))
            .filter_map(|i| {
                let (y, x) = (i / w, i % w);
                let dx = x as f64 + 0.5 - w as f64 / 2.0;
                let dy = (y as f64 + 0.5 - h as f64 / 2.0) * aspect;
                let ring = (dx * dx + dy * dy).sqrt() / rmax * N as f64;
                let band = (ring as usize).min(N - 1);
                let off = (ring - band as f64 - 0.5).abs();
                let outer = band == N - 1 && ring > band as f64 + 0.5 && (x * 7 + y * 13) % 5 < 2;
                (off < 0.3 || outer).then(|| Particle {
                    y,
                    x,
                    band,
                    shape: 1.0 - off.min(0.5) as f32,
                    ..Default::default()
                })
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// A 150Hz sine through the real filter graph must peak in band 1: the
    /// graph builds, and the channel indexing and tag parsing line up.
    #[test]
    fn sine_lands_in_its_band() {
        if !crate::on_path("ffmpeg") {
            return;
        }
        let g = af();
        let graph = &g[g.find("lavfi=[").unwrap() + "lavfi=[".len()..g.len() - 1];
        let r = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "sine=f=150:d=0.5"])
            .args([
                "-af",
                &format!("{graph},ametadata=mode=print:file=/dev/stdout"),
            ])
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        // last frame's tags, as mpv would hand them over
        let mut meta = serde_json::Map::new();
        for line in String::from_utf8_lossy(&r.stdout).lines() {
            if let Some((k, v)) = line.split_once('=') {
                meta.insert(k.into(), v.into());
            }
        }
        let db = db(&Value::Object(meta));
        let loudest = (0..N).max_by(|&a, &b| db[a].total_cmp(&db[b])).unwrap();
        assert_eq!(loudest, 1, "{db:?}");
    }
}
