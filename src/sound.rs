//! Audio cues, synthesized in code (no sound files).
//!
//! Everything is dry and percussive, like small objects being placed: short sine "bodies" with a
//! fast pitch drop, band-passed noise for the tactile edge, and low thuds for weight. No melodies
//! and no reverb tails. Each block landing in the progress grid gets a slightly different tick so
//! rapid sequences never sound mechanical, and is panned to where the block lands.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};

const SR: u32 = 48_000;
/// Number of variations of the block tick.
pub const BLOCK_VARIANTS: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cue {
    /// Compression begins: two quick dry taps.
    Start,
    /// A block lands in the progress grid. (variant, pan -2..=2)
    Block(u8, i8),
    /// A block completes a column: a fuller "tock". (pan -2..=2)
    Column(i8),
    /// Came out too big; blocks fall away.
    Retry,
    /// The file is built: a scan across the grid, one rising click per column, then a snap.
    /// (number of columns)
    Finish(u8),
    /// Nothing needed building (the file was already small enough).
    Done,
    /// Installed or updated from the settings window: a short rising run.
    Installed,
    Error,
    Cancel,
    /// File copied to the clipboard.
    Copy,
    Save,
    ToggleOn,
    ToggleOff,
}

static ENABLED: AtomicBool = AtomicBool::new(true);
static SENDER: OnceLock<Sender<Cue>> = OnceLock::new();

/// Opens the audio device on a background thread. Safe to call more than once.
pub fn init(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cue>();
        std::thread::spawn(move || {
            // No audio device: silently drop cues.
            let Ok(sink) = rodio::DeviceSinkBuilder::open_default_sink() else {
                for _ in rx {}
                return;
            };
            let mut cache: HashMap<Cue, Vec<f32>> = HashMap::new();
            for cue in rx {
                let samples = cache.entry(cue).or_insert_with(|| render(cue)).clone();
                sink.mixer().add(rodio::buffer::SamplesBuffer::new(rodio::nz!(2), rodio::nz!(48_000), samples));
            }
        });
        tx
    });
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn play(cue: Cue) {
    if ENABLED.load(Ordering::Relaxed)
        && let Some(tx) = SENDER.get()
    {
        let _ = tx.send(cue);
    }
}

/// Maps a horizontal position (-1 = left, 1 = right) to one of the five pan buckets.
pub fn pan_bucket(x: f32) -> i8 {
    (x.clamp(-1.0, 1.0) * 2.0).round() as i8
}

/// Time between columns in the finishing scan, so the whole scan takes about 0.6 s.
/// Shared by the sound and the animation so they stay in sync.
pub fn scan_step(cols: usize) -> f32 {
    (0.6 / cols.max(1) as f32).clamp(0.012, 0.035)
}

/// Every cue in a little sequence (`comprimo sounds`), for previewing.
pub fn demo_sequence() -> Vec<(Cue, u64)> {
    let mut seq = vec![(Cue::Start, 350)];
    for i in 0..16u8 {
        let pan = pan_bucket(1.0 - i as f32 / 8.0);
        let cue = if i % 4 == 3 { Cue::Column(pan) } else { Cue::Block(i % BLOCK_VARIANTS, pan) };
        seq.push((cue, 110));
    }
    seq.push((Cue::Retry, 600));
    for i in 0..8u8 {
        let cue = if i % 4 == 3 { Cue::Column(-1) } else { Cue::Block(i, -1) };
        seq.push((cue, 45));
    }
    seq.extend([
        (Cue::Finish(24), 1300),
        (Cue::Done, 1000),
        (Cue::Installed, 900),
        (Cue::Copy, 700),
        (Cue::Save, 700),
        (Cue::ToggleOn, 500),
        (Cue::ToggleOff, 700),
        (Cue::Error, 900),
        (Cue::Cancel, 900),
    ]);
    seq
}

pub fn demo() {
    init(true);
    for (cue, wait) in demo_sequence() {
        play(cue);
        std::thread::sleep(std::time::Duration::from_millis(wait));
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
}

// ---------------------------------------------------------------------------------------------
// Synthesis
// ---------------------------------------------------------------------------------------------

struct Mix {
    l: Vec<f32>,
    r: Vec<f32>,
}

/// Small deterministic random numbers in -1..1, so variants are stable between runs.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
    (angle.cos(), angle.sin())
}

impl Mix {
    fn new(secs: f32) -> Self {
        let len = (secs * SR as f32) as usize;
        Mix { l: vec![0.0; len], r: vec![0.0; len] }
    }

    fn put(&mut self, i: usize, v: f32, (gl, gr): (f32, f32)) {
        if i < self.l.len() {
            self.l[i] += v * gl;
            self.r[i] += v * gr;
        }
    }

    /// A sine "body" that drops slightly in pitch and dies fast. `drop` = relative pitch drop.
    fn body(&mut self, at: f32, freq: f32, decay: f32, amp: f32, drop: f32, pan: f32) {
        let start = (at * SR as f32) as usize;
        let len = (decay * 6.0 * SR as f32) as usize;
        let gains = pan_gains(pan);
        let mut phase = 0.0f32;
        for i in 0..len {
            let t = i as f32 / SR as f32;
            let f = freq * (1.0 + drop * (-t / 0.004).exp());
            phase = (phase + TAU * f / SR as f32) % TAU;
            let attack = (t / 0.0004).min(1.0);
            let tail = ((len - i) as f32 / (SR as f32 * 0.003)).min(1.0);
            self.put(start + i, phase.sin() * (-t / decay).exp() * attack * tail * amp, gains);
        }
    }

    /// Band-passed noise burst: the tactile "edge" of a click.
    #[allow(clippy::too_many_arguments)]
    fn noise(&mut self, at: f32, center: f32, q: f32, attack: f32, decay: f32, amp: f32, pan: f32, seed: u32) {
        let start = (at * SR as f32) as usize;
        let len = ((attack + decay * 6.0) * SR as f32) as usize;
        let gains = pan_gains(pan);
        // RBJ band-pass (0 dB peak).
        let w0 = TAU * center / SR as f32;
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b2, a1, a2) = (alpha / a0, -alpha / a0, -2.0 * w0.cos() / a0, (1.0 - alpha) / a0);
        let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut rng = Rng(seed.max(1));
        for i in 0..len {
            let t = i as f32 / SR as f32;
            let x = rng.next();
            let y = b0 * x + b2 * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, x, y1, y);
            let env = if t < attack { t / attack.max(1e-4) } else { (-(t - attack) / decay).exp() };
            let tail = ((len - i) as f32 / (SR as f32 * 0.003)).min(1.0);
            self.put(start + i, y * env * tail * amp, gains);
        }
    }

    /// Two very short early reflections: a sense of a surface, without any tail.
    fn surface(&mut self) {
        for (ch, delay) in [(&mut self.l, 0.0067f32), (&mut self.r, 0.0093)] {
            let d = (delay * SR as f32) as usize;
            let dry = ch.clone();
            for i in d..ch.len() {
                ch[i] += dry[i - d] * 0.14;
            }
        }
    }

    fn finish(mut self) -> Vec<f32> {
        self.surface();
        self.finish_dry()
    }

    /// Without the surface reflections: for the progress taps, which should be as dry as possible.
    fn finish_dry(self) -> Vec<f32> {
        let len = self.l.len();
        let fade = (SR as usize / 200).min(len);
        let mut out = Vec::with_capacity(len * 2);
        for i in 0..len {
            let g = if i + fade >= len { (len - i) as f32 / fade as f32 } else { 1.0 };
            out.push(self.l[i].tanh() * g);
            out.push(self.r[i].tanh() * g);
        }
        out
    }
}

fn render(cue: Cue) -> Vec<f32> {
    match cue {
        Cue::Block(variant, pan) => {
            // Kept deliberately minimal: one short, dry tap with a little natural variation.
            let mut r = Rng(0x9E37_79B9 ^ (variant as u32 + 1).wrapping_mul(2_654_435_761));
            let pan = pan as f32 * 0.3;
            let (pitch, level) = (1.0 + 0.06 * r.next(), 1.0 + 0.12 * r.next());
            let mut m = Mix::new(0.025);
            m.noise(0.0, 3800.0 * pitch, 1.0, 0.0002, 0.0012, 0.30 * level, pan, variant as u32 + 7);
            m.body(0.0, 1900.0 * pitch, 0.0025, 0.12 * level, 0.05, pan);
            m.finish_dry()
        }
        Cue::Column(pan) => {
            let pan = pan as f32 * 0.3;
            let mut m = Mix::new(0.03);
            m.noise(0.0, 2600.0, 0.9, 0.0002, 0.0018, 0.30, pan, 91);
            m.body(0.0, 1250.0, 0.004, 0.16, 0.06, pan);
            m.finish_dry()
        }
        Cue::Start => {
            // Two dry taps in quick succession, like the progress taps: "t-tk".
            let mut m = Mix::new(0.09);
            m.noise(0.0, 3000.0, 1.0, 0.0002, 0.0014, 0.26, -0.25, 11);
            m.body(0.0, 1500.0, 0.003, 0.13, 0.05, -0.25);
            m.noise(0.034, 3900.0, 1.1, 0.0002, 0.0014, 0.30, 0.25, 13);
            m.body(0.034, 1950.0, 0.003, 0.14, 0.05, 0.25);
            m.finish_dry()
        }
        Cue::Installed => {
            // A short version of the finishing scan: a quick rising run that clicks into place.
            let (n, step) = (7, 0.026f32);
            let end = n as f32 * step;
            let mut m = Mix::new(end + 0.1);
            for k in 0..n {
                let x = k as f32 / (n - 1) as f32;
                let at = k as f32 * step;
                let pitch = 1400.0 * 2f32.powf(x);
                let pan = -0.4 + 0.8 * x;
                let level = 0.10 + 0.07 * x;
                m.body(at, pitch, 0.004, level, 0.06, pan);
                m.noise(at, pitch * 2.1, 1.3, 0.0003, 0.0018, level * 0.9, pan, 200 + k);
            }
            m.body(end, 3300.0, 0.006, 0.20, 0.06, 0.4);
            m.noise(end, 6200.0, 1.4, 0.0002, 0.0022, 0.17, 0.4, 199);
            m.finish()
        }
        Cue::Retry => {
            let mut m = Mix::new(0.16);
            for (i, f) in [1900.0f32, 1550.0, 1250.0].into_iter().enumerate() {
                let at = i as f32 * 0.032;
                let pan = 0.3 - i as f32 * 0.3;
                m.body(at, f, 0.005, 0.15, 0.08, pan);
                m.noise(at, f * 2.0, 1.2, 0.0003, 0.002, 0.12, pan, 21 + i as u32);
            }
            m.finish()
        }
        Cue::Finish(cols) => {
            let cols = (cols as usize).max(4);
            let step = scan_step(cols);
            let end = cols as f32 * step;
            let mut m = Mix::new(end + 0.15);
            for k in 0..cols {
                let x = k as f32 / (cols - 1) as f32;
                let at = k as f32 * step;
                // Rises a bit over an octave, sweeps left → right, and grows slightly louder.
                let pitch = 1300.0 * 2f32.powf(x * 1.25);
                let pan = -0.6 + 1.2 * x;
                let level = 0.11 + 0.08 * x;
                m.body(at, pitch, 0.005, level, 0.06, pan);
                m.noise(at, pitch * 2.1, 1.3, 0.0003, 0.002, level * 0.9, pan, 100 + k as u32);
            }
            // The last piece clicks into place.
            m.body(end, 3300.0, 0.007, 0.22, 0.06, 0.6);
            m.body(end, 140.0, 0.030, 0.22, 0.6, 0.3);
            m.noise(end, 6000.0, 1.4, 0.0002, 0.0025, 0.18, 0.6, 99);
            m.finish()
        }
        Cue::Done => {
            let mut m = Mix::new(0.36);
            // Thunk: weight and wood.
            m.body(0.0, 108.0, 0.045, 0.34, 0.8, 0.0);
            m.body(0.0, 860.0, 0.012, 0.16, 0.12, 0.0);
            m.noise(0.0, 2300.0, 0.8, 0.0004, 0.006, 0.16, 0.0, 31);
            // Air underneath, like a lid closing.
            m.noise(0.0, 6500.0, 0.7, 0.035, 0.05, 0.035, 0.0, 33);
            // Snap: the latch catching.
            m.body(0.085, 2650.0, 0.006, 0.20, 0.06, 0.15);
            m.noise(0.085, 6000.0, 1.4, 0.0002, 0.002, 0.18, 0.15, 35);
            m.finish()
        }
        Cue::Error => {
            let mut m = Mix::new(0.28);
            m.body(0.0, 140.0, 0.030, 0.19, 0.5, 0.1);
            m.body(0.0, 520.0, 0.012, 0.065, 0.1, 0.1);
            m.body(0.12, 112.0, 0.040, 0.19, 0.5, -0.1);
            m.body(0.12, 420.0, 0.014, 0.065, 0.1, -0.1);
            m.finish()
        }
        Cue::Cancel => {
            let mut m = Mix::new(0.2);
            m.body(0.0, 130.0, 0.035, 0.24, 0.9, 0.0);
            m.noise(0.0, 1200.0, 0.7, 0.002, 0.02, 0.07, 0.0, 41);
            m.finish()
        }
        Cue::Copy => {
            let mut m = Mix::new(0.12);
            m.body(0.0, 3000.0, 0.004, 0.15, 0.06, 0.2);
            m.noise(0.0, 6000.0, 1.4, 0.0002, 0.0018, 0.12, 0.2, 51);
            m.body(0.045, 2350.0, 0.005, 0.15, 0.06, -0.1);
            m.noise(0.045, 5000.0, 1.4, 0.0002, 0.002, 0.10, -0.1, 53);
            m.finish()
        }
        Cue::Save => {
            let mut m = Mix::new(0.08);
            m.body(0.0, 2200.0, 0.006, 0.20, 0.08, 0.0);
            m.body(0.0, 170.0, 0.016, 0.16, 0.6, 0.0);
            m.noise(0.0, 4500.0, 1.2, 0.0003, 0.0025, 0.16, 0.0, 61);
            m.finish()
        }
        Cue::ToggleOn => {
            let mut m = Mix::new(0.06);
            m.body(0.0, 2400.0, 0.005, 0.18, 0.08, 0.0);
            m.noise(0.0, 5200.0, 1.3, 0.0002, 0.002, 0.14, 0.0, 71);
            m.finish()
        }
        Cue::ToggleOff => {
            let mut m = Mix::new(0.07);
            m.body(0.0, 1500.0, 0.006, 0.17, 0.08, 0.0);
            m.body(0.0, 160.0, 0.014, 0.12, 0.6, 0.0);
            m.noise(0.0, 3500.0, 1.2, 0.0002, 0.0025, 0.12, 0.0, 73);
            m.finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_cues() -> Vec<Cue> {
        let mut cues: Vec<Cue> = demo_sequence().into_iter().map(|c| c.0).collect();
        cues.extend((0..BLOCK_VARIANTS).map(|v| Cue::Block(v, 0)));
        cues.extend([Cue::Finish(4), Cue::Finish(60)]);
        cues
    }

    #[test]
    fn cues_are_short_audible_and_never_clip() {
        for cue in all_cues() {
            let s = render(cue);
            let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.05 && peak < 0.9, "{cue:?} peak {peak}");
            let limit = if matches!(cue, Cue::Finish(_)) { SR as usize } else { SR as usize * 4 / 10 };
            assert!(s.len() / 2 <= limit, "{cue:?} is too long");
            assert!(s.iter().rev().take(4).all(|v| v.abs() < 1e-3), "{cue:?} ends with a click");
        }
    }

    #[test]
    fn block_variants_differ() {
        let a = render(Cue::Block(0, 0));
        let b = render(Cue::Block(1, 0));
        assert_ne!(a, b);
    }

    /// Writes every cue to %TEMP%\comprimo-sounds (plus the demo sequence as one file):
    /// `cargo test --release write_wavs -- --ignored`
    #[test]
    #[ignore]
    fn write_wavs() {
        let dir = std::env::temp_dir().join("comprimo-sounds");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, s: &[f32]| {
            let mut wav = Vec::new();
            let data_len = (s.len() * 2) as u32;
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + data_len).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16u32.to_le_bytes());
            wav.extend_from_slice(&1u16.to_le_bytes());
            wav.extend_from_slice(&2u16.to_le_bytes());
            wav.extend_from_slice(&SR.to_le_bytes());
            wav.extend_from_slice(&(SR * 4).to_le_bytes());
            wav.extend_from_slice(&4u16.to_le_bytes());
            wav.extend_from_slice(&16u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&data_len.to_le_bytes());
            for v in s {
                wav.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
            }
            std::fs::write(dir.join(format!("{name}.wav")), wav).unwrap();
        };
        let mut sequence: Vec<f32> = Vec::new();
        let mut cursor = 0usize;
        for (cue, wait) in demo_sequence() {
            let s = render(cue);
            write(&format!("{cue:?}").replace(['(', ')', ',', ' ', '-'], "_"), &s);
            if sequence.len() < cursor + s.len() {
                sequence.resize(cursor + s.len(), 0.0);
            }
            for (i, v) in s.iter().enumerate() {
                sequence[cursor + i] += v;
            }
            cursor += wait as usize * SR as usize / 1000 * 2;
        }
        write("_sequence", &sequence);
    }
}
