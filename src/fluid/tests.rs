use super::*;
use ratatui::backend::TestBackend;

const SAMPLE_RATE: f32 = 48_000.0;

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < f32::EPSILON,
        "expected {expected}, got {actual}"
    );
}

fn timing(sample: u64, bpm: f32) -> TimingContext {
    let sample_rate = f64::from(SAMPLE_RATE);
    let bpm = f64::from(bpm);
    let beat = sample as f64 * bpm / (60.0 * sample_rate);
    TimingContext::new(sample_rate, bpm, beat)
}

#[test]
fn midi_to_hz_matches_known_notes() {
    assert_close(midi_to_hz(69), 440.0); // A4
    assert_close(midi_to_hz(45), 110.0); // A2
    assert_close(midi_to_hz(60), 440.0 * 2f32.powf((60.0 - 69.0) / 12.0)); // C4
}

#[test]
fn pad_chord_converts_progression_a_first_chord() {
    let chord = pad_chord(0, 0, 0.0);
    assert_close(chord[0], 110.0); // A2
    assert_close(chord[1], 440.0 * 2f32.powf((50.0 - 69.0) / 12.0)); // D3
    assert_close(chord[2], 440.0 * 2f32.powf((55.0 - 69.0) / 12.0)); // G3
    assert_close(chord[3], 440.0 * 2f32.powf((60.0 - 69.0) / 12.0)); // C4
}

#[test]
fn pad_chord_applies_master_tune_offset() {
    let flat = pad_chord(0, 0, 0.0);
    let up_octave = pad_chord(0, 0, 12.0);
    let down_octave = pad_chord(0, 0, -12.0);
    for i in 0..4 {
        assert_close(up_octave[i], flat[i] * 2.0);
        assert_close(down_octave[i], flat[i] * 0.5);
    }
}

#[test]
fn pad_chord_converts_progression_d_last_chord() {
    let chord = pad_chord(3, 7, 0.0);
    assert_close(chord[0], 440.0 * 2f32.powf((43.0 - 69.0) / 12.0)); // G2
    assert_close(chord[1], 440.0 * 2f32.powf((50.0 - 69.0) / 12.0)); // D3
    assert_close(chord[2], 440.0 * 2f32.powf((55.0 - 69.0) / 12.0)); // G3
    assert_close(chord[3], 440.0 * 2f32.powf((64.0 - 69.0) / 12.0)); // E4
}

#[test]
fn pad_chord_wraps_progression_and_step_index() {
    let wrapped_progression = pad_chord(4, 0, 0.0);
    let base_progression = pad_chord(0, 0, 0.0);
    assert_eq!(wrapped_progression, base_progression);

    let wrapped_step = pad_chord(0, 8, 0.0);
    let base_step = pad_chord(0, 0, 0.0);
    assert_eq!(wrapped_step, base_step);
}

#[test]
fn pad_defaults_use_progression_a_and_eight_bar_chords() {
    let controls = PadControls::default();
    assert_close(controls.chord_bars, 8.0);
    assert_close(controls.progression, 0.0);
}

#[test]
fn tab_previous_wraps_back_one_tab() {
    assert_eq!(Tab::Master.previous(), Tab::Clap);
    assert_eq!(Tab::Kick.previous(), Tab::Bass);
    assert_eq!(Tab::Bass.previous(), Tab::Chords);
}

#[test]
fn render_fluid_draws_without_terminal_backend() {
    let controls = FluidControls::default();
    let fluid = FluidState::new();
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    let items = tab_controls(Tab::Master, &controls);

    terminal
        .draw(|f| render(f, &items, Tab::Master, 0, None, false, &fluid))
        .unwrap();
}

#[test]
fn defaults_match_current_mix() {
    let controls = FluidControls::default();

    assert_close(controls.master.bpm, 82.0);
    assert_close(controls.master.drive, 0.1);
    assert_close(controls.master.comp_threshold, -8.0);

    assert_close(controls.perc.decay_ms, 200.0);
    assert_close(controls.perc.filter, 0.7);
    assert_close(controls.perc.lfo_rate_bars, 1.0);
    assert_close(controls.perc.lfo_depth, 0.1);
    assert_close(controls.perc.interval_beats, 0.25);
    assert_close(controls.perc.offset_beats, 0.0);

    assert_close(controls.kick.start_freq, 160.0);
    assert_close(controls.kick.pitch_decay_ms, 55.0);
    assert_close(controls.kick.amp_decay_ms, 250.0);

    assert_close(controls.tonal.step_interval_beats, 2.5);
    assert_close(controls.tonal.note_length_beats, 1.5);
    assert_close(controls.tonal.randomness, 0.5);

    assert_close(controls.clap.room, 0.0);
}

#[test]
fn apply_min_moves_selected_control_to_floor() {
    let mut controls = FluidControls::default();

    controls.master.drive = 0.8;
    apply_min(Tab::Master, 8, &mut controls);
    assert_close(controls.master.drive, 0.0);

    controls.master.bpm = 120.0;
    apply_min(Tab::Master, 6, &mut controls);
    assert_close(controls.master.bpm, MASTER_BPM_MIN);

    controls.master.tone = 0.5;
    apply_min(Tab::Master, 12, &mut controls);
    assert_close(controls.master.tone, -1.0);

    controls.pad.chord_bars = 16.0;
    apply_min(Tab::Chords, 1, &mut controls);
    assert_close(controls.pad.chord_bars, 1.0);
}

#[test]
fn apply_value_accepts_percent_style_unit_controls() {
    let mut controls = FluidControls::default();

    apply_value(Tab::Master, 7, 42.0, &mut controls);
    assert_close(controls.master.level, 0.42);

    apply_value(Tab::Master, 7, 0.7, &mut controls);
    assert_close(controls.master.level, 0.7);
}

#[test]
fn apply_value_snaps_direct_numeric_entry_to_control_grid() {
    let mut controls = FluidControls::default();

    apply_value(Tab::Kick, 1, 1.13, &mut controls);
    assert_close(controls.kick.interval_beats, 1.25);

    apply_value(Tab::Chords, 1, 12.0, &mut controls);
    assert_close(controls.pad.chord_bars, 16.0);

    apply_value(Tab::Clap, 3, 3.6, &mut controls);
    assert_close(controls.clap.slap_count, 4.0);
}

#[test]
fn tab_controls_classify_each_slider_kind() {
    use ControlKind::{Continuous, Discrete, Gain, Timing};

    let controls = FluidControls::default();
    let cases = [
        (
            Tab::Master,
            vec![
                Gain, Gain, Gain, Gain, Gain, Gain, Timing, Gain, Gain, Continuous, Continuous,
                Timing, Continuous, Discrete,
            ],
        ),
        (
            Tab::Perc,
            vec![Gain, Timing, Timing, Timing, Gain, Timing, Gain],
        ),
        (
            Tab::Chords,
            vec![
                Gain, Timing, Discrete, Gain, Gain, Gain, Gain, Timing, Timing,
            ],
        ),
        (
            Tab::Bass,
            vec![
                Gain, Timing, Timing, Discrete, Discrete, Timing, Timing, Gain,
            ],
        ),
        (
            Tab::Kick,
            vec![
                Gain, Timing, Timing, Continuous, Timing, Timing, Gain, Gain, Gain, Timing,
                Gain, Gain, Gain,
            ],
        ),
        (Tab::Tonal, vec![Gain, Timing, Timing, Gain, Timing, Gain]),
        (
            Tab::Clap,
            vec![
                Gain, Timing, Timing, Discrete, Timing, Timing, Gain, Gain, Gain,
            ],
        ),
    ];

    for (tab, expected) in cases {
        let actual: Vec<_> = tab_controls(tab, &controls)
            .into_iter()
            .map(|item| item.kind)
            .collect();
        assert_eq!(actual, expected, "unexpected kind map for {}", tab.name());
    }
}

#[test]
fn control_registry_specs_are_internally_consistent() {
    let tabs = [
        Tab::Master,
        Tab::Perc,
        Tab::Chords,
        Tab::Bass,
        Tab::Kick,
        Tab::Tonal,
        Tab::Clap,
    ];
    for tab in tabs {
        for spec in tab_specs(tab) {
            let ctx = format!("{} / {}", tab.name(), spec.label);
            assert!(!spec.label.is_empty(), "{ctx}: empty label");
            assert!(spec.min < spec.max, "{ctx}: min must be below max");
            assert!(
                spec.reset >= spec.min && spec.reset <= spec.max,
                "{ctx}: reset outside [min, max]"
            );
            if spec.bar == Bar::Log2 {
                assert!(spec.min > 0.0, "{ctx}: log bar needs positive min");
            }
            if let Step::Linear(step) = spec.step {
                assert!(step > 0.0, "{ctx}: step must be positive");
            }

            // get/set must address the same field.
            let mut c = FluidControls::default();
            (spec.set)(&mut c, spec.max);
            assert!(
                ((spec.get)(&c) - spec.max).abs() < 1e-6,
                "{ctx}: get/set roundtrip failed at max"
            );
            (spec.set)(&mut c, spec.reset);
            assert!(
                ((spec.get)(&c) - spec.reset).abs() < 1e-6,
                "{ctx}: get/set roundtrip failed at reset"
            );
        }
    }
}

#[test]
fn control_kind_smoothing_policy_is_explicit() {
    assert!(ControlKind::Gain.smooths_audio());
    assert!(!ControlKind::Continuous.smooths_audio());
    assert!(!ControlKind::Timing.smooths_audio());
    assert!(!ControlKind::Discrete.smooths_audio());
}

#[test]
fn gain_smoother_reaches_target_over_ramp() {
    let mut smoother = GainSmoother::new(0.0);
    smoother.set_target(1.0, 10);

    assert_close(smoother.next(), 0.1);
    for _ in 0..8 {
        smoother.next();
    }
    assert_close(smoother.next(), 1.0);
    assert_close(smoother.next(), 1.0);
}

#[test]
fn gain_smoothers_ramp_live_gain_controls_without_timing_changes() {
    let mut controls = FluidControls::default();
    controls.pad.level = 0.0;
    controls.pad.reverb_mix = 0.0;
    controls.perc.filter = 0.5;
    controls.kick.echo_amount = 0.0;
    controls.master.level = 0.0;
    controls.master.drive = 0.0;

    let mut smoothers = GainSmoothers::new(&controls);
    controls.pad.level = 1.0;
    controls.pad.reverb_mix = 1.0;
    controls.perc.filter = 1.0;
    controls.kick.echo_amount = 0.9;
    controls.master.level = 0.5;
    controls.master.drive = 1.0;
    controls.master.bpm = 123.0;
    controls.bass.drive = 1.0;
    smoothers.set_targets(&controls, 100.0);

    let next = smoothers.next_controls(&controls);
    assert_close(next.master.bpm, 123.0);
    assert!(next.pad.level > 0.0 && next.pad.level < 1.0);
    assert!(next.pad.reverb_mix > 0.0 && next.pad.reverb_mix < 1.0);
    assert!(next.perc.filter > 0.5 && next.perc.filter < 1.0);
    assert!(next.kick.echo_amount > 0.0 && next.kick.echo_amount < 0.9);
    assert!(next.master.level > 0.0 && next.master.level < 0.5);
    assert!(next.master.drive > 0.0 && next.master.drive < 1.0);
    assert_close(next.bass.drive, 1.0);
}

#[test]
fn chords_tab_shows_progression_row_with_letter_display() {
    let mut controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[2].label, "Progression");
    assert_eq!(rows[2].display, "A");

    controls.pad.progression = 2.0;
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[2].display, "C");
}

#[test]
fn chords_progression_adjusts_and_clamps() {
    let mut controls = FluidControls::default();

    apply_delta(Tab::Chords, 2, 1.0, &mut controls);
    assert_close(controls.pad.progression, 1.0);

    controls.pad.progression = 3.0;
    apply_delta(Tab::Chords, 2, 1.0, &mut controls);
    assert_close(controls.pad.progression, 3.0);

    controls.pad.progression = 0.0;
    apply_delta(Tab::Chords, 2, -1.0, &mut controls);
    assert_close(controls.pad.progression, 0.0);

    controls.pad.progression = 2.0;
    apply_min(Tab::Chords, 2, &mut controls);
    assert_close(controls.pad.progression, 0.0);
}

#[test]
fn bass_rhythms_have_expected_hit_counts() {
    assert_eq!(BASS_RHYTHMS[0].iter().filter(|&&b| b).count(), 4);
    assert!(BASS_RHYTHMS[0][0]);
    assert!(BASS_RHYTHMS[1].iter().filter(|&&b| b).count() > 4);
    assert_eq!(BASS_RHYTHMS[2].iter().filter(|&&b| b).count(), 8);
}

#[test]
fn bass_root_note_follows_authored_bass_line() {
    assert_eq!(bass_root_note(0, 0), 45);
    // Progression A's authored line diverges from the chord's lowest
    // tone at step 3 (B chord's min is 47) — proves the bass line is
    // independent data, not derived from PROGRESSIONS.
    assert_eq!(bass_root_note(0, 3), 43);
    assert_eq!(bass_root_note(2, 3), 43);
}

#[test]
fn bass_defaults_are_silent_quarter_note_a() {
    let controls = BassControls::default();
    assert_close(controls.level, 0.0);
    assert_close(controls.rhythm, 0.0);
    assert_close(controls.octave, -1.0);
    assert_close(controls.interval_beats, 4.0);
}

#[test]
fn bass_tab_shows_rhythm_row_with_letter_display() {
    let mut controls = FluidControls::default();
    let rows = tab_controls(Tab::Bass, &controls);
    assert_eq!(rows[3].label, "Rhythm");
    assert_eq!(rows[3].display, "A");

    controls.bass.rhythm = 3.0;
    let rows = tab_controls(Tab::Bass, &controls);
    assert_eq!(rows[3].display, "D");
}

#[test]
fn bass_controls_adjust_and_clamp() {
    let mut controls = FluidControls::default();

    apply_delta(Tab::Bass, 3, 1.0, &mut controls);
    assert_close(controls.bass.rhythm, 1.0);

    controls.bass.rhythm = 3.0;
    apply_delta(Tab::Bass, 3, 1.0, &mut controls);
    assert_close(controls.bass.rhythm, 3.0);

    controls.bass.octave = -1.0;
    apply_delta(Tab::Bass, 4, -1.0, &mut controls);
    apply_delta(Tab::Bass, 4, -1.0, &mut controls);
    assert_close(controls.bass.octave, -3.0);

    apply_min(Tab::Bass, 0, &mut controls);
    assert_close(controls.bass.level, 0.0);

    controls.bass.decay_time = 0.4;
    apply_delta(Tab::Bass, 6, 1.0, &mut controls);
    assert!(controls.bass.decay_time > 0.4);

    apply_min(Tab::Bass, 6, &mut controls);
    assert_close(controls.bass.decay_time, 0.005);
}

#[test]
fn bass_engine_follows_pad_chord_root_across_advances() {
    let sample_rate = 48_000.0;
    let mut bass = BassEngine::new(sample_rate);
    let pad = PadControls {
        chord_bars: 1.0 / 4.0, // advance every beat, fast enough to observe within the test
        ..PadControls::default()
    };
    let bass_controls = BassControls {
        interval_beats: 1.0,
        rhythm: 0.0,
        ..BassControls::default()
    };
    let mut clock = TempoClock::new(sample_rate, 120.0);

    // Step far enough to guarantee at least one chord advance and one
    // rhythm hit have occurred.
    for _ in 0..(sample_rate as usize) {
        let timing = clock.tick(120.0);
        bass.next(&bass_controls, &pad, 0.0, timing);
    }

    assert_ne!(bass.step_index, 0);
    assert!(bass.rhythm_step < BASS_RHYTHMS[0].len());
}

#[test]
fn bass_voice_decays_to_silence_without_sustaining() {
    let sample_rate = 48_000.0;
    let mut voice = BassVoice::new(110.0, 0.005, 0.05, 0.0, sample_rate);

    // Well past attack+decay (0.055s); a sustaining envelope would still
    // be holding at ~0.85 gain here, an AD envelope has decayed to ~0.
    for _ in 0..(sample_rate * 0.5) as usize {
        voice.next();
    }

    let (l, r) = voice.next();
    assert!(l.abs() < 0.001 && r.abs() < 0.001);
}

#[test]
fn bass_interval_crops_phrase_instead_of_stretching_it() {
    // Step duration is always a fixed 16th note; `interval_beats` only
    // decides how many steps of the 16-step phrase play before looping
    // back to step 0.
    let hits_within = |rhythm: usize, loop_len: usize| -> Vec<usize> {
        (0..loop_len)
            .filter(|&s| s < BASS_RHYTHMS[rhythm].len() && BASS_RHYTHMS[rhythm][s])
            .collect()
    };

    // Progression A (quarter notes) hits every 4 steps; cropping to a
    // 4-step (1 beat) loop still only exposes step 0, which recurs at
    // the same cadence as the full 16-step phrase - no audible change.
    assert_eq!(hits_within(0, 16), vec![0, 4, 8, 12]);
    assert_eq!(hits_within(0, 4), vec![0]);
    assert_eq!(hits_within(0, 8), vec![0, 4]);

    // A busier pattern's crop is audibly different: only its first half
    // survives an 8-step (2 beat) loop.
    let full = hits_within(1, 16);
    let cropped = hits_within(1, 8);
    assert!(cropped.len() < full.len());
    assert!(cropped.iter().all(|s| full.contains(s)));
}

#[test]
fn chords_reverb_mix_row_shifted_to_index_three() {
    let controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[3].label, "Reverb Mix");
}

#[test]
fn chords_release_row_present_with_lowered_attack_floor() {
    let controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[7].label, "Attack");
    assert_close(rows[7].min, 0.05);
    assert_eq!(rows[8].label, "Release");
    assert_close(rows[8].value, 8.0);
    assert_close(rows[8].min, 0.05);
    assert_close(rows[8].max, 20.0);
}

#[test]
fn chords_attack_and_release_adjust_and_clamp_low() {
    let mut controls = FluidControls::default();

    controls.pad.attack_time = 0.1;
    apply_delta(Tab::Chords, 7, -1.0, &mut controls);
    assert_close(controls.pad.attack_time, 0.05);
    apply_min(Tab::Chords, 7, &mut controls);
    assert_close(controls.pad.attack_time, 0.05);

    controls.pad.release_time = 0.1;
    apply_delta(Tab::Chords, 8, -1.0, &mut controls);
    assert_close(controls.pad.release_time, 0.05);
    apply_min(Tab::Chords, 8, &mut controls);
    assert_close(controls.pad.release_time, 0.05);
}

#[test]
fn kick_interval_floor_is_quarter_beat() {
    let mut controls = FluidControls::default();
    controls.kick.interval_beats = 1.0;
    apply_min(Tab::Kick, 1, &mut controls);
    assert_close(controls.kick.interval_beats, 0.25);

    controls.kick.interval_beats = 0.25;
    apply_delta(Tab::Kick, 1, -1.0, &mut controls);
    assert_close(controls.kick.interval_beats, 0.25);
}

#[test]
fn perc_continuous_mode_pushes_no_hits() {
    let mut controls = PercControls::default();
    controls.level = 1.0;
    controls.interval_beats = 4.25;

    let mut engine = PercEngine::new(SAMPLE_RATE);
    engine.rng = StdRng::seed_from_u64(7);
    let bpm = 82.0;
    for sample in 0..(SAMPLE_RATE as u64 * 2) {
        let t = timing(sample, bpm);
        engine.next(&controls, t);
    }
    assert!(engine.hits.is_empty());
}

#[test]
fn perc_continuous_mode_has_no_periodic_rms_dips() {
    let mut controls = PercControls::default();
    controls.level = 1.0;
    controls.lfo_depth = 0.0;
    controls.interval_beats = 4.25;

    let mut engine = PercEngine::new(SAMPLE_RATE);
    engine.rng = StdRng::seed_from_u64(7);
    let bpm = 82.0;
    let window_samples = (SAMPLE_RATE * 0.01) as usize;
    let total_samples = SAMPLE_RATE as usize * 2;
    let mut window_rms = Vec::new();
    let mut window = Vec::with_capacity(window_samples);

    for sample in 0..total_samples as u64 {
        let t = timing(sample, bpm);
        let out = engine.next(&controls, t);
        window.push(out);
        if window.len() == window_samples {
            let sum_sq: f32 = window.iter().map(|x| x * x).sum();
            window_rms.push((sum_sq / window.len() as f32).sqrt());
            window.clear();
        }
    }

    let settle_windows = (SAMPLE_RATE * 0.25) as usize / window_samples;
    let rms_tail = &window_rms[settle_windows..];

    let min_rms = rms_tail.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_rms = rms_tail.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

    assert!(
        min_rms > 0.0,
        "continuous mode produced silence in a window"
    );
    assert!(
        max_rms / min_rms < 2.0,
        "windowed RMS varies too much ({min_rms}..{max_rms}), suggests periodic triggering survived"
    );
}

#[test]
fn perc_tab_controls_include_interval_and_offset() {
    let controls = FluidControls::default();
    let rows = tab_controls(Tab::Perc, &controls);
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[1].label, "Interval");
    assert_close(rows[1].min, 0.25);
    assert_close(rows[1].max, 4.25);
    assert_eq!(rows[2].label, "Offset");
    assert_close(rows[2].min, 0.0);
    assert_close(rows[2].max, 4.0);
}

#[test]
fn perc_interval_displays_continuous_at_top() {
    let mut controls = FluidControls::default();
    controls.perc.interval_beats = 4.25;
    let rows = tab_controls(Tab::Perc, &controls);
    assert_eq!(rows[1].display, "Continuous");
}

#[test]
fn perc_interval_and_offset_adjust_and_clamp() {
    let mut controls = FluidControls::default();

    apply_delta(Tab::Perc, 1, 1.0, &mut controls);
    assert_close(controls.perc.interval_beats, 0.5);

    controls.perc.interval_beats = 4.25;
    apply_delta(Tab::Perc, 1, 1.0, &mut controls);
    assert_close(controls.perc.interval_beats, 4.25);

    apply_delta(Tab::Perc, 2, 1.0, &mut controls);
    assert_close(controls.perc.offset_beats, 0.25);

    controls.perc.offset_beats = 4.0;
    apply_delta(Tab::Perc, 2, 1.0, &mut controls);
    assert_close(controls.perc.offset_beats, 4.0);

    apply_min(Tab::Perc, 1, &mut controls);
    assert_close(controls.perc.interval_beats, 0.25);

    apply_min(Tab::Perc, 2, &mut controls);
    assert_close(controls.perc.offset_beats, 0.0);
}

#[test]
fn pad_engine_caps_released_layers() {
    let controls = PadControls {
        chord_bars: 1.0,
        attack_time: 1.0,
        ..PadControls::default()
    };
    let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

    for chord in 1..12 {
        let sample = chord * SAMPLE_RATE as u64 * 2;
        let _ = pad.next(&controls, 0.0, timing(sample, 120.0));
        assert!(pad.layers.len() <= MAX_PAD_LAYERS);
    }
}

#[test]
fn pad_engine_step_index_wraps_at_eight() {
    let controls = PadControls {
        chord_bars: 1.0,
        attack_time: 1.0,
        ..PadControls::default()
    };
    let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

    // chord_bars=1.0 means chord_trigger fires every 4.0 beats; at 120 BPM
    // that's 2 seconds of samples per chord. Render 9 chord-advances worth
    // of samples (18 seconds) and confirm the telemetry index wrapped past 8.
    for chord in 1..=9 {
        let sample = chord * SAMPLE_RATE as u64 * 2;
        let _ = pad.next(&controls, 0.0, timing(sample, 120.0));
    }
    let final_index = pad.telemetry.chord_index.load(Ordering::Relaxed);
    assert!(
        final_index < 8,
        "step_index must wrap into 0..8, got {final_index}"
    );
}

#[test]
fn pad_engine_progression_switch_retriggers_immediately() {
    let mut controls = PadControls {
        chord_bars: 64.0, // long chord length so no chord-advance trigger fires
        // Short attack so the original layer's envelope is already audible
        // (not still ~0 from the very first sample) by the time it's released;
        // otherwise the release phase completes in the same tick it starts and
        // `retain` prunes it before this test can observe the pushed layer.
        attack_time: 0.001,
        ..PadControls::default()
    };
    let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

    // Warm up the original layer's envelope (still progression 0, so no push
    // happens here) so its level is non-negligible before it gets released;
    // otherwise the Adsr release completes in the same tick it starts and
    // `retain` prunes the layer before this test can observe the pushed one.
    for sample in 0..10 {
        let _ = pad.next(&controls, 0.0, timing(sample, 120.0));
    }
    let layers_before = pad.layers.len();

    // Flip progression with no further elapsed time / no chord-advance trigger.
    controls.progression = 1.0;
    let _ = pad.next(&controls, 0.0, timing(10, 120.0));

    assert!(
        pad.layers.len() > layers_before,
        "switching progression must push a new layer immediately, without waiting for chord_trigger"
    );
}

#[test]
fn kick_delay_buffer_covers_max_echo_at_min_bpm() {
    let max_delay =
        ((KICK_ECHO_TIME_BEATS_MAX * 60.0 / MASTER_BPM_MIN) * SAMPLE_RATE).ceil() as usize;
    let delay = KickDelay::new(max_kick_echo_delay_samples(SAMPLE_RATE));

    assert_eq!(delay.buf_l.len(), max_delay + 1);
}

#[test]
fn tempo_clock_preserves_beat_phase_when_bpm_changes() {
    let mut clock = TempoClock::new(SAMPLE_RATE, 120.0);
    let mut before = clock.tick(120.0);

    for _ in 1..20_000 {
        before = clock.tick(120.0);
    }

    let after = clock.tick(60.0);

    assert!(after.beat > before.beat);
    assert!(after.beat - before.beat < 0.001);
    assert!(after.bpm < before.bpm);
    assert!(after.bpm > 60.0);
}

#[test]
fn grid_trigger_keeps_next_hit_when_only_bpm_changes() {
    let mut clock = TempoClock::new(SAMPLE_RATE, 120.0);
    let mut trigger = GridTrigger::new();

    for _ in 0..25_000 {
        let timing = clock.tick(120.0);
        let _ = trigger.pop(timing, 1.0, 0.0);
    }

    let before = trigger.next_hit.map(|hit| hit.beat);
    let timing = clock.tick(60.0);
    let fired = trigger.pop(timing, 1.0, 0.0);
    let after = trigger.next_hit.map(|hit| hit.beat);

    assert!(!fired);
    assert_eq!(before, after);
}

#[test]
fn grid_trigger_fires_identically_for_same_params() {
    let mut a = GridTrigger::new();
    let mut b = GridTrigger::new();
    let mut a_hits = Vec::new();
    let mut b_hits = Vec::new();

    for sample in 0..(SAMPLE_RATE as u64 * 6) {
        let timing = timing(sample, 120.0);
        if a.pop(timing, 2.0, 1.0) {
            a_hits.push(sample);
        }
        if b.pop(timing, 2.0, 1.0) {
            b_hits.push(sample);
        }
    }

    assert!(a_hits.len() >= 3);
    assert_eq!(a_hits, b_hits);
}

#[test]
fn grid_trigger_no_silence_after_bpm_decrease() {
    let change_at = 50_000u64;
    let mut clock = TempoClock::new(SAMPLE_RATE, 120.0);
    let mut kick = GridTrigger::new();
    let mut clap = GridTrigger::new();
    let mut kick_hits: Vec<u64> = Vec::new();
    let mut clap_hits: Vec<u64> = Vec::new();

    for sample in 0..change_at {
        let timing = clock.tick(120.0);
        if kick.pop(timing, 1.0, 0.0) {
            kick_hits.push(sample);
        }
        if clap.pop(timing, 2.0, 1.0) {
            clap_hits.push(sample);
        }
    }

    for sample in change_at..(SAMPLE_RATE as u64 * 8) {
        let timing = clock.tick(60.0);
        if kick.pop(timing, 1.0, 0.0) {
            kick_hits.push(sample);
        }
        if clap.pop(timing, 2.0, 1.0) {
            clap_hits.push(sample);
        }
    }

    // Kick should fire within one new beat period after the change
    let one_beat_samples = (60.0 / 60.0 * SAMPLE_RATE as f64) as u64;
    let first_post = kick_hits.iter().copied().find(|&s| s >= change_at);
    assert!(
        first_post.is_some_and(|s| s - change_at <= one_beat_samples),
        "kick stalled after BPM decrease"
    );
}

#[test]
fn grid_trigger_no_silence_after_interval_increase() {
    let change_at = 50_000u64;
    let mut trigger = GridTrigger::new();
    let mut hits: Vec<u64> = Vec::new();

    for sample in 0..change_at {
        if trigger.pop(timing(sample, 120.0), 0.5, 0.0) {
            hits.push(sample);
        }
    }

    for sample in change_at..(SAMPLE_RATE as u64 * 8) {
        if trigger.pop(timing(sample, 120.0), 4.0, 0.0) {
            hits.push(sample);
        }
    }

    let new_interval_samples = (4.0 * 60.0 / 120.0 * SAMPLE_RATE) as u64;
    let first_post = hits.iter().copied().find(|&s| s >= change_at);
    assert!(
        first_post.is_some_and(|s| s - change_at <= new_interval_samples),
        "trigger stalled after interval increase"
    );
}

#[test]
fn clap_voice_starts_first_burst_at_local_sample_zero() {
    let controls = ClapControls {
        level: 1.0,
        slap_count: 4.0,
        slap_spread_ms: 40.0,
        ..ClapControls::default()
    };
    let mut rng = StdRng::seed_from_u64(99);
    let mut voice = ClapVoice::new(&controls, SAMPLE_RATE, &mut rng);

    assert_eq!(voice.scheduled.first().copied(), Some(0));
    let _ = voice.next(&mut rng);
    assert_eq!(voice.current, 1);
    assert!(!voice.bursts.is_empty());
    assert!(voice.scheduled.iter().all(|&sample| sample > 0));
}
