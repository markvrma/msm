//! Audio visualizer: circles made of one ring of particles per frequency
//! band, bass at the center and highs outward (a speaker cone, not a target).
//! The FOR YOU pane gets one circle, the largest that fits, whose rings then
//! repeat outward (bands cycling 0, 1, 2, ...) to the pane edges and corners. The full-screen one (`multi`) gets
//! overlapping circles of random size and place that together cover the
//! screen, all reading the same levels: a kick pulses the core of every one of
//! them at once. A particle climbs a glyph
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
    multi: bool, // overlapping random circles instead of one
}

impl Visualizer {
    pub fn new(multi: bool) -> Visualizer {
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
            multi,
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
        self.dims = (0, 0); // new cover, new song: deal a fresh arrangement
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
            // each particle jumps to its own random share of the band
            let jitter = 0.5 + 0.5 * xorshift(&mut self.rng);
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
        // multi circles overlap: per cell, the particle standing highest is drawn
        let mut top: Vec<Option<&Particle>> = vec![None; (h.max(0) * w.max(0)) as usize];
        for p in &self.parts {
            let t = &mut top[(p.y * w + p.x) as usize];
            if p.h >= SHOW && t.is_none_or(|t| p.h > t.h) {
                *t = Some(p);
            }
        }
        for p in top.into_iter().flatten() {
            let ladder: &[&str] = if p.rising { &FILLED } else { &HOLLOW };
            let g = ladder[((p.h * ladder.len() as f32) as usize).min(ladder.len() - 1)];
            let color = self.colors[p.band * self.colors.len() / N];
            cell(p.y, p.x, g, color);
        }
    }

    /// Place particles: every circle splits into one ring per band, and each
    /// ring's particles sit along its midline. A single circle is the largest
    /// that fits, and past its sixth ring the rings keep going, same width,
    /// out to the corners, the bands cycling round again.
    fn layout(&mut self, h: i64, w: i64) {
        self.dims = (h, w);
        if !self.multi {
            let aspect = crate::tui::CELL_ASPECT;
            let rmax = (w as f64 / 2.0).min(h as f64 / 2.0 * aspect).max(1.0);
            self.parts = (0..h.max(0) * w.max(0))
                .filter_map(|i| {
                    let (y, x) = (i / w, i % w);
                    let dx = x as f64 + 0.5 - w as f64 / 2.0;
                    let dy = (y as f64 + 0.5 - h as f64 / 2.0) * aspect;
                    let ring = (dx * dx + dy * dy).sqrt() / rmax * N as f64;
                    let off = (ring - (ring as usize) as f64 - 0.5).abs();
                    (off < 0.3).then(|| Particle {
                        y,
                        x,
                        band: ring as usize % N,
                        shape: 1.0 - off as f32,
                        ..Default::default()
                    })
                })
                .collect();
            return;
        }
        self.parts.clear();
        for (cx, cy, r) in circles(h, w, &mut self.rng) {
            let (y0, y1) = (((cy - r) / ASPECT) as i64, ((cy + r) / ASPECT) as i64);
            let (x0, x1) = ((cx - r) as i64, (cx + r) as i64);
            for y in y0.max(0)..=y1.min(h - 1) {
                for x in x0.max(0)..=x1.min(w - 1) {
                    let ring = dist(x, y, cx, cy) / r * N as f64;
                    let band = ring as usize;
                    let off = (ring - band as f64 - 0.5).abs();
                    if band < N && off < 0.3 {
                        self.parts.push(Particle {
                            y,
                            x,
                            band,
                            shape: 1.0 - off as f32,
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }
}

const ASPECT: f64 = crate::tui::CELL_ASPECT;

/// Distance from cell (x, y) to a point, in square units: a row is ASPECT
/// columns tall, so circles come out round.
fn dist(x: i64, y: i64, cx: f64, cy: f64) -> f64 {
    let (dx, dy) = (x as f64 + 0.5 - cx, (y as f64 + 0.5) * ASPECT - cy);
    (dx * dx + dy * dy).sqrt()
}

/// Random circles (cx, cy, r in square units) until every cell of an h x w
/// pane lies inside one. Radii run from a fifth to half the pane's shorter
/// side (never under 6 columns, so six rings still fit), so the count grows
/// with the pane. Each circle is centered near a random still-uncovered
/// cell -- off by at most r/2 per axis, so that cell is always covered and
/// the loop always ends.
fn circles(h: i64, w: i64, rng: &mut u32) -> Vec<(f64, f64, f64)> {
    let side = (w as f64).min(h as f64 * ASPECT);
    let (rmin, rmax) = ((side / 5.0).max(6.0), (side / 2.0).max(6.0));
    let mut covered = vec![false; (h.max(0) * w.max(0)) as usize];
    let mut out = Vec::new();
    loop {
        let open = covered.iter().filter(|&&c| !c).count();
        if open == 0 {
            return out;
        }
        let pick = (xorshift(rng) * open as f32) as usize % open;
        let i = covered
            .iter()
            .enumerate()
            .filter(|c| !c.1)
            .nth(pick)
            .unwrap()
            .0 as i64;
        let r = rmin + (rmax - rmin) * xorshift(rng) as f64;
        let mut jog = || (xorshift(rng) as f64 - 0.5) * r;
        let cx = (i % w) as f64 + 0.5 + jog();
        let cy = ((i / w) as f64 + 0.5) * ASPECT + jog();
        for (j, c) in covered.iter_mut().enumerate() {
            let j = j as i64;
            *c |= dist(j % w, j / w, cx, cy) < r;
        }
        out.push((cx, cy, r));
    }
}

/// Uniform 0..1 off a xorshift32 state.
fn xorshift(s: &mut u32) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    (*s >> 8) as f32 / (1 << 24) as f32
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

    /// The pane's single circle reaches every corner: its rings repeat past
    /// the sixth, bands cycling.
    #[test]
    fn single_circle_reaches_the_corners() {
        let (h, w) = (24, 80);
        let mut v = Visualizer::new(false);
        v.layout(h, w);
        assert!(
            v.parts.iter().any(|p| p.y == 0 && p.band < 3),
            "no repeated rings"
        );
        for (cy, cx) in [(0, 0), (0, w - 2), (h - 2, 0), (h - 2, w - 2)] {
            let near = |p: &&Particle| (cy..cy + 2).contains(&p.y) && (cx..cx + 2).contains(&p.x);
            assert!(
                v.parts.iter().any(|p| near(&p)),
                "corner ({cy}, {cx}) empty"
            );
        }
    }

    #[test]
    fn circles_cover_every_cell() {
        let (h, w) = (24, 80);
        let cs = circles(h, w, &mut 0x9e37_79b9);
        assert!(cs.len() > 1, "{cs:?}");
        for (y, x) in (0..h).flat_map(|y| (0..w).map(move |x| (y, x))) {
            assert!(
                cs.iter().any(|&(cx, cy, r)| dist(x, y, cx, cy) < r),
                "({y}, {x}) uncovered"
            );
        }
    }
}
