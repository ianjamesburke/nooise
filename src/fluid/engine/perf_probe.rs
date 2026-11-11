//! Ignored audio-thread cost probe: per-buffer render time for ordinary and
//! bar-crossing buffers, steady CPU share, and heap traffic under UI edits.
//! `cargo test --release engine_perf_probe -- --ignored --nocapture --test-threads=1`
//! Timings are wall-clock on a shared machine; record `uptime` alongside them.

use super::*;
use crate::alloc_count::{HeapTraffic, count};
use crate::fluid::auto::decode_auto_states;
use std::time::Instant;

const RATE: f32 = 44_100.0;
const BUF: usize = 256;
const SECONDS: f64 = 20.0;

fn engine_for(song: &SongState) -> (LiveSession, FluidEngine) {
    let session = LiveSession::new(LiveSessionSnapshot::from_song(song));
    let engine = FluidEngine::new(
        RATE,
        session.clone(),
        no_morph(),
        Arc::new(FluidTelemetry::default()),
    );
    (session, engine)
}

#[derive(Default)]
struct Stats {
    ordinary: Vec<u64>,
    bar: Vec<u64>,
    heap: HeapTraffic,
}

fn run(
    engine: &mut FluidEngine,
    buffers: usize,
    mut between: impl FnMut(usize, &mut FluidEngine),
) -> Stats {
    let mut stats = Stats {
        ordinary: Vec::with_capacity(buffers),
        ..Stats::default()
    };
    for b in 0..buffers {
        between(b, engine);
        let bar_before = (engine.tempo.beat / 4.0).floor();
        let (dt, heap) = count(|| {
            let t = Instant::now();
            let mut acc = 0.0f32;
            for _ in 0..BUF {
                let (l, r) = engine.next_stereo();
                acc += l + r;
            }
            std::hint::black_box(acc);
            t.elapsed().as_nanos() as u64
        });
        stats.heap += heap;
        if (engine.tempo.beat / 4.0).floor() != bar_before {
            stats.bar.push(dt);
        } else {
            stats.ordinary.push(dt);
        }
    }
    stats
}

fn mean_us(v: &[u64]) -> f64 {
    v.iter().sum::<u64>() as f64 / v.len().max(1) as f64 / 1000.0
}

fn pct_us(v: &mut [u64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * p) as usize] as f64 / 1000.0
}

fn report(name: &str, mut s: Stats) {
    let budget = BUF as f64 / f64::from(RATE) * 1e6;
    let mean = mean_us(&s.ordinary);
    let bar_mean = mean_us(&s.bar);
    println!(
        "{name:<34} ordinary n={} mean {mean:.0} p99 {:.0} max {:.0} us ({:.1}% of {budget:.0} us) | bar-crossing n={} mean {bar_mean:.0} max {:.0} us | allocs {} frees {}",
        s.ordinary.len(),
        pct_us(&mut s.ordinary, 0.99),
        pct_us(&mut s.ordinary, 1.0),
        mean / budget * 100.0,
        s.bar.len(),
        pct_us(&mut s.bar, 1.0),
        s.heap.allocs,
        s.heap.frees,
    );
}

#[test]
#[ignore = "timing probe; run in release with --nocapture"]
fn engine_perf_probe() {
    let songs = decode_auto_states();
    let buffers = (SECONDS * f64::from(RATE) / BUF as f64) as usize;
    let default_song = SongState::from_controls(FluidControls::default());

    let (_s, mut e) = engine_for(&default_song);
    report("default steady", run(&mut e, buffers, |_, _| {}));
    let mut all = Stats::default();
    for song in &songs {
        let (_s, mut e) = engine_for(song);
        let s = run(&mut e, buffers, |_, _| {});
        all.ordinary.extend(s.ordinary);
        all.bar.extend(s.bar);
        all.heap += s.heap;
    }
    report(&format!("all {} auto songs steady", songs.len()), all);

    // A held knob: the UI publishes an edit about 21 times a second.
    for (label, song) in [("default", &default_song), ("song 3", &songs[2])] {
        let (s, mut e) = engine_for(song);
        report(
            &format!("{label} + UI edit every 8 buffers"),
            run(&mut e, buffers, |b, _| {
                if b % 8 == 0 {
                    s.update(|snap| {
                        snap.controls.master.level = 0.5 + 0.001 * ((b / 8) % 100) as f32;
                    });
                }
            }),
        );
    }

    // Stop the transport a few samples before a bar and stay stopped.
    {
        let (s, mut e) = engine_for(&songs[0]);
        let probe = 64.0 * e.tempo.bpm / (60.0 * f64::from(RATE));
        while 4.0 - e.tempo.beat.rem_euclid(4.0) > probe || e.tempo.beat < 4.0 {
            e.next_stereo();
        }
        s.update(|snap| snap.transport = Transport::Stopped);
        report(
            "song 1 stopped just before a bar",
            run(&mut e, buffers / 4, |_, _| {}),
        );
    }
}
