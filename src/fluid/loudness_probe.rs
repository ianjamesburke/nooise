//! Output loudness of the built-in songs, measured through the whole engine:
//! an ITU-R BS.1770-4 integrated-loudness meter, an ignored per-song table,
//! and regression bounds on the Master output stage.
//! `cargo test --release loudness_probe -- --ignored --nocapture`

use super::*;
use crate::fx::limiter::CEILING;

const RATE: f32 = 48_000.0;
/// Start-up (fades, envelopes, first phrase) is left out of every measurement.
const SKIP_SECONDS: f32 = 10.0;
const MEASURE_SECONDS: f32 = 45.0;

/// One second-order section in direct form I.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self {
            b,
            a,
            ..Self::default()
        }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// BS.1770-4 K-weighting at 48 kHz (the coefficients the recommendation
/// tabulates), 400 ms blocks on a 100 ms hop, -70 LUFS absolute and -10 LU
/// relative gates. Stereo, both channels weighted 1.0.
struct LoudnessMeter {
    filters: [[Biquad; 2]; 2],
    hop_energy: [f64; 2],
    hop_fill: usize,
    hops: Vec<f64>,
    peak: f32,
    energy: f64,
    frames: u64,
}

const HOP: usize = (RATE as usize) / 10;

impl LoudnessMeter {
    fn new() -> Self {
        let stage = || {
            [
                Biquad::new(
                    [
                        1.535_124_859_586_97,
                        -2.691_696_189_406_38,
                        1.198_392_810_852_85,
                    ],
                    [-1.690_659_293_182_41, 0.732_480_774_215_85],
                ),
                Biquad::new(
                    [1.0, -2.0, 1.0],
                    [-1.990_047_454_833_98, 0.990_072_250_366_21],
                ),
            ]
        };
        Self {
            filters: [stage(), stage()],
            hop_energy: [0.0; 2],
            hop_fill: 0,
            hops: Vec::new(),
            peak: 0.0,
            energy: 0.0,
            frames: 0,
        }
    }

    fn push(&mut self, l: f32, r: f32) {
        for (channel, x) in [l, r].into_iter().enumerate() {
            let [shelf, high_pass] = &mut self.filters[channel];
            let k = high_pass.process(shelf.process(f64::from(x)));
            self.hop_energy[channel] += k * k;
            self.peak = self.peak.max(x.abs());
            self.energy += f64::from(x * x);
        }
        self.frames += 1;
        self.hop_fill += 1;
        if self.hop_fill == HOP {
            self.hops.push(self.hop_energy[0] + self.hop_energy[1]);
            self.hop_energy = [0.0; 2];
            self.hop_fill = 0;
        }
    }

    /// Gated integrated loudness in LUFS.
    fn integrated(&self) -> f32 {
        let blocks: Vec<f64> = self
            .hops
            .windows(4)
            .map(|w| w.iter().sum::<f64>() / (4 * HOP) as f64)
            .collect();
        let lufs = |z: f64| -0.691 + 10.0 * z.log10();
        let mean = |gate: f64| {
            let kept: Vec<f64> = blocks.iter().copied().filter(|z| lufs(*z) > gate).collect();
            kept.iter().sum::<f64>() / kept.len().max(1) as f64
        };
        let relative = lufs(mean(-70.0)) - 10.0;
        lufs(mean(relative.max(-70.0))) as f32
    }

    fn peak_db(&self) -> f32 {
        20.0 * self.peak.max(1e-9).log10()
    }

    /// Unweighted stereo RMS in dBFS (mean of both channels' power).
    fn rms_db(&self) -> f32 {
        10.0 * (self.energy / (2 * self.frames.max(1)) as f64)
            .max(1e-18)
            .log10() as f32
    }
}

/// A full-scale 997 Hz sine in one channel reads -3.01 LUFS (BS.1770-4,
/// Annex 1 note); the same sine at -20 dBFS in both channels reads -20.0.
#[test]
fn loudness_meter_matches_the_recommendation_calibration() {
    let sine = |amplitude: f32, both: bool| {
        let mut meter = LoudnessMeter::new();
        for n in 0..(5.0 * RATE) as usize {
            let x = amplitude * (std::f32::consts::TAU * 997.0 * n as f32 / RATE).sin();
            meter.push(x, if both { x } else { 0.0 });
        }
        meter.integrated()
    };
    let one = sine(1.0, false);
    assert!((one + 3.01).abs() < 0.05, "full-scale mono sine read {one}");
    let both = sine(0.1, true);
    assert!(
        (both + 20.0).abs() < 0.1,
        "-20 dBFS stereo sine read {both}"
    );
}

struct Measured {
    meter: LoudnessMeter,
    /// Samples past the output ceiling.
    over: u32,
}

/// Render `song` through the engine and meter its output after start-up.
/// With `wav`, the measured span is also written as 32-bit float.
fn measure(song: &SongState, held: &[GestureKind], wav: Option<&std::path::Path>) -> Measured {
    let mut snapshot = LiveSessionSnapshot::from_song(song);
    snapshot.gestures = GestureState::default();
    for kind in held {
        snapshot.gestures.lanes[*kind as usize] = GestureEnvelope {
            amount: 1.0,
            at_seconds: 0.0,
            held: true,
            restored: false,
        };
    }
    let mut engine = FluidEngine::new(
        RATE,
        LiveSession::new(snapshot),
        no_morph(),
        Arc::new(FluidTelemetry::default()),
    );
    engine.reseed(7);
    let mut meter = LoudnessMeter::new();
    let mut over = 0;
    let mut writer = wav.map(|path| {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: RATE as u32,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        hound::WavWriter::create(path, spec).expect("create probe wav")
    });
    for frame in 0..((SKIP_SECONDS + MEASURE_SECONDS) * RATE) as usize {
        let (l, r) = engine.next_stereo();
        if (frame as f32) < SKIP_SECONDS * RATE {
            continue;
        }
        over += u32::from(l.abs() > CEILING) + u32::from(r.abs() > CEILING);
        meter.push(l, r);
        if let Some(writer) = &mut writer {
            writer.write_sample(l).expect("write probe wav");
            writer.write_sample(r).expect("write probe wav");
        }
    }
    if let Some(writer) = writer {
        writer.finalize().expect("finalize probe wav");
    }
    Measured { meter, over }
}

/// Every built-in song, 1-based as `nooise N` numbers it, with Master Level
/// as authored and at 100%.
fn songs_at_both_levels() -> Vec<(usize, f32, SongState, SongState)> {
    decode_auto_states()
        .into_iter()
        .enumerate()
        .map(|(index, authored)| {
            let mut full = authored.clone();
            full.controls.master.level = 1.0;
            (index + 1, authored.controls.master.level, authored, full)
        })
        .collect()
}

#[test]
#[ignore = "diagnostic render, prints a loudness table"]
fn loudness_probe() {
    // Set to a directory to keep each authored render for listening or an
    // independent check (`ffmpeg -i N.wav -af ebur128 -f null -`).
    let wav_dir = std::env::var_os("NOOISE_LOUDNESS_WAV_DIR").map(std::path::PathBuf::from);
    let rows = std::thread::scope(|scope| {
        let handles: Vec<_> = songs_at_both_levels()
            .into_iter()
            .map(|(number, level, authored, full)| {
                let wav = wav_dir
                    .as_ref()
                    .map(|dir| dir.join(format!("{number}.wav")));
                scope.spawn(move || {
                    let a = measure(&authored, &[], wav.as_deref());
                    let f = measure(&full, &[], None);
                    (number, level, a, f)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("render thread"))
            .collect::<Vec<_>>()
    });
    println!(
        "song level | authored: LUFS  peak dBFS crest dB   over | at 100%: LUFS  peak dBFS crest dB   over"
    );
    for (number, level, a, f) in &rows {
        let row = |m: &Measured| {
            format!(
                "{:6.1} {:9.1} {:8.1} {:6}",
                m.meter.integrated(),
                m.meter.peak_db(),
                m.meter.peak_db() - m.meter.rms_db(),
                m.over
            )
        };
        println!(
            "{number:4} {:4.0}% | {} | {}",
            level * 100.0,
            row(a),
            row(f)
        );
    }
    let spread = |pick: fn(&(usize, f32, Measured, Measured)) -> f32| {
        let values: Vec<f32> = rows.iter().map(pick).collect();
        let lo = values.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut sorted = values.clone();
        sorted.sort_by(f32::total_cmp);
        (lo, sorted[sorted.len() / 2], hi)
    };
    let (lo, median, hi) = spread(|row| row.2.meter.integrated());
    println!("authored LUFS: min {lo:.1} median {median:.1} max {hi:.1}");
    let (lo, median, hi) = spread(|row| row.3.meter.integrated());
    println!("100% LUFS:     min {lo:.1} median {median:.1} max {hi:.1}");
}
