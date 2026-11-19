//! Measurements for the output gain path. The ignored profile prints the
//! default mix, fader extremes, built-in songs, and a dense stress mix.
//! `cargo test --release gain_stage_profile -- --ignored --nocapture`

use super::*;
use crate::audio::StereoEngine;

const RATE: f32 = 44_100.0;

struct LevelStats {
    peak: f32,
    rms: f32,
    rms_after_fade: f32,
    clamp_hits: u32,
    non_finite: u32,
    voice_peak_rms: [f32; TAB_COUNT],
    master_gr_db: f32,
}

fn db(linear: f32) -> f32 {
    20.0 * linear.max(1e-12).log10()
}

fn render(
    controls: &FluidControls,
    seed: u64,
    seconds: f32,
    gestures: &[GestureKind],
) -> LevelStats {
    let mut snapshot = LiveSessionSnapshot::from_song(&SongState::from_controls(controls.clone()));
    for kind in gestures {
        snapshot.gestures.lanes[*kind as usize] = GestureEnvelope {
            amount: 1.0,
            at_seconds: 0.0,
            held: true,
            restored: false,
        };
    }
    render_snapshot(&snapshot, seed, seconds)
}

fn render_song(song: &SongState, seed: u64, seconds: f32, gestures: &[GestureKind]) -> LevelStats {
    let mut snapshot = LiveSessionSnapshot::from_song(song);
    for kind in gestures {
        snapshot.gestures.lanes[*kind as usize] = GestureEnvelope {
            amount: 1.0,
            at_seconds: 0.0,
            held: true,
            restored: false,
        };
    }
    render_snapshot(&snapshot, seed, seconds)
}

fn render_snapshot(snapshot: &LiveSessionSnapshot, seed: u64, seconds: f32) -> LevelStats {
    let telemetry = Arc::new(FluidTelemetry::default());
    let mut engine = FluidEngine::new(
        RATE,
        LiveSession::new(snapshot.clone()),
        no_morph(),
        Arc::clone(&telemetry),
    );
    engine.reseed(seed);
    let fade_frame = (2.0 * RATE) as usize;
    let frames = (seconds * RATE) as usize;
    let mut peak = 0.0_f32;
    let mut sum_sq = 0.0_f64;
    let mut sum_sq_after = 0.0_f64;
    let mut after = 0_usize;
    let mut clamp_hits = 0_u32;
    let mut non_finite = 0_u32;
    let mut voice_peak_rms = [0.0_f32; TAB_COUNT];
    let mut master_gr_db = 0.0_f32;
    let gr_index = module_layer_index(Tab::Master).unwrap() * MODULE_SLOTS + 1;
    for frame in 0..frames {
        let (left, right) = engine.next_stereo();
        for sample in [left, right] {
            if !sample.is_finite() {
                non_finite += 1;
                continue;
            }
            peak = peak.max(sample.abs());
            let sq = f64::from(sample * sample);
            sum_sq += sq;
            if frame >= fade_frame {
                sum_sq_after += sq;
                after += 1;
            }
            if sample.abs() >= 0.95 - 1e-4 {
                clamp_hits += 1;
            }
        }
        if frame % LEVEL_BLOCK as usize == 0 {
            for (index, shown) in voice_peak_rms.iter_mut().enumerate() {
                *shown = shown.max(telemetry.level(Tab::all()[index]));
            }
            master_gr_db = master_gr_db.max(telemetry.gain_reduction()[gr_index]);
        }
    }
    let denom = (frames * 2).max(1) as f64;
    let after_denom = (after.max(1)) as f64;
    LevelStats {
        peak,
        rms: (sum_sq / denom).sqrt() as f32,
        rms_after_fade: (sum_sq_after / after_denom).sqrt() as f32,
        clamp_hits,
        non_finite,
        voice_peak_rms,
        master_gr_db,
    }
}

fn report(name: &str, stats: &LevelStats) {
    println!(
        "{name}: peak {:.3} ({:.2} dBFS) rms {:.2} dBFS after-fade {:.2} dBFS crest {:.2} dB clamp {} nan {} masterGR {:.2} dB",
        stats.peak,
        db(stats.peak),
        db(stats.rms),
        db(stats.rms_after_fade),
        db(stats.peak) - db(stats.rms_after_fade),
        stats.clamp_hits,
        stats.non_finite,
        stats.master_gr_db,
    );
    let names = [
        "pad", "perc", "bass", "kick", "tonal", "clap", "arp", "lead", "master",
    ];
    print!("  meters");
    for (name, rms) in names.iter().zip(stats.voice_peak_rms) {
        print!(" {name} {:.1}", db(rms));
    }
    println!();
}

fn dense_controls() -> FluidControls {
    let mut controls = FluidControls::default();
    controls.master.level = 1.0;
    controls.pad.level = 1.0;
    controls.pad.chord_notes = 5.0;
    controls.pad.detune = 1.0;
    controls.pad.octave_mix = 1.0;
    controls.pad.attack_time = 0.02;
    controls.pad.release_time = 6.0;
    controls.pad.chord_bars = 1.0;
    controls.perc.level = 1.0;
    controls.kick.level = 1.0;
    controls.kick.amp_decay_ms = 400.0;
    controls.tonal.level = 1.0;
    controls.clap.level = 1.0;
    controls.bass.level = 1.0;
    controls.arp.gain = 1.0;
    controls.lead.level = 1.0;
    controls.lead.pattern = 1.0;
    for slots in [
        &mut controls.modules.perc,
        &mut controls.modules.bass,
        &mut controls.modules.kick,
        &mut controls.modules.clap,
    ] {
        slots[0].right_time = 1.0;
    }
    controls.modules.master[0].amount = 1.0;
    controls.modules.pad[0].amount = 1.0;
    controls
}

fn dry_master(controls: &mut FluidControls) {
    for slot in &mut controls.modules.master {
        slot.amount = 0.0;
    }
    controls.modules.pad[0].amount = 0.0;
}

#[test]
fn master_level_zero_is_exact_silence_past_the_startup_fade() {
    let mut controls = FluidControls::default();
    controls.master.level = 0.0;
    let stats = render(&controls, 42, 2.5, &[]);
    assert_eq!(stats.peak, 0.0, "master level 0 must not pass audio");
    assert_eq!(stats.non_finite, 0);
}

#[test]
fn pad_level_zero_is_exact_silence_past_the_startup_fade() {
    let mut controls = FluidControls::default();
    controls.pad.level = 0.0;
    let stats = render(&controls, 42, 2.5, &[]);
    assert_eq!(
        stats.peak, 0.0,
        "pad level 0 must not pass the air or the chord"
    );
    assert_eq!(stats.non_finite, 0);
}

#[test]
fn master_mute_is_exact_silence_past_the_startup_fade() {
    let mut snapshot =
        LiveSessionSnapshot::from_song(&SongState::from_controls(FluidControls::default()));
    snapshot.muted[Tab::Master as usize] = true;
    let stats = render_snapshot(&snapshot, 42, 2.5);
    assert_eq!(stats.peak, 0.0, "master mute gates the bus after level");
    assert_eq!(stats.non_finite, 0);
}

/// Factory default, seed 42, 44.1 kHz. The window is the pad-sum calibration:
/// sample peak near -9.4 dBFS (the dry +6 dB, after the factory drive), crest
/// still that of an unsquashed pad, nothing on the master clamp. A return to
/// the old -14 dBFS peak, or a makeup large enough to square the chord, fails
/// this.
#[test]
fn default_mix_stays_in_the_calibrated_window() {
    let stats = render(&FluidControls::default(), 42, 10.0, &[]);
    assert_eq!(stats.non_finite, 0);
    assert_eq!(stats.clamp_hits, 0);
    assert!(
        (0.32..0.55).contains(&stats.peak),
        "default peak {:.3} ({:.2} dBFS) left the calibrated window",
        stats.peak,
        db(stats.peak),
    );
    let crest = db(stats.peak) - db(stats.rms_after_fade);
    assert!(
        crest > 8.0,
        "default crest {crest:.2} dB collapsed; the makeup should not be a limiter",
    );
}

#[test]
fn master_fader_stays_linear_on_the_default_pad() {
    let mut full = FluidControls::default();
    full.master.level = 1.0;
    let quiet = render(&FluidControls::default(), 42, 8.0, &[]);
    let loud = render(&full, 42, 8.0, &[]);
    let ratio = loud.peak / quiet.peak;
    assert!(
        (1.20..1.30).contains(&ratio),
        "master 100/80 peak ratio {ratio:.3}, expected ~1.25",
    );
}

#[test]
fn dense_mix_stays_finite_and_clear_of_the_master_clamp() {
    for seed in [42, 7] {
        let stats = render(&dense_controls(), seed, 6.0, &[]);
        assert_eq!(stats.non_finite, 0, "seed {seed}");
        assert_eq!(stats.clamp_hits, 0, "seed {seed} peak {:.3}", stats.peak);
        assert!(stats.peak < 0.9, "seed {seed} peak {:.3}", stats.peak);
        assert!(stats.peak > 0.2, "seed {seed} went silent");
    }
}

#[test]
#[ignore = "prints the gain-stage profile; run on demand"]
fn gain_stage_profile() {
    let seconds = 12.0;
    let base = FluidControls::default();
    report("default", &render(&base, 42, seconds, &[]));

    let mut master_full = base.clone();
    master_full.master.level = 1.0;
    report("master 100%", &render(&master_full, 42, seconds, &[]));

    let mut pad_full = base.clone();
    pad_full.pad.level = 1.0;
    report("pad 100% master 80%", &render(&pad_full, 42, seconds, &[]));

    let mut both = pad_full.clone();
    both.master.level = 1.0;
    report("pad 100% master 100%", &render(&both, 42, seconds, &[]));

    let mut dry = both.clone();
    dry_master(&mut dry);
    report(
        "pad 100% master 100% no room/drive/comp",
        &render(&dry, 42, seconds, &[]),
    );

    let mut voices = base.clone();
    voices.perc.level = 0.8;
    voices.kick.level = 0.8;
    voices.tonal.level = 0.6;
    voices.clap.level = 0.6;
    voices.bass.level = 0.7;
    voices.arp.gain = 0.6;
    voices.lead.level = 0.6;
    voices.lead.pattern = 1.0;
    report(
        "default pad plus voices up",
        &render(&voices, 42, seconds, &[]),
    );

    report("dense stress", &render(&dense_controls(), 42, seconds, &[]));
    report(
        "dense stress seed 7",
        &render(&dense_controls(), 7, seconds, &[]),
    );
    report(
        "dense + gestures",
        &render(
            &dense_controls(),
            42,
            seconds,
            &[GestureKind::Bloom, GestureKind::Echo, GestureKind::Submerge],
        ),
    );

    let songs = decode_auto_states();
    let mut loudest = (0_usize, 0.0_f32);
    for (index, song) in songs.iter().enumerate() {
        let stats = render_song(song, 42, 8.0, &[]);
        let c = &song.controls;
        println!(
            "auto {index:02} peak {:.2} dBFS rms {:.2} clamp {} GR {:.1} | M {:.2} pad {:.2} kick {:.2} bass {:.2} ton {:.2} clap {:.2} perc {:.2} arp {:.2} lead {:.2}",
            db(stats.peak),
            db(stats.rms_after_fade),
            stats.clamp_hits,
            stats.master_gr_db,
            c.master.level,
            c.pad.level,
            c.kick.level,
            c.bass.level,
            c.tonal.level,
            c.clap.level,
            c.perc.level,
            c.arp.gain,
            c.lead.level,
        );
        if stats.peak > loudest.1 {
            loudest = (index, stats.peak);
        }
    }
    for index in [3, 5, 8, 9, 13, 16, 17, 18] {
        let song = &songs[index];
        let mut silenced = song.clone();
        silenced.controls.pad.level = 0.0;
        let dry = render_song(song, 7, 9.0, &[]);
        let no_pad = render_song(&silenced, 7, 9.0, &[]);
        let thrown = render_song(
            song,
            7,
            9.0,
            &[GestureKind::Bloom, GestureKind::Echo, GestureKind::Submerge],
        );
        println!(
            "split {index:02} with-pad {:.3} ({:.2} dB) no-pad {:.3} ({:.2} dB) thrown {:.3} ({:.2} dB) clamp {}",
            dry.peak,
            db(dry.peak),
            no_pad.peak,
            db(no_pad.peak),
            thrown.peak,
            db(thrown.peak),
            thrown.clamp_hits,
        );
    }

    let mut dense = dense_controls();
    let with_pad = render(&dense, 42, 8.0, &[]);
    dense.pad.level = 0.0;
    let no_pad = render(&dense, 42, 8.0, &[]);
    println!(
        "dense split with-pad {:.3} no-pad {:.3}",
        with_pad.peak, no_pad.peak
    );

    let song = &songs[loudest.0];
    let thrown = render_song(
        song,
        42,
        9.0,
        &[GestureKind::Bloom, GestureKind::Echo, GestureKind::Submerge],
    );
    println!(
        "loudest auto {} dry {:.3} thrown peak {:.3} ({:.2} dBFS) clamp {} nan {}",
        loudest.0,
        loudest.1,
        thrown.peak,
        db(thrown.peak),
        thrown.clamp_hits,
        thrown.non_finite,
    );
}
