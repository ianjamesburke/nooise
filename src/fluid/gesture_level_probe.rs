//! Master headroom with gestures held at full amount, measured at real
//! playback level through the whole engine, output clamp included: a
//! regression bound and an ignored diagnostic table.
//! `cargo test --release gesture_level_probe -- --ignored --nocapture`

use super::*;

const RATE: f32 = 48_000.0;
const SKIP_SECONDS: f32 = 3.0;
/// `MasterBus` clamps its output here; a sample sitting on it was clipped.
const MASTER_CLAMP: f32 = 0.95;

struct Level {
    clamp_hits: u32,
    peak: f32,
    /// Energy below 250 Hz, 250 Hz to 2.5 kHz, and above 2.5 kHz.
    bands_db: [f32; 3],
}

fn render(song: &SongState, held: &[GestureKind], seconds: f32) -> Level {
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
    let one_pole = |hz: f32| 1.0 - (-std::f32::consts::TAU * hz / RATE).exp();
    let (a_low, a_high) = (one_pole(250.0), one_pole(2_500.0));
    let (mut lp_low, mut lp_high) = (0.0_f32, 0.0_f32);
    let mut energy = [0.0_f64; 3];
    let (mut clamp_hits, mut peak) = (0, 0.0_f32);
    for frame in 0..(seconds * RATE) as usize {
        let (l, r) = engine.next_stereo();
        if (frame as f32) < SKIP_SECONDS * RATE {
            continue;
        }
        for x in [l, r] {
            clamp_hits += u32::from(x.abs() >= MASTER_CLAMP - 1e-6);
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
        clamp_hits,
        peak,
        bands_db: energy.map(|e| 10.0 * e.max(1e-12).log10() as f32),
    }
}

/// The Master clamp is a backstop, never gain staging. Every gesture that
/// adds a return, thrown fully at once at real playback level, must never
/// touch it and must keep 4 dB clear of it: on the song whose Bloom lifts
/// peaks most (5), a song the pre-fix Bloom clipped 14,497 times (9), and
/// the loudest built-in song (18).
#[test]
fn full_gesture_throw_never_reaches_the_master_clamp() {
    use GestureKind::*;
    let songs = decode_auto_states();
    for index in [5, 9, 18] {
        let level = render(&songs[index], &[Bloom, Echo, Submerge], 9.0);
        assert!(
            level.clamp_hits == 0 && level.peak < 0.6,
            "song {index}: {} clamp hits, peak {:.3}",
            level.clamp_hits,
            level.peak
        );
    }
}

#[test]
#[ignore = "diagnostic render, prints a level table"]
fn gesture_level_probe() {
    use GestureKind::*;
    let cases: [(&str, &[GestureKind]); 5] = [
        ("Bloom", &[Bloom]),
        ("Submerge", &[Submerge]),
        ("Echo", &[Echo]),
        ("Lift", &[Lift]),
        ("Bl+Ec+Su", &[Bloom, Echo, Submerge]),
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
        println!("{name}: dry peak {:.3}, {} hits", dry.peak, dry.clamp_hits);
        for (case, held) in cases {
            let wet = render(song, held, 14.0);
            let delta = |band: usize| wet.bands_db[band] - dry.bands_db[band];
            println!(
                "  {case:<8} peak {:.3} hits {:>5} | low {:+5.1} mid {:+5.1} high {:+5.1} dB",
                wet.peak,
                wet.clamp_hits,
                delta(0),
                delta(1),
                delta(2)
            );
        }
    }
}

/// Bloom lets go in 50 ms without a click: rendered through the whole
/// engine at real level, the wash's own sample-to-sample motion never jumps
/// during the release beyond what it does while held, and once the release
/// ends the output is the dry song, bit for bit.
#[test]
fn bloom_release_lets_go_cleanly_within_fifty_milliseconds() {
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
    released.lanes[GestureKind::Bloom as usize] = GestureEnvelope {
        amount: 1.0,
        at_seconds: RELEASE_AT,
        held: false,
        restored: false,
    };
    let dry = render_frames(GestureState::default());
    let wet = render_frames(released);
    let release_frame = (RELEASE_AT * f64::from(RATE)) as usize;
    let end_frame = release_frame + (0.05 * RATE) as usize + 2;
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
        "release stepped {release_step} against {held_step} while held"
    );
    let lingering = (end_frame..wet.len()).find(|frame| wet[*frame] != dry[*frame]);
    assert_eq!(lingering, None, "Bloom outlived its release");
}
