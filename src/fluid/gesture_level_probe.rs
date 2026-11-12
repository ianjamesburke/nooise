//! Master headroom with gestures held at full amount, measured at real
//! playback level through the whole engine, output stage included: a
//! regression bound and an ignored diagnostic table.
//! `cargo test --release gesture_level_probe -- --ignored --nocapture`

use super::*;
use crate::fx::limiter::{CEILING, Limiter};

const RATE: f32 = 48_000.0;
const SKIP_SECONDS: f32 = 3.0;

struct Level {
    /// Output samples past the -1 dBFS ceiling; the limiter allows none.
    over: u32,
    /// The limiter's deepest gain reduction, in dB (0 = never engaged).
    reduction_db: f32,
    peak: f32,
    /// Energy below 250 Hz, 250 Hz to 2.5 kHz, and above 2.5 kHz.
    bands_db: [f32; 3],
}

fn render(song: &SongState, held: &[GestureKind], seconds: f32) -> Level {
    let mut snapshot = LiveSessionSnapshot::from_song(song);
    snapshot.gestures = GestureState::default();
    for kind in held {
        if *kind == GestureKind::BeatRepeat {
            continue;
        }
        snapshot.gestures.lanes[*kind as usize] = GestureEnvelope {
            amount: 1.0,
            at_seconds: 0.0,
            held: true,
            restored: false,
        };
    }
    let session = LiveSession::new(snapshot);
    let mut engine = FluidEngine::new(
        RATE,
        session.clone(),
        no_morph(),
        Arc::new(FluidTelemetry::default()),
    );
    engine.reseed(7);
    let one_pole = |hz: f32| 1.0 - (-std::f32::consts::TAU * hz / RATE).exp();
    let (a_low, a_high) = (one_pole(250.0), one_pole(2_500.0));
    let (mut lp_low, mut lp_high) = (0.0_f32, 0.0_f32);
    let mut energy = [0.0_f64; 3];
    let (mut over, mut peak, mut least_gain) = (0, 0.0_f32, 1.0_f32);
    for frame in 0..(seconds * RATE) as usize {
        // Repeat real playing history, rather than an empty startup buffer.
        if frame == (2.0 * RATE) as usize && held.contains(&GestureKind::BeatRepeat) {
            session.update(|snapshot| {
                snapshot.gestures.press(GestureKind::BeatRepeat, 2.0);
            });
        }
        let (l, r) = engine.next_stereo();
        if (frame as f32) < SKIP_SECONDS * RATE {
            continue;
        }
        least_gain = least_gain.min(engine.master_bus.limiter.gain());
        for x in [l, r] {
            over += u32::from(x.abs() > CEILING);
            peak = peak.max(x.abs());
        }
        let mono = (l + r) * 0.5;
        lp_low += a_low * (mono - lp_low);
        lp_high += a_high * (mono - lp_high);
        for (band, value) in [lp_low, lp_high - lp_low, mono - lp_high]
            .into_iter()
            .enumerate()
        {
            energy[band] += f64::from(value * value);
        }
    }
    Level {
        over,
        reduction_db: -20.0 * least_gain.log10(),
        peak,
        bands_db: energy.map(|e| 10.0 * e.max(1e-12).log10() as f32),
    }
}

/// Headroom is the gesture stage's job; the output limiter is a ceiling,
/// not gain staging. Every gesture that adds a return, combined with Beat
/// Repeat over populated live history, must keep the stage's input peak
/// under 0.6 before makeup (4 dB clear of the old 0.95 clamp), which is a
/// bound on the limiter's deepest reduction: on the song whose Bloom lifts
/// peaks most (index 5), a song the pre-fix Bloom clipped 14,497 times (9),
/// and the loudest built-in song (18).
#[test]
fn full_gesture_throw_keeps_the_limiter_light() {
    use GestureKind::*;
    let bound_db = 20.0 * (0.6 * MASTER_MAKEUP / CEILING).log10();
    let songs = decode_auto_states();
    for index in [5, 9, 18] {
        let level = render(&songs[index], &[Bloom, Echo, Submerge, BeatRepeat], 9.0);
        assert!(
            level.over == 0 && level.reduction_db < bound_db,
            "song {index}: {} over the ceiling, {:.2} dB reduction (bound {bound_db:.2})",
            level.over,
            level.reduction_db
        );
    }
}

/// No built-in song clips at Master Level 100%, dry or with every gesture
/// held at full throw: the output stays at or under the -1 dBFS ceiling.
#[test]
fn every_song_at_full_level_stays_under_the_ceiling() {
    let held: [&[GestureKind]; 2] = [&[], &GestureKind::ALL];
    let levels = std::thread::scope(|scope| {
        let handles: Vec<_> = decode_auto_states()
            .into_iter()
            .enumerate()
            .flat_map(|(index, mut song)| {
                song.controls.master.level = 1.0;
                held.map(|held| {
                    let song = song.clone();
                    scope.spawn(move || (index + 1, held.len(), render(&song, held, 9.0)))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("render thread"))
            .collect::<Vec<_>>()
    });
    for (number, gestures, level) in &levels {
        assert_eq!(
            level.over, 0,
            "song {number} with {gestures} gestures: peak {:.4}",
            level.peak
        );
    }
}

/// A mix far hotter than any built-in song (every layer and Master Level at
/// 100%) is held at the ceiling, not clipped past it. The limiter's
/// reduction is how far the stage's input crossed -1 dBFS, so without it
/// this render would go over.
#[test]
fn hot_mix_is_held_at_the_ceiling() {
    let mut song = decode_auto_states().swap_remove(9);
    for tab in Tab::all() {
        if let Some(id) = tab.level_id() {
            (spec_by_id(id).unwrap().set)(&mut song.controls, 1.0);
        }
    }
    let level = render(&song, &[], 9.0);
    assert_eq!(level.over, 0, "peak {:.4}", level.peak);
    assert!(
        level.reduction_db > 0.5,
        "the mix never drove the limiter: {:.2} dB",
        level.reduction_db
    );
}

#[test]
#[ignore = "diagnostic render, prints a level table"]
fn gesture_level_probe() {
    use GestureKind::*;
    let cases: [(&str, &[GestureKind]); 6] = [
        ("Bloom", &[Bloom]),
        ("Submerge", &[Submerge]),
        ("Echo", &[Echo]),
        ("Lift", &[Lift]),
        ("Beat Repeat", &[BeatRepeat]),
        ("Bl+Ec+Su+Rp", &[Bloom, Echo, Submerge, BeatRepeat]),
    ];
    let mut songs: Vec<(String, SongState)> = decode_auto_states()
        .into_iter()
        .enumerate()
        .map(|(index, song)| (format!("auto {index:2}"), song))
        .collect();
    for progression in 0..PROGRESSIONS.len() {
        let mut controls = FluidControls::default();
        controls.pad.progression = progression as f32;
        songs.push((
            format!("fresh {progression}"),
            SongState::from_controls(controls),
        ));
    }
    for (name, song) in &songs {
        let dry = render(song, &[], 14.0);
        println!(
            "{name}: dry peak {:.3}, limiter {:.2} dB",
            dry.peak, dry.reduction_db
        );
        for (case, held) in cases {
            let wet = render(song, held, 14.0);
            let delta = |band: usize| wet.bands_db[band] - dry.bands_db[band];
            println!(
                "  {case:<8} peak {:.3} over {:>3} limiter {:5.2} dB | low {:+5.1} mid {:+5.1} high {:+5.1} dB",
                wet.peak,
                wet.over,
                wet.reduction_db,
                delta(0),
                delta(1),
                delta(2)
            );
        }
    }
}

/// Bloom and Echo let go within `GESTURE_RELEASE_SECONDS` without a click:
/// rendered through the whole engine at real level, the wash's own
/// sample-to-sample motion never jumps during the release beyond what it does
/// while held, and once the release ends the output is the dry song, bit for
/// bit.
#[test]
fn wet_gestures_let_go_cleanly_within_the_release_time() {
    for kind in [GestureKind::Bloom, GestureKind::Echo] {
        assert_releases_cleanly(kind);
    }
}

fn assert_releases_cleanly(kind: GestureKind) {
    const RELEASE_AT: f64 = 4.0;
    let song = &decode_auto_states()[9];
    let render_frames = |gesture: GestureState| {
        let mut snapshot = LiveSessionSnapshot::from_song(song);
        snapshot.gestures = gesture;
        let mut engine = FluidEngine::new(
            RATE,
            LiveSession::new(snapshot),
            no_morph(),
            Arc::new(FluidTelemetry::default()),
        );
        engine.reseed(7);
        (0..((RELEASE_AT + 0.5) * f64::from(RATE)) as usize)
            .map(|_| engine.next_stereo())
            .collect::<Vec<_>>()
    };
    let mut released = GestureState::default();
    // Full from the start, returning from RELEASE_AT: exactly a key-up there.
    released.lanes[kind as usize] = GestureEnvelope {
        amount: 1.0,
        at_seconds: RELEASE_AT,
        held: false,
        restored: false,
    };
    let dry = render_frames(GestureState::default());
    let wet = render_frames(released);
    let release_frame = (RELEASE_AT * f64::from(RATE)) as usize;
    // The output stage's lookahead delays the release by its latency.
    let latency = Limiter::new(RATE).latency();
    let end_frame =
        release_frame + latency + (GESTURE_RELEASE_SECONDS * f64::from(RATE)) as usize + 2;
    let wash = |frame: usize| (wet[frame].0 - dry[frame].0, wet[frame].1 - dry[frame].1);
    let max_step = |frames: std::ops::Range<usize>| {
        frames
            .map(|frame| {
                let (now, before) = (wash(frame), wash(frame - 1));
                (now.0 - before.0).abs().max((now.1 - before.1).abs())
            })
            .fold(0.0_f32, f32::max)
    };

    let held_step = max_step(release_frame - 48_000..release_frame);
    let release_step = max_step(release_frame..end_frame);

    assert!(
        release_step <= held_step,
        "{} release stepped {release_step} against {held_step} while held",
        kind.name()
    );
    let lingering = (end_frame..wet.len()).find(|frame| wet[*frame] != dry[*frame]);
    assert_eq!(lingering, None, "{} outlived its release", kind.name());
}
