use super::*;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

const SAMPLE_RATE: f32 = 48_000.0;

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < f32::EPSILON,
        "expected {expected}, got {actual}"
    );
}

/// Test-only reconstruction of the pad's built-in chord frequencies, from
/// the two building blocks production code actually uses
/// (`pad_chord_midi`, `midi_to_hz`, `tune_ratio`) now that the custom
/// chord-slot path replaced the direct frequency helper in production.
fn pad_chord(progression: usize, step: usize, tune: f32) -> [f32; 4] {
    pad_chord_midi(progression, step).map(|note| midi_to_hz(note) * tune_ratio(tune))
}

fn assert_near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-5,
        "expected {expected}, got {actual}"
    );
}

fn timing(sample: u64, bpm: f32) -> TimingContext {
    let sample_rate = f64::from(SAMPLE_RATE);
    let bpm = f64::from(bpm);
    let beat = sample as f64 * bpm / (60.0 * sample_rate);
    TimingContext::new(sample_rate, bpm, beat)
}

/// A macro route riding a single slot, for tests that only care about one
/// macro slider (most of the pre-existing single-target coverage).
fn single_macro_route(slot: usize, amount: f32) -> MacroRoute {
    let mut amounts = [0.0; MACRO_COUNT];
    amounts[slot] = amount;
    MacroRoute { amounts }
}

fn append_record_to_code(code: &str, record_type: u8, payload: &[u8]) -> String {
    let encoded = code.strip_prefix("n1_").unwrap();
    let mut bytes = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    song::write_record(record_type, payload, &mut bytes).unwrap();
    format!("n1_{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn write_test_str(value: &str, out: &mut Vec<u8>) {
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

fn automation_payload(target_id: &str, route: LfoRoute) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.push(2);
    payload.extend_from_slice(&1u16.to_le_bytes());
    write_test_str(target_id, &mut payload);
    payload.extend_from_slice(&route.cycle_beats.to_le_bytes());
    payload.extend_from_slice(&route.depth_ratio.to_le_bytes());
    payload.push(0);
    payload.extend_from_slice(&route.phase_offset_beats.to_le_bytes());
    payload
}

fn buffer_text(buffer: &Buffer) -> String {
    buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
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
    let wrapped_progression = pad_chord(8, 0, 0.0);
    let base_progression = pad_chord(0, 0, 0.0);
    assert_eq!(wrapped_progression, base_progression);

    let wrapped_step = pad_chord(0, 8, 0.0);
    let base_step = pad_chord(0, 0, 0.0);
    assert_eq!(wrapped_step, base_step);
}

#[test]
fn new_progressions_hold_a_common_tone_between_consecutive_steps() {
    // Progressions E-H (indices 4-7, added alongside A-D's dark/major
    // expansion) are held to the strict common-tone discipline described in
    // their doc comments. A-D predate this test and include one documented
    // exception (progression A, step 4->5, an intentional stepwise glide
    // with no shared tone), so they are left unchecked here.
    for (progression_index, progression) in PROGRESSIONS.iter().enumerate().skip(4) {
        for step in 0..8 {
            let current = progression[step];
            let next = progression[(step + 1) % 8];
            let shares_a_tone = current.iter().any(|note| next.contains(note));
            assert!(
                shares_a_tone,
                "progression {progression_index} step {step} -> {} shares no common tone \
                 (an 8s release needs at least one held tone so chords don't clash)",
                (step + 1) % 8
            );
        }
    }
}

#[test]
fn tonal_phrase_a_keeps_existing_zero_randomness_melody() {
    assert_eq!(tonal_phrase(0), &[45, 50, 55, 48, 52, 57, 50, 55]);
}

#[test]
fn tonal_note_applies_master_tune_offset() {
    let flat = tonal_note_hz(45, 0.0);
    assert_close(tonal_note_hz(45, 12.0), flat * 2.0);
    assert_close(tonal_note_hz(45, -12.0), flat * 0.5);
}

#[test]
fn piano_harmonics_interpolate_with_note_pitch() {
    // Felt uses the acoustic-piano keyframe table, whose upper partials
    // strengthen from the low register into the mids.
    let profile = piano_profile(3);
    let low = piano_harmonic_amplitudes(profile, 36);
    let mid = piano_harmonic_amplitudes(profile, 48);
    let high = piano_harmonic_amplitudes(profile, 60);

    assert!(low[6] > high[6]);
    assert!(mid[1] > low[1]);
}

#[test]
fn piano_harmonic_decay_gets_faster_with_pitch() {
    let profile = piano_profile(1);
    let low = piano_harmonic_decay_rates(profile, 36, tonal_note_hz(36, 0.0));
    let high = piano_harmonic_decay_rates(profile, 60, tonal_note_hz(60, 0.0));

    assert!(high[15] > low[15]);
}

#[test]
fn attack_decay_gain_ramps_attack_then_decays() {
    // attack=0.5s, decay=0.5s, power=1 (linear decay): ramp up across the
    // attack, peak at its end, then fall linearly to zero across the decay.
    // The note's whole life is attack + decay = 1s.
    assert_close(attack_decay_gain(0.0, 0.5, 0.5, 1.0), 0.0);
    assert_close(attack_decay_gain(0.25, 0.5, 0.5, 1.0), 0.5);
    assert_close(attack_decay_gain(0.5, 0.5, 0.5, 1.0), 1.0);
    assert_close(attack_decay_gain(0.75, 0.5, 0.5, 1.0), 0.5);
    assert_close(attack_decay_gain(1.0, 0.5, 0.5, 1.0), 0.0);
}

#[test]
fn attack_decay_gain_is_pure_decay_with_zero_attack() {
    // With attack 0 the note peaks on the first sample and is a pure
    // `(1 - t)^power` decay across the decay time — exactly the clap/perc
    // decay shape the whole synth is unified around.
    let decay = 2.0;
    for &t in &[0.0f32, 0.1, 0.25, 0.6, 0.9, 1.0] {
        let elapsed = t * decay;
        let gain = attack_decay_gain(elapsed, 0.0, decay, 2.0);
        assert_near(gain, (1.0 - t).powf(2.0));
    }
}

#[test]
fn attack_decay_gain_peaks_at_end_of_attack() {
    // The envelope reaches unity exactly when the attack ramp finishes, then
    // immediately begins decaying — there is no hold-at-full stage.
    assert_close(attack_decay_gain(0.1, 0.2, 1.0, 1.0), 0.5);
    assert_close(attack_decay_gain(0.2, 0.2, 1.0, 1.0), 1.0);
    assert!(attack_decay_gain(0.3, 0.2, 1.0, 1.0) < 1.0);
}

#[test]
fn attack_decay_gain_reaches_zero_at_end_of_life() {
    // The envelope hits exactly 0 at elapsed = attack + decay, so a voice
    // killed at that length ends in silence rather than a clicking cut.
    assert_close(attack_decay_gain(1.0, 0.2, 0.8, 1.0), 0.0);
    // A decay of 0 collapses the note to nothing (the registry floors the
    // control above 0 so the UI can never request this hard cut).
    assert_close(attack_decay_gain(0.05, 0.1, 0.0, 1.0), 0.0);
}

#[test]
fn tonal_attack_control_changes_piano_voice_onset() {
    // Compare energy over the first handful of samples rather than the
    // very first one, since every oscillator starts at zero phase and
    // would otherwise mask the envelope difference.
    let profile = piano_profile(1);
    let mut fast = PianoTonalVoice::new(profile, 60, 440.0, 0.0, 1.0, SAMPLE_RATE, 0.0, 2.0);
    let mut slow = PianoTonalVoice::new(profile, 60, 440.0, 0.0, 1.0, SAMPLE_RATE, 0.05, 2.0);

    let fast_energy: f32 = (0..10).map(|_| fast.next().0.abs()).sum();
    let slow_energy: f32 = (0..10).map(|_| slow.next().0.abs()).sum();

    assert!(
        fast_energy > slow_energy,
        "a longer tonal.attack should produce a quieter onset: fast={fast_energy}, slow={slow_energy}"
    );
}

#[test]
fn tonal_decay_control_sets_ring_length() {
    // Decay is the note's whole ring: a long decay is still sounding well
    // past the point where a short-decay note has already fallen silent and
    // ended. This is the clap/perc decay model applied to the tonal voice.
    let profile = piano_profile(1);
    // attack 0, so each note's life is exactly its decay time.
    let mut long_decay = PianoTonalVoice::new(profile, 60, 440.0, 0.0, 1.0, SAMPLE_RATE, 0.0, 2.0);
    let mut short_decay = PianoTonalVoice::new(profile, 60, 440.0, 0.0, 1.0, SAMPLE_RATE, 0.0, 0.1);

    // 0.25s in — past the short note's 0.1s life, deep inside the long note's.
    let quarter_second = (SAMPLE_RATE * 0.25) as u64;
    for _ in 0..quarter_second {
        long_decay.next();
        short_decay.next();
    }
    let (long_l, _) = long_decay.next();
    let (short_l, _) = short_decay.next();

    assert!(
        short_decay.is_done(),
        "a short-decay note must have ended by 0.25s"
    );
    assert!(
        long_l.abs() > short_l.abs(),
        "a longer tonal.decay should still be ringing when the short note is silent: \
         long={long_l}, short={short_l}"
    );
}

#[test]
fn tonal_engine_triggers_all_non_sine_type_variants() {
    let controls = TonalControls {
        level: 1.0,
        randomness: 0.0,
        ..TonalControls::default()
    };
    for synth_type in 1..=9 {
        let controls = TonalControls {
            synth_type: synth_type as f32,
            ..controls.clone()
        };
        let mut tonal = TonalEngine::new(SAMPLE_RATE);

        let _ = tonal.next(
            &controls,
            0.0,
            TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 0.0),
        );

        assert!(matches!(tonal.voices.first(), Some(TonalVoice::Piano(_))));
    }
}

#[test]
fn tonal_type_labels_cover_exploration_variants() {
    assert_eq!(tonal_synth_type_label(0.0), "Sine");
    assert_eq!(tonal_synth_type_label(1.0), "Rhodes");
    assert_eq!(tonal_synth_type_label(2.0), "Wurli");
    assert_eq!(tonal_synth_type_label(3.0), "Felt");
    assert_eq!(tonal_synth_type_label(4.0), "Marimba");
    assert_eq!(tonal_synth_type_label(5.0), "Kalimba");
    assert_eq!(tonal_synth_type_label(6.0), "Pluck");
    assert_eq!(tonal_synth_type_label(7.0), "Dulcet");
    assert_eq!(tonal_synth_type_label(8.0), "Cloud Keys");
    assert_eq!(tonal_synth_type_label(9.0), "Haze");
}

#[test]
fn bass_low_pass_reduces_high_energy_without_thinning_low_notes() {
    fn filtered_sine_rms(hz: f32, cutoff_hz: f32) -> f32 {
        let mut low_pass = BassLowPass::new();
        let total = SAMPLE_RATE as u64 * 2;
        let warmup = SAMPLE_RATE as u64 / 2;
        let mut sum = 0.0f32;
        let mut count = 0u64;

        for sample in 0..total {
            let phase = TAU * hz * sample as f32 / SAMPLE_RATE;
            let filtered = low_pass.process(phase.sin(), cutoff_hz, SAMPLE_RATE);
            if sample >= warmup {
                sum += filtered * filtered;
                count += 1;
            }
        }

        (sum / count as f32).sqrt()
    }

    let cutoff = 300.0;
    let low = filtered_sine_rms(80.0, cutoff);
    let high = filtered_sine_rms(cutoff * 8.0, cutoff);

    assert!(low > 0.4, "low note rms should stay strong, got {low}");
    assert!(high < 0.3, "high content should be reduced, got {high}");
}

#[test]
fn tonal_low_cut_reduces_sub_energy_without_thinning_low_notes() {
    fn filtered_sine_rms(hz: f32) -> f32 {
        let mut low_cut = TonalLowCut::new(SAMPLE_RATE, TONAL_LOW_CUT_HZ);
        let total = SAMPLE_RATE as u64 * 2;
        let warmup = SAMPLE_RATE as u64 / 2;
        let mut sum = 0.0f32;
        let mut count = 0u64;

        for sample in 0..total {
            let phase = TAU * hz * sample as f32 / SAMPLE_RATE;
            let filtered = low_cut.process(phase.sin());
            if sample >= warmup {
                sum += filtered * filtered;
                count += 1;
            }
        }

        (sum / count as f32).sqrt()
    }

    let sub = filtered_sine_rms(TONAL_LOW_CUT_HZ * 0.5);
    let low_a = filtered_sine_rms(110.0);

    assert!(sub < 0.4, "sub rms should be reduced, got {sub}");
    assert!(
        low_a > 0.55,
        "lowest tonal fundamental should stay present, got {low_a}"
    );
    assert!(
        low_a > sub * 1.5,
        "low note should survive more than sub energy: sub {sub}, low_a {low_a}"
    );
}

#[test]
fn tonal_cycle_crops_phrase_without_stretching_rate() {
    assert_eq!(tonal_loop_len(4.0, 0.5), 8);
    assert_eq!(tonal_loop_len(16.0, 0.5), 32);
    assert_eq!(tonal_loop_len(4.0, 1.0), 4);
    assert_eq!(tonal_cycle_step(3.75, 4.0, 0.0, 0.5), 7);
    assert_eq!(tonal_cycle_step(3.75, 4.0, 0.0, 1.0), 3);
    assert_eq!(tonal_cycle_step(4.0, 4.0, 0.0, 0.5), 0);
}

#[test]
fn tonal_rate_controls_trigger_spacing_independent_of_cycle() {
    let controls = TonalControls {
        rate_beats: 1.0,
        step_interval_beats: 4.0,
        randomness: 0.0,
        ..TonalControls::default()
    };
    let mut tonal = TonalEngine::new(SAMPLE_RATE);

    let _ = tonal.next(
        &controls,
        0.0,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 0.0),
    );
    assert_eq!(tonal.step_index, 0);

    let _ = tonal.next(
        &controls,
        0.0,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 0.5),
    );
    assert_eq!(tonal.step_index, 0);

    let _ = tonal.next(
        &controls,
        0.0,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 1.0),
    );
    assert_eq!(tonal.step_index, 1);
}

#[test]
fn tonal_evolve_rate_maps_to_actual_notes_per_cycle() {
    assert_eq!(tonal_evolve_note_count(0.0, 8), 0);
    assert_eq!(tonal_evolve_note_count(0.01, 8), 1);
    assert_eq!(tonal_evolve_note_count(0.50, 8), 2);
    assert_eq!(tonal_evolve_note_count(1.0, 8), 4);
}

#[test]
fn tonal_engine_evolves_one_actual_note_at_low_rate() {
    let mut tonal = TonalEngine::new(SAMPLE_RATE);
    tonal.rng = StdRng::seed_from_u64(5);
    let before = tonal.evolved_phrase.clone();

    tonal.evolve_phrase(0.01);

    let changed = before
        .iter()
        .zip(&tonal.evolved_phrase)
        .filter(|(before, after)| before != after)
        .count();
    assert_eq!(changed, 1);
}

#[test]
fn tonal_engine_evolves_more_notes_at_high_rate() {
    let mut tonal = TonalEngine::new(SAMPLE_RATE);
    tonal.rng = StdRng::seed_from_u64(5);
    let before = tonal.evolved_phrase.clone();

    tonal.evolve_phrase(1.0);

    let changed = before
        .iter()
        .zip(&tonal.evolved_phrase)
        .filter(|(before, after)| before != after)
        .count();
    assert_eq!(changed, 4);
}

#[test]
fn pad_defaults_use_progression_a_and_sixteen_beat_chords() {
    let controls = PadControls::default();
    assert_close(controls.chord_bars, 4.0);
    assert_close(controls.progression, 0.0);
    assert_close(controls.voice_type, 0.0);
}

#[test]
fn chords_tab_shows_type_row_with_letter_display() {
    let mut controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[3].id, "pad.type");
    assert_eq!(rows[3].label, "Type");
    assert_eq!(rows[3].display, "Warm");

    controls.pad.voice_type = 1.0;
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[3].display, "Dark");

    controls.pad.voice_type = 2.0;
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[3].display, "Glass");
}

#[test]
fn tab_previous_wraps_back_one_tab() {
    assert_eq!(Tab::Master.previous(), Tab::Macros);
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
    let automation = AutomationState::default();

    terminal
        .draw(|f| {
            render(
                f,
                &items,
                Tab::Master,
                0,
                0,
                0.0,
                NumericDisplay::empty(),
                &fluid,
                &automation,
                &controls,
                None,
                &FlippedUnits::new(),
                ChordDrill::None,
                &[None; 9],
            )
        })
        .unwrap();
}

#[test]
fn automation_open_or_create_uses_safe_lfo_defaults() {
    let mut automation = AutomationState::default();
    let address = ControlAddress::new("master.level");

    let route = automation.open_or_create(address);

    assert_close(route.cycle_beats, 2.0);
    assert_close(route.depth_ratio, 0.0);
    assert_eq!(route.shape, LfoShape::Sine);
    assert_close(route.phase_offset_beats, 0.0);
    assert_eq!(automation.active_address(), Some(address));
}

#[test]
fn lfo_field_adjust_steps_and_clamps() {
    let mut route = LfoRoute::default();

    route.adjust_field_at(LfoField::Amount, 1.0, 0.0);
    assert_close(route.depth_ratio, 0.01);
    route.set_field_at(LfoField::Amount, 0.0, 0.0);
    route.adjust_field_at(LfoField::Amount, -1.0, 0.0);
    assert_close(route.depth_ratio, 0.0);

    route.adjust_field_at(LfoField::Interval, 1.0, 0.0);
    assert_close(route.cycle_beats, 2.25);
    for _ in 0..150 {
        route.adjust_field_at(LfoField::Interval, 1.0, 0.0);
    }
    assert_close(route.cycle_beats, 16.0);
    for _ in 0..150 {
        route.adjust_field_at(LfoField::Interval, -1.0, 0.0);
    }
    assert_close(route.cycle_beats, 0.125);

    route.adjust_field_at(LfoField::Offset, -1.0, 0.0);
    assert_close(route.phase_offset_beats, 0.0);
    route.adjust_field_at(LfoField::Offset, 1.0, 0.0);
    assert_close(route.phase_offset_beats, 0.125);
    for _ in 0..100 {
        route.adjust_field_at(LfoField::Offset, 1.0, 0.0);
    }
    assert_close(route.phase_offset_beats, 4.0);
}

#[test]
fn lfo_field_set_snaps_to_eighth_beat_grid() {
    let mut route = LfoRoute::default();

    route.set_field_at(LfoField::Interval, 3.1, 0.0);
    assert_close(route.cycle_beats, 3.0);
    route.set_field_at(LfoField::Interval, 0.17, 0.0);
    assert_close(route.cycle_beats, 0.125);
    route.set_field_at(LfoField::Interval, 100.0, 0.0);
    assert_close(route.cycle_beats, 16.0);
    route.set_field_at(LfoField::Amount, 130.0, 0.0);
    assert_close(route.depth_ratio, 1.0);
    route.set_field_at(LfoField::Amount, 40.0, 0.0);
    assert_close(route.depth_ratio, 0.4);
    route.set_field_at(LfoField::Offset, 1.3, 0.0);
    assert_close(route.phase_offset_beats, 1.25);
    route.set_field_at(LfoField::Offset, 9.0, 0.0);
    assert_close(route.phase_offset_beats, 4.0);
}

#[test]
fn discrete_fields_clamp_at_their_ends_instead_of_wrapping() {
    let mut route = LfoRoute::default();
    route.adjust_field_at(LfoField::Shape, -1.0, 0.0);
    assert_eq!(
        route.shape,
        LfoShape::Sine,
        "shape must not wrap below sine"
    );
    for _ in 0..20 {
        route.adjust_field_at(LfoField::Shape, 1.0, 0.0);
    }
    assert_eq!(
        route.shape,
        LfoShape::SampleHold,
        "shape must stop at the last entry"
    );
    route.set_field_at(LfoField::Shape, 99.0, 0.0);
    assert_eq!(
        route.shape,
        LfoShape::SampleHold,
        "numeric entry clamps, not wraps"
    );

    let mut env = EnvelopeRoute::default();
    for _ in 0..20 {
        env.adjust_field(EnvField::Trigger, 1.0);
    }
    assert_eq!(env.trigger, EnvTrigger::Once, "trigger must stop at once");
    env.adjust_field(EnvField::Trigger, 1.0);
    assert_eq!(env.trigger, EnvTrigger::Once);
    for _ in 0..20 {
        env.adjust_field(EnvField::Trigger, -1.0);
    }
    assert_eq!(env.trigger, EnvTrigger::EveryBeats(1.0));
}

#[test]
fn lfo_field_reset_uses_slider_minimums() {
    let mut route = LfoRoute {
        cycle_beats: 4.0,
        depth_ratio: 0.75,
        phase_offset_beats: 2.0,
        ..LfoRoute::default()
    };

    route.reset_field_at(LfoField::Amount, 1.0);
    assert_close(route.depth_ratio, 0.0);
    route.reset_field_at(LfoField::Interval, 1.0);
    assert_close(route.cycle_beats, MIN_LFO_CYCLE_BEATS);
    route.reset_field_at(LfoField::Offset, 1.0);
    assert_close(route.phase_offset_beats, 0.0);
}

#[test]
fn lfo_interval_edits_preserve_live_phase_when_possible() {
    let mut route = LfoRoute {
        cycle_beats: 2.0,
        phase_offset_beats: 0.0,
        ..LfoRoute::default()
    };
    let beat = 4.0;
    let before = route.phase_at(beat);

    route.adjust_field_at(LfoField::Interval, 1.0, beat);

    assert_close(route.cycle_beats, 2.25);
    assert!((route.phase_at(beat) - before).abs() < 1e-9);
}

#[test]
fn close_editor_deletes_zero_depth_route() {
    let mut automation = AutomationState::default();
    let address = ControlAddress::new("master.level");
    automation.open_or_create(address).depth_ratio = 0.0;

    automation.close_editor();

    assert!(automation.route(address).is_none());
    assert!(!automation.is_editor_open());

    automation.open_or_create(address);
    automation.close_editor();
    assert!(automation.route(address).is_none());
}

#[test]
fn same_key_toggles_editor_closed_and_keeps_the_route() {
    let controls = FluidControls::default();
    let items = tab_controls(Tab::Master, &controls);
    let shared = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let mut automation = PublishedAutomation::new(AutomationState::default(), shared);
    let address = ControlAddress::new(items[0].id);
    let mut sub = 0usize;

    open_modulator(&mut automation, &items, 0, ModKind::Lfo, &mut sub);
    automation.edit(|state| state.route_mut(address).unwrap().depth_ratio = 0.4);
    assert!(automation.state().is_editor_open());

    // Second tap closes the editor but the route keeps playing.
    open_modulator(&mut automation, &items, 0, ModKind::Lfo, &mut sub);
    assert!(!automation.state().is_editor_open());
    assert_close(automation.state().route(address).unwrap().depth_ratio, 0.4);
}

#[test]
fn x_removes_the_open_route_or_clears_the_whole_control() {
    let address = ControlAddress::new("master.level");
    let mut automation = AutomationState::default();

    // x with the editor open removes exactly that route and closes.
    automation.open_or_create(address).depth_ratio = 0.4;
    automation.remove_open_route();
    assert!(!automation.is_editor_open());
    assert!(automation.route(address).is_none());

    // x with no editor open strips every modulator on the control.
    automation.open_or_create(address).depth_ratio = 0.4;
    automation.close_editor();
    automation.set_macro_route(address, single_macro_route(0, 0.5));
    automation.clear_control(address);
    assert!(automation.route(address).is_none());
    assert!(automation.macro_route(address).is_none());
}

#[test]
fn macro_route_scales_target_into_its_range() {
    let mut controls = FluidControls::default();
    controls.master.level = 0.2;
    controls.macros.values[0] = 0.5;
    let mut automation = AutomationState::default();
    automation.set_macro_route(
        ControlAddress::new("master.level"),
        single_macro_route(0, 1.0),
    );

    let mut effective = controls.clone();
    apply_automation(&mut effective, &automation, timing(0, 120.0));
    assert_near(effective.master.level, 0.7);

    // Negative amount dips below the base and clamps at the control minimum.
    automation.set_macro_route(
        ControlAddress::new("master.level"),
        single_macro_route(0, -1.0),
    );
    let mut effective = controls.clone();
    apply_automation(&mut effective, &automation, timing(0, 120.0));
    assert_near(effective.master.level, 0.0);
}

#[test]
fn a_control_can_ride_several_macro_sliders_at_once() {
    // The core of the "4 amount sliders" model: no target selection, so a
    // control can be assigned to more than one macro at the same time, each
    // amount set and adjusted independently.
    let mut controls = FluidControls::default();
    controls.master.level = 0.2;
    controls.macros.values[0] = 0.5;
    controls.macros.values[2] = 1.0;
    let mut automation = AutomationState::default();
    let mut route = MacroRoute::default();
    route.amounts[0] = 0.4;
    route.amounts[2] = -0.3;
    automation.set_macro_route(ControlAddress::new("master.level"), route);

    // 0.2 + range(1.0) * (0.4 * 0.5 + -0.3 * 1.0) = 0.2 + (0.2 - 0.3) = 0.1
    let mut effective = controls.clone();
    apply_automation(&mut effective, &automation, timing(0, 120.0));
    assert_near(effective.master.level, 0.1);

    // Zeroing one slot doesn't disturb the other.
    automation
        .macro_route_mut(ControlAddress::new("master.level"))
        .unwrap()
        .amounts[2] = 0.0;
    let mut effective = controls.clone();
    apply_automation(&mut effective, &automation, timing(0, 120.0));
    assert_near(effective.master.level, 0.4);
}

#[test]
fn field_macro_on_lfo_amount_is_off_by_default_and_only_appears_after_v() {
    let controls = FluidControls::default();
    let items = tab_controls(Tab::Master, &controls);
    let shared = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let mut automation = PublishedAutomation::new(AutomationState::default(), shared);
    let address = ControlAddress::new(items[0].id);
    let key = unit_key(address.id(), Some("lfo.amount"));

    // Opening the LFO editor alone must NOT create a stacked macro on
    // amount — it only exists after the user explicitly presses v there.
    automation.edit(|state| {
        state.open_or_create(address);
    });
    assert_eq!(
        lfo_submenu_rows(automation.state(), address).len(),
        LfoField::ALL.len(),
        "no macro rows before v is pressed"
    );
    assert!(automation.state().field_macro(&key).is_none());

    // v on the amount row (lfo_selected == 1) expands it.
    automation.edit(|state| state.toggle_open_field(key.clone()));
    assert_eq!(
        lfo_submenu_rows(automation.state(), address).len(),
        LfoField::ALL.len() + MacroField::ALL.len(),
        "one row per macro slider nests in once expanded"
    );
    assert!(automation.state().field_macro(&key).is_some());

    // Left neutral, closing (v again) prunes it back out — same rule as
    // every other route kind.
    automation.edit(|state| state.toggle_open_field(key.clone()));
    assert!(automation.state().field_macro(&key).is_none());
    assert_eq!(
        lfo_submenu_rows(automation.state(), address).len(),
        LfoField::ALL.len()
    );
}

#[test]
fn close_one_level_collapses_the_nested_field_macro_before_the_whole_lfo_editor() {
    let controls = FluidControls::default();
    let items = tab_controls(Tab::Master, &controls);
    let shared = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let mut automation = PublishedAutomation::new(AutomationState::default(), shared);
    let address = ControlAddress::new(items[0].id);
    let key = unit_key(address.id(), Some("lfo.amount"));

    automation.edit(|state| {
        state.open_or_create(address).depth_ratio = 0.3;
    });
    automation.edit(|state| state.toggle_open_field(key.clone()));
    automation.edit(|state| {
        state.field_macro_mut(&key).unwrap().amounts[0] = 0.5;
    });
    let mut lfo_selected = 5; // sitting on one of the nested macro rows

    // First close: only the nested field-macro editor collapses. The LFO
    // editor (and its now-non-neutral field macro) stays open.
    close_one_level(&mut automation, &mut lfo_selected);
    assert!(automation.state().is_editor_open());
    assert_eq!(automation.state().active_kind(), Some(ModKind::Lfo));
    assert!(automation.state().field_macro(&key).is_some());
    assert_eq!(
        lfo_selected, 1,
        "cursor lands back on the amount field's own row"
    );

    // Second close: nothing nested remains open, so this closes the whole
    // LFO editor.
    close_one_level(&mut automation, &mut lfo_selected);
    assert!(!automation.state().is_editor_open());
    assert_eq!(lfo_selected, 0);
    // The route itself survives close — only the editor closed.
    assert_close(automation.state().route(address).unwrap().depth_ratio, 0.3);
}

#[test]
fn close_one_level_prunes_a_neutral_field_macro_left_open() {
    let controls = FluidControls::default();
    let items = tab_controls(Tab::Master, &controls);
    let shared = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let mut automation = PublishedAutomation::new(AutomationState::default(), shared);
    let address = ControlAddress::new(items[0].id);
    let key = unit_key(address.id(), Some("lfo.amount"));

    automation.edit(|state| {
        state.open_or_create(address);
    });
    automation.edit(|state| state.toggle_open_field(key.clone()));
    let mut lfo_selected = 5;

    close_one_level(&mut automation, &mut lfo_selected);
    assert!(
        automation.state().field_macro(&key).is_none(),
        "left neutral, the field macro prunes on close like every other route"
    );
    assert!(automation.state().is_editor_open());
    assert_eq!(lfo_selected, 1);
}

#[test]
fn macro_stacked_on_lfo_amount_via_v_scales_the_depth() {
    let mut controls = FluidControls::default();
    controls.master.level = 0.2;
    controls.macros.values[0] = 0.5;
    let mut automation = AutomationState::default();
    automation.set_route(
        ControlAddress::new("master.level"),
        LfoRoute {
            depth_ratio: 0.0,
            cycle_beats: 2.0,
            ..LfoRoute::default()
        },
    );
    // The only way this route exists: the user pressed v on the amount row.
    automation.set_field_macro(
        unit_key("master.level", Some("lfo.amount")),
        single_macro_route(0, 1.0),
    );

    // Half a beat into a 2-beat sine sits at its +1 peak, so the level is
    // base + range * (0 + 1.0 * macro 0.5).
    let half_beat = SAMPLE_RATE as u64 / 4; // 0.5 beats at 120 BPM
    let mut effective = controls.clone();
    apply_automation(&mut effective, &automation, timing(half_beat, 120.0));
    assert_near(effective.master.level, 0.7);

    // With the macro slider at zero the route contributes nothing again.
    controls.macros.values[0] = 0.0;
    let mut effective = controls.clone();
    apply_automation(&mut effective, &automation, timing(half_beat, 120.0));
    assert_near(effective.master.level, 0.2);
}

#[test]
fn macro_sliders_own_lfos_never_take_a_stacked_field_macro() {
    let mut automation = AutomationState::default();
    let address = ControlAddress::new("macro.1");
    automation.set_route(ControlAddress::new("macro.1"), LfoRoute::default());
    // Even if a stray field-macro entry existed for a macro's own LFO (e.g.
    // from a hand-edited song code), it must never apply.
    automation.set_field_macro(
        unit_key("macro.1", Some("lfo.amount")),
        single_macro_route(1, 1.0),
    );
    let controls = FluidControls::default();
    let route = automation.route(address).unwrap();
    let effective = effective_lfo_route(&automation, &controls, address, route);
    assert_close(effective.depth_ratio, route.depth_ratio);
}

#[test]
fn macro_slider_lfo_fields_do_not_expose_stacked_macro_rows() {
    let mut automation = AutomationState::default();
    let address = ControlAddress::new("macro.1");
    automation.open_or_create(address);
    automation.toggle_open_field(unit_key("macro.1", Some("lfo.amount")));

    assert_eq!(
        lfo_submenu_rows(&automation, address).len(),
        LfoField::ALL.len(),
        "macro sliders may have LFOs, but their LFO fields do not take nested macro routes"
    );
}

#[test]
fn macro_own_lfo_feeds_targets_in_the_same_pass() {
    let mut controls = FluidControls::default();
    controls.master.level = 0.0;
    controls.macros.values[0] = 0.0;
    let mut automation = AutomationState::default();
    automation.set_route(
        ControlAddress::new("macro.1"),
        LfoRoute {
            depth_ratio: 1.0,
            cycle_beats: 2.0,
            shape: LfoShape::Sine,
            ..LfoRoute::default()
        },
    );
    automation.set_macro_route(
        ControlAddress::new("master.level"),
        single_macro_route(0, 1.0),
    );

    // Sine peak: beat 0.5 of a 2-beat cycle. At 120 BPM that is 0.25 s.
    let sample = (f64::from(SAMPLE_RATE) * 0.25) as u64;
    let mut effective = controls.clone();
    apply_automation(&mut effective, &automation, timing(sample, 120.0));
    assert!(
        effective.macros.values[0] > 0.99,
        "macro slider should sit at its LFO peak, got {}",
        effective.macros.values[0]
    );
    assert!(
        effective.master.level > 0.99,
        "target should follow the modulated macro, got {}",
        effective.master.level
    );
}

#[test]
fn envelope_opens_only_on_macros_and_macros_never_target_macros() {
    let controls = FluidControls::default();
    let shared = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let mut automation = PublishedAutomation::new(AutomationState::default(), shared);
    let mut sub = 0usize;

    // e on a regular control: refused.
    let master_items = tab_controls(Tab::Master, &controls);
    open_modulator(
        &mut automation,
        &master_items,
        0,
        ModKind::Envelope,
        &mut sub,
    );
    assert!(!automation.state().is_editor_open());

    // v on a macro slider: refused.
    let macro_items = tab_controls(Tab::Macros, &controls);
    open_modulator(&mut automation, &macro_items, 0, ModKind::Macro, &mut sub);
    assert!(!automation.state().is_editor_open());

    // e on a macro slider and v on a regular control: allowed.
    open_modulator(
        &mut automation,
        &macro_items,
        0,
        ModKind::Envelope,
        &mut sub,
    );
    assert_eq!(automation.state().active_kind(), Some(ModKind::Envelope));
    automation.edit(AutomationState::close_editor);
    open_modulator(&mut automation, &master_items, 0, ModKind::Macro, &mut sub);
    assert_eq!(automation.state().active_kind(), Some(ModKind::Macro));
}

#[test]
fn engine_publishes_beat_telemetry() {
    let controls = Arc::new(ArcSwap::from_pointee(FluidControls::default()));
    let automation = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let telemetry = Arc::new(FluidTelemetry::default());
    let bpm = f64::from(controls.load().master.bpm);
    let mut engine = FluidEngine::new(
        44_100.0,
        controls,
        automation,
        no_morph(),
        Arc::clone(&telemetry),
    );

    for _ in 0..512 {
        engine.next_stereo();
    }

    let expected = 256.0 * bpm / (60.0 * 44_100.0);
    let beat = telemetry.beat();
    assert!(beat > 0.0);
    assert!(
        (beat - expected).abs() / expected < 0.01,
        "expected ~{expected}, got {beat}"
    );
}

// ============================================================
// Golden render — reproducibility guardrail
//
// `render --seed` is relied on to be byte-identical across releases (the
// release profile comment in Cargo.toml calls this out explicitly). Nothing
// enforced that before this test: it's the tripwire for any change to the
// per-sample DSP math (oscillators, decay curves, filter coefficients) that
// alters float rounding, even when the change is otherwise behavior-neutral.
// A deliberate DSP change (e.g. replacing a per-sample `exp()` with an
// equivalent closed-form recurrence) is expected to trip this — re-bless
// GOLDEN_RENDER_CHECKSUM only after confirming the new output is inaudibly
// close to the old one.
const GOLDEN_RENDER_SAMPLES: usize = 48_000;
// Re-blessed when tonal and arp were unified onto one attack+decay envelope
// (`attack_decay_gain`): every note now ramps in over `attack`, then decays
// from the peak to silence over `decay`, with the note's whole life = attack +
// decay and no step-clamped hold. This shifts both the tonal and arp voices in
// this render (tonal.level 0.5 + arp.gain 0.4); pad/bass paths are unchanged.
const GOLDEN_RENDER_CHECKSUM: u64 = 0x08f0_c949_89ea_81c5;

/// FNV-1a fold of one sample's bit pattern into a running hash. Hashing raw
/// bit patterns (not values) means any float divergence, including sub-ULP
/// rounding differences, changes the checksum.
fn fold_sample_bits(hash: u64, bits: u32) -> u64 {
    (hash ^ u64::from(bits)).wrapping_mul(0x100000001b3)
}

#[test]
fn golden_render_is_byte_identical_for_a_seed() {
    // Non-default tonal/arp levels and synth types so the render actually
    // exercises the piano voice's per-harmonic decay path, not just silence.
    let controls = Arc::new(ArcSwap::from_pointee(FluidControls {
        master: MasterControls {
            bpm: 140.0,
            ..MasterControls::default()
        },
        tonal: TonalControls {
            level: 0.5,
            synth_type: 2.0,
            rate_beats: 0.25,
            ..TonalControls::default()
        },
        arp: ArpControls {
            gain: 0.4,
            ..ArpControls::default()
        },
        ..FluidControls::default()
    }));
    let automation = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let telemetry = Arc::new(FluidTelemetry::default());
    let mut engine = FluidEngine::new(SAMPLE_RATE, controls, automation, no_morph(), telemetry);
    engine.reseed(42);

    let mut hash = 0xcbf2_9ce4_8422_2325u64; // FNV offset basis
    for _ in 0..GOLDEN_RENDER_SAMPLES {
        let (l, r) = engine.next_stereo();
        hash = fold_sample_bits(hash, l.to_bits());
        hash = fold_sample_bits(hash, r.to_bits());
    }

    assert_eq!(
        hash, GOLDEN_RENDER_CHECKSUM,
        "seeded render output changed — this either broke reproducibility \
         unintentionally, or is an expected result of a deliberate DSP change. \
         If the latter, re-bless GOLDEN_RENDER_CHECKSUM (checksum was {hash:#x})"
    );
}

#[test]
fn ambient_reverb_send_ducks_dry_sources_by_mix() {
    let mut send = AmbientReverbSend::new(SAMPLE_RATE);

    let frame = send.process((1.0, -1.0), (0.5, -0.5), (0.0, 0.0), 1.0, 0.5, 0.0);

    assert_near(frame.pad_l, AmbientReverbSend::dry_gain(1.0));
    assert_near(frame.pad_r, -AmbientReverbSend::dry_gain(1.0));
    assert_near(frame.tonal_l, 0.5 * AmbientReverbSend::dry_gain(0.5));
    assert_near(frame.tonal_r, -0.5 * AmbientReverbSend::dry_gain(0.5));
    assert_close(frame.wet_l, 0.0);
    assert_close(frame.wet_r, 0.0);
}

#[test]
fn full_pad_reverb_does_not_boost_pad_rms() {
    fn pad_rms(reverb_mix: f32) -> f32 {
        let controls = PadControls {
            reverb_mix,
            attack_time: 0.01,
            release_time: 0.1,
            ..PadControls::default()
        };
        let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));
        pad.rng = StdRng::seed_from_u64(7);
        let mut send = AmbientReverbSend::new(SAMPLE_RATE);
        let mut sum = 0.0;
        let mut count = 0;
        let total = SAMPLE_RATE as u64 * 4;
        let warmup = SAMPLE_RATE as u64;

        for sample in 0..total {
            let dry = pad.next(&controls, 0.0, timing(sample, 120.0));
            let frame = send.process(dry, (0.0, 0.0), (0.0, 0.0), controls.reverb_mix, 0.0, 0.0);
            if sample >= warmup {
                let l = frame.pad_l + frame.wet_l;
                let r = frame.pad_r + frame.wet_r;
                sum += l * l + r * r;
                count += 2;
            }
        }

        (sum / count as f32).sqrt()
    }

    let dry = pad_rms(0.0);
    let wet = pad_rms(1.0);

    assert!(
        wet <= dry * 1.05,
        "full reverb should not make pad much louder: dry rms {dry}, wet rms {wet}"
    );
}

#[test]
fn full_tonal_reverb_does_not_boost_tonal_rms() {
    fn tonal_rms(reverb_mix: f32) -> f32 {
        let controls = TonalControls {
            level: 0.8,
            randomness: 0.0,
            step_interval_beats: 4.0,
            reverb_mix,
            ..TonalControls::default()
        };
        let mut tonal = TonalEngine::new(SAMPLE_RATE);
        tonal.rng = StdRng::seed_from_u64(11);
        let mut send = AmbientReverbSend::new(SAMPLE_RATE);
        let mut sum = 0.0;
        let mut count = 0;
        let total = SAMPLE_RATE as u64 * 4;
        let warmup = SAMPLE_RATE as u64;

        for sample in 0..total {
            let dry = tonal.next(&controls, 0.0, timing(sample, 120.0));
            let frame = send.process((0.0, 0.0), dry, (0.0, 0.0), 0.0, controls.reverb_mix, 0.0);
            if sample >= warmup {
                let l = frame.tonal_l + frame.wet_l;
                let r = frame.tonal_r + frame.wet_r;
                sum += l * l + r * r;
                count += 2;
            }
        }

        (sum / count as f32).sqrt()
    }

    let dry = tonal_rms(0.0);
    let wet = tonal_rms(1.0);

    assert!(
        wet <= dry * 1.05,
        "full reverb should not make tonal much louder: dry rms {dry}, wet rms {wet}"
    );
}

#[test]
fn lfo_phase_at_uses_cycle_and_offset() {
    let route = LfoRoute {
        cycle_beats: 2.0,
        phase_offset_beats: 0.5,
        ..LfoRoute::default()
    };

    assert!((route.phase_at(1.0) - 0.75).abs() < 1e-9);
    assert!((route.phase_at(2.0) - 0.25).abs() < 1e-9);
}

#[test]
fn render_fluid_draws_lfo_submenu_and_animated_lane() {
    let controls = FluidControls::default();
    let fluid = FluidState::new();
    let items = tab_controls(Tab::Master, &controls);
    let mut automation = AutomationState::default();
    automation.open_or_create(ControlAddress::new(items[0].id));

    let draw_at = |beat: f64| {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render(
                    f,
                    &items,
                    Tab::Master,
                    0,
                    1,
                    beat,
                    NumericDisplay::empty(),
                    &fluid,
                    &automation,
                    &controls,
                    None,
                    &FlippedUnits::new(),
                    ChordDrill::None,
                    &[None; 9],
                )
            })
            .unwrap();
        terminal.backend().buffer().clone()
    };

    let at_start = draw_at(0.0);
    let text = buffer_text(&at_start);
    assert!(text.contains("amount"));
    assert!(text.contains("interval"));
    assert!(text.contains("offset"));
    assert!(text.contains("0%"));

    // Default cycle is 2 beats, so beat 1.0 is the opposite phase: the lane's
    // bright head has moved even though the 0% wave glyphs are flat.
    let at_half_cycle = draw_at(1.0);
    assert_ne!(at_start, at_half_cycle);
}

#[test]
fn lfo_lane_is_phase_locked() {
    let route = LfoRoute::default();

    let start = lfo_lane_line(&route, 0.0, 24, true);
    let same_phase = lfo_lane_line(&route, 2.0, 24, true);
    let opposite_phase = lfo_lane_line(&route, 1.0, 24, true);

    let styles =
        |line: &ratatui::text::Line<'_>| line.spans.iter().map(|s| s.style).collect::<Vec<_>>();
    assert_eq!(styles(&start), styles(&same_phase));
    assert_ne!(styles(&start), styles(&opposite_phase));
}

#[test]
fn automation_applies_bounded_lfo_offset_and_clamps_to_spec_range() {
    let mut controls = FluidControls::default();
    controls.master.level = 0.9;
    let mut automation = AutomationState::default();
    let route = automation.open_or_create(ControlAddress::new("master.level"));
    route.depth_ratio = 0.5;

    apply_automation(
        &mut controls,
        &automation,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 0.5),
    );

    assert_close(controls.master.level, 1.0);
}

#[test]
fn automation_uses_beat_cycle_phase_for_opposite_lfo_offsets() {
    let mut automation = AutomationState::default();
    let route = automation.open_or_create(ControlAddress::new("master.level"));
    route.cycle_beats = 2.0;
    route.depth_ratio = 0.25;

    let mut positive = FluidControls::default();
    positive.master.level = 0.5;
    apply_automation(
        &mut positive,
        &automation,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 0.5),
    );

    let mut negative = FluidControls::default();
    negative.master.level = 0.5;
    apply_automation(
        &mut negative,
        &automation,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 1.5),
    );

    assert_near(positive.master.level, 0.75);
    assert_near(negative.master.level, 0.25);
}

#[test]
fn automation_preserves_base_controls_and_modulates_only_effective_clone() {
    let mut base = FluidControls::default();
    base.master.level = 0.5;
    let mut effective = base.clone();
    let mut automation = AutomationState::default();
    let route = automation.open_or_create(ControlAddress::new("master.level"));
    route.depth_ratio = 0.25;

    apply_automation(
        &mut effective,
        &automation,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 0.5),
    );

    assert_near(effective.master.level, 0.75);
    assert_close(base.master.level, 0.5);
}

#[test]
fn defaults_match_current_mix() {
    let controls = FluidControls::default();

    assert_close(controls.master.bpm, 82.0);
    assert_close(controls.master.drive, 0.1);
    assert_close(controls.master.comp_threshold, -8.0);

    assert_close(controls.perc.decay_ms, 200.0);
    assert_close(controls.perc.filter, 0.7);
    assert_close(controls.perc.interval_beats, 0.25);
    assert_close(controls.perc.offset_beats, 0.0);

    assert_close(controls.kick.start_freq, 160.0);
    assert_close(controls.kick.pitch_decay_ms, 55.0);
    assert_close(controls.kick.amp_decay_ms, 250.0);

    assert_close(controls.tonal.phrase, 0.0);
    assert_close(controls.tonal.synth_type, 0.0);
    assert_close(controls.tonal.rate_beats, 0.5);
    assert_close(controls.tonal.step_interval_beats, 16.0);
    assert_close(controls.tonal.decay, 1.2);
    assert_close(controls.tonal.randomness, 0.5);
    assert_close(controls.tonal.evolve_rate, 0.0);

    assert_close(controls.clap.room, 0.0);
}

#[test]
fn apply_min_moves_selected_control_to_floor() {
    let mut controls = FluidControls::default();

    controls.master.drive = 0.8;
    apply_min(Tab::Master, 9, &mut controls);
    assert_close(controls.master.drive, 0.0);

    controls.master.bpm = 120.0;
    apply_min(Tab::Master, 7, &mut controls);
    assert_close(controls.master.bpm, MASTER_BPM_MIN);

    controls.master.tone = 0.5;
    apply_min(Tab::Master, 13, &mut controls);
    assert_close(controls.master.tone, -1.0);

    controls.pad.chord_bars = 16.0;
    apply_min(Tab::Chords, 4, &mut controls);
    assert_close(controls.pad.chord_bars, 1.0);
}

#[test]
fn apply_value_accepts_percent_style_unit_controls() {
    let mut controls = FluidControls::default();

    apply_value(Tab::Master, 8, 42.0, &mut controls);
    assert_close(controls.master.level, 0.42);

    // Typed entry is always a plain percent integer, never a pre-divided
    // ratio: 1 means 1%, not 100%.
    apply_value(Tab::Master, 8, 1.0, &mut controls);
    assert_close(controls.master.level, 0.01);
}

#[test]
fn apply_value_snaps_direct_numeric_entry_to_control_grid() {
    let mut controls = FluidControls::default();

    apply_value(Tab::Kick, 4, 1.13, &mut controls);
    assert_close(controls.kick.interval_beats, 1.25);

    apply_value(Tab::Kick, 4, 0.16, &mut controls);
    assert_close(controls.kick.interval_beats, 0.125);

    apply_value(Tab::Chords, 4, 12.0, &mut controls);
    assert_close(controls.pad.chord_bars, 4.0);

    apply_value(Tab::Clap, 5, 3.6, &mut controls);
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
                Gain, Gain, Gain, Gain, Gain, Gain, Gain, Timing, Gain, Gain, Continuous,
                Continuous, Timing, Continuous, Discrete,
            ],
        ),
        (Tab::Perc, vec![Gain, Gain, Timing, Timing, Timing, Gain]),
        (
            Tab::Chords,
            vec![
                Gain, Timing, Timing, Discrete, Timing, Discrete, Discrete, Gain, Gain, Gain, Gain,
                Discrete, Discrete, Discrete, Discrete, Discrete, Discrete, Discrete, Discrete,
                Discrete, Discrete, Discrete, Discrete, Discrete, Discrete, Discrete, Discrete,
                Discrete, Discrete, Discrete, Discrete, Discrete, Discrete, Discrete, Discrete,
                Discrete, Discrete, Discrete, Discrete, Discrete, Discrete, Discrete, Discrete,
            ],
        ),
        (
            Tab::Bass,
            vec![
                Gain, Continuous, Timing, Timing, Discrete, Timing, Timing, Discrete, Discrete,
                Gain,
            ],
        ),
        (
            Tab::Kick,
            vec![
                Gain, Gain, Timing, Timing, Timing, Timing, Continuous, Gain, Gain,
            ],
        ),
        (
            Tab::Tonal,
            vec![
                Gain, Timing, Timing, Discrete, Discrete, Discrete, Timing, Timing, Timing, Gain,
                Gain, Continuous, Gain,
            ],
        ),
        (
            Tab::Clap,
            vec![
                Gain, Gain, Timing, Timing, Timing, Discrete, Timing, Gain, Gain,
            ],
        ),
        (
            Tab::Arp,
            vec![
                Gain, Timing, Timing, Discrete, Timing, Timing, Gain, Discrete, Discrete, Gain,
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
        Tab::Arp,
    ];
    for tab in tabs {
        for spec in tab_specs(tab) {
            let ctx = format!("{} / {}", tab.name(), spec.label);
            assert!(!spec.id.is_empty(), "{ctx}: empty stable id");
            assert!(!spec.label.is_empty(), "{ctx}: empty label");
            assert!(spec.min < spec.max, "{ctx}: min must be below max");
            assert!(
                spec.reset >= spec.min && spec.reset <= spec.max,
                "{ctx}: reset outside [min, max]"
            );
            if spec.taper == Taper::Log2 {
                assert!(spec.min > 0.0, "{ctx}: log taper needs positive min");
            }
            if let Taper::Exp(n) = spec.taper {
                assert!(n > 0.0, "{ctx}: exp taper needs a positive exponent");
                assert!(spec.min >= 0.0, "{ctx}: exp taper needs a non-negative min");
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
fn song_code_round_trips_quantized_snapshot_values() {
    let mut controls = FluidControls::default();
    controls.master.bpm = 123.4;
    controls.pad.chord_bars = 12.0;
    controls.clap.slap_count = 6.6;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.master.bpm, 123.0);
    assert_close(decoded.controls.pad.chord_bars, 16.0);
    assert_close(decoded.controls.clap.slap_count, 7.0);
}

#[test]
fn song_code_round_trips_a_custom_progression() {
    let mut controls = FluidControls::default();
    controls.pad.progression = CUSTOM_PROGRESSION_INDEX as f32;
    controls.pad.chord_count = 3.0;
    controls.pad.chord_slots[0].degree = 2.0;
    controls.pad.chord_slots[0].accidental = -1.0;
    controls.pad.chord_slots[0].extension = 2.0;
    controls.pad.chord_slots[0].inversion = 1.0;
    controls.pad.chord_slots[2].degree = -3.0;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(
        decoded.controls.pad.progression,
        CUSTOM_PROGRESSION_INDEX as f32,
    );
    assert_close(decoded.controls.pad.chord_count, 3.0);
    assert_close(decoded.controls.pad.chord_slots[0].degree, 2.0);
    assert_close(decoded.controls.pad.chord_slots[0].accidental, -1.0);
    assert_close(decoded.controls.pad.chord_slots[0].extension, 2.0);
    assert_close(decoded.controls.pad.chord_slots[0].inversion, 1.0);
    assert_close(decoded.controls.pad.chord_slots[2].degree, -3.0);
}

/// A song code encoded before this feature existed simply never wrote the
/// `pad.chordN_*`/`pad.chord_count` ids (the generic snapshot codec only
/// writes ids that differ from default) — so it decodes today with those
/// fields at their defaults, no format migration required.
#[test]
fn song_code_predating_custom_progression_decodes_with_defaults() {
    let controls = FluidControls::default();
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let encoded = code.strip_prefix("n1_").unwrap();
    let bytes = URL_SAFE_NO_PAD.decode(encoded).unwrap();

    assert!(
        !bytes
            .windows(b"pad.chord1_degree".len())
            .any(|window| window == b"pad.chord1_degree")
    );

    let decoded = song::decode_song_code(&code).unwrap();
    assert_close(decoded.controls.pad.progression, 0.0);
    assert_close(decoded.controls.pad.chord_count, 8.0);
    assert_close(decoded.controls.pad.chord_slots[0].degree, 0.0);
}

/// End-to-end: a custom progression configured via song code, with pad,
/// bass, and arp all turned on, renders several seconds through the full
/// engine without panicking or producing non-finite samples.
#[test]
fn full_engine_renders_a_custom_progression_from_song_code_without_panicking() {
    let mut controls = FluidControls::default();
    controls.pad.progression = CUSTOM_PROGRESSION_INDEX as f32;
    controls.pad.chord_count = 5.0;
    controls.pad.chord_bars = 1.0;
    for (slot, degree) in [2.0, -2.0, 4.0, -4.0, 6.0].into_iter().enumerate() {
        controls.pad.chord_slots[slot].degree = degree;
        controls.pad.chord_slots[slot].accidental = if slot % 2 == 0 { 1.0 } else { -1.0 };
        controls.pad.chord_slots[slot].extension = (slot % 4) as f32;
        controls.pad.chord_slots[slot].inversion = (slot % 3) as f32;
    }
    controls.bass.level = 0.5;
    controls.arp.gain = 0.5;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();
    assert_close(
        decoded.controls.pad.progression,
        CUSTOM_PROGRESSION_INDEX as f32,
    );

    let controls_swap = Arc::new(ArcSwap::from_pointee(decoded.controls));
    let automation = Arc::new(ArcSwap::from_pointee(decoded.automation));
    let telemetry = Arc::new(FluidTelemetry::default());
    let mut engine = FluidEngine::new(
        SAMPLE_RATE,
        controls_swap,
        automation,
        no_morph(),
        telemetry,
    );

    for _ in 0..(SAMPLE_RATE as usize * 4) {
        let (l, r) = engine.next_stereo();
        assert!(
            l.is_finite() && r.is_finite(),
            "engine produced non-finite output"
        );
    }
}

#[test]
fn song_code_round_trips_bass_type() {
    let mut controls = FluidControls::default();
    controls.bass.voice_type = 2.0;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.bass.voice_type, 2.0);
}

#[test]
fn song_code_predating_bass_type_decodes_as_default_sub() {
    // A code written before `bass.type` existed simply omits the id; the
    // generic snapshot codec (keyed by durable id, not position) already
    // decodes any missing id to its default, same as every other control.
    let controls = FluidControls::default();
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.bass.voice_type, 0.0);
}

#[test]
fn song_code_round_trips_pad_type() {
    let mut controls = FluidControls::default();
    controls.pad.voice_type = 2.0;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.pad.voice_type, 2.0);
}

#[test]
fn song_code_predating_pad_type_decodes_as_default_warm() {
    // Same generic id->f32 snapshot codec as bass.type: a code written
    // before `pad.type` existed simply omits the id and decodes to default.
    let controls = FluidControls::default();
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.pad.voice_type, 0.0);
}

#[test]
fn song_code_round_trips_bass_cutoff() {
    let mut controls = FluidControls::default();
    controls.bass.cutoff = 500.0;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.bass.cutoff, 500.0);
}

#[test]
fn song_code_predating_bass_cutoff_decodes_as_default_open() {
    // Same generic id->f32 snapshot codec as bass.type/pad.type: a code
    // written before `bass.cutoff` existed simply omits the id and decodes
    // to the fully-open default (BASS_CUTOFF_MAX_HZ), preserving the
    // pre-existing sound.
    let controls = FluidControls::default();
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.bass.cutoff, BASS_CUTOFF_MAX_HZ);
}

#[test]
fn song_code_round_trips_tonal_octave() {
    let mut controls = FluidControls::default();
    controls.tonal.octave = -1.0;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.tonal.octave, -1.0);
}

#[test]
fn song_code_predating_tonal_octave_decodes_as_default_zero() {
    // Same generic id->f32 snapshot codec as bass.cutoff/bass.type/pad.type: a
    // code written before `tonal.octave` existed simply omits the id and
    // decodes to the no-shift default (0.0), preserving the pre-existing
    // sound.
    let controls = FluidControls::default();
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.tonal.octave, 0.0);
}

#[test]
fn song_code_round_trips_arp_reverb_mix() {
    let mut controls = FluidControls::default();
    controls.arp.reverb_mix = 0.9;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.arp.reverb_mix, 0.9);
}

#[test]
fn song_code_predating_arp_reverb_mix_decodes_as_default_fixed_mix() {
    // Same generic id->f32 snapshot codec as tonal.octave/bass.cutoff: a code
    // written before `arp.reverb_mix` existed simply omits the id and decodes
    // to the former fixed-mix default (0.5), preserving the pre-existing
    // sound.
    let controls = FluidControls::default();
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.arp.reverb_mix, 0.5);
}

#[test]
fn song_code_round_trips_arp_offset_beats() {
    let mut controls = FluidControls::default();
    controls.arp.offset_beats = 1.5;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.arp.offset_beats, 1.5);
}

#[test]
fn song_code_predating_arp_offset_beats_decodes_as_default_zero() {
    // Same generic id->f32 snapshot codec as arp.reverb_mix/tonal.octave: a
    // code written before `arp.offset_beats` existed simply omits the id and
    // decodes to the no-shift default (0.0), preserving the pre-existing
    // trigger phase.
    let controls = FluidControls::default();
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.arp.offset_beats, 0.0);
}

#[test]
fn song_code_decodes_missing_controls_as_defaults() {
    let mut controls = FluidControls::default();
    controls.master.bpm = 120.0;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(
        decoded.controls.pad.level,
        FluidControls::default().pad.level,
    );
}

#[test]
fn song_code_round_trips_tonal_attack_and_decay() {
    let mut controls = FluidControls::default();
    controls.tonal.attack = 0.2;
    controls.tonal.decay = 1.5;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.tonal.attack, 0.2);
    assert_close(decoded.controls.tonal.decay, 1.5);
}

/// A song code encoded before tonal.attack/tonal.decay existed simply has
/// no entries for those ids; the generic id->f32 snapshot format (unchanged
/// since it predates this control) already decodes them as defaults with no
/// version bump required.
#[test]
fn song_code_predating_tonal_attack_decay_decodes_with_defaults() {
    let code = song::encode_song_code(&SongState::default()).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(
        decoded.controls.tonal.attack,
        TonalControls::default().attack,
    );
    assert_close(decoded.controls.tonal.decay, TonalControls::default().decay);
}

#[test]
fn song_code_decodes_snapshot_only_payload_with_empty_automation() {
    let mut controls = FluidControls::default();
    controls.master.bpm = 120.0;
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();

    let decoded = song::decode_song_code(&code).unwrap();

    assert_eq!(decoded.automation.routes().count(), 0);
}

#[test]
fn song_code_round_trips_lfo_automation_record() {
    let mut controls = FluidControls::default();
    controls.master.level = 0.6;
    let mut automation = AutomationState::default();
    automation.set_route(
        ControlAddress::new("master.level"),
        LfoRoute {
            cycle_beats: 4.0,
            depth_ratio: 0.4,
            shape: LfoShape::Sine,
            phase_offset_beats: 0.25,
            ..LfoRoute::default()
        },
    );
    let song = SongState {
        controls,
        automation,
    };

    let code = song::encode_song_code(&song).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();
    let route = decoded
        .automation
        .route(ControlAddress::new("master.level"))
        .unwrap();

    assert_close(decoded.controls.master.level, 0.6);
    assert_close(route.cycle_beats, 4.0);
    assert_close(route.depth_ratio, 0.4);
    assert_eq!(route.shape, LfoShape::Sine);
    assert_close(route.phase_offset_beats, 0.25);
}

#[test]
fn song_code_skips_unknown_records() {
    let mut controls = FluidControls::default();
    controls.master.tune = 5.0;
    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let code = append_record_to_code(&code, 99, &[1, 2, 3, 4]);

    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.master.tune, 5.0);
}

#[test]
fn song_code_skips_unknown_control_ids() {
    let code = song::encode_song_code(&SongState::default()).unwrap();
    let mut payload = Vec::new();
    let id = b"future.control.id";
    payload.extend_from_slice(&1u16.to_le_bytes());
    payload.push(id.len() as u8);
    payload.extend_from_slice(id);
    payload.extend_from_slice(&0.75f32.to_le_bytes());
    let code = append_record_to_code(&code, song::SNAPSHOT_RECORD, &payload);

    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(
        decoded.controls.master.level,
        FluidControls::default().master.level,
    );
}

#[test]
fn song_code_skips_unknown_automation_target_control_ids() {
    let code = song::encode_song_code(&SongState::default()).unwrap();
    let payload = automation_payload(
        "future.control.id",
        LfoRoute {
            depth_ratio: 0.2,
            ..LfoRoute::default()
        },
    );
    let code = append_record_to_code(&code, song::AUTOMATION_RECORD, &payload);

    let decoded = song::decode_song_code(&code).unwrap();

    assert_eq!(decoded.automation.routes().count(), 0);
}

#[test]
fn launch_line_is_cli_launchable() {
    let line = launch_line(&SongState::default()).unwrap();

    assert!(line.starts_with("nooise n1_"));
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

    assert_near(smoother.next(), 0.028);
    for _ in 0..4 {
        smoother.next();
    }
    assert_near(smoother.current, 0.5);
    for _ in 0..4 {
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
    controls.kick.click = 0.0;
    controls.kick.drive = 0.0;
    controls.kick.filter = 0.0;
    controls.tonal.randomness = 0.0;
    controls.clap.filter = 0.5;
    controls.clap.body = 0.0;
    controls.master.level = 0.0;
    controls.master.drive = 0.0;
    controls.bass.drive = 0.0;

    let mut smoothers = GainSmoothers::new(&controls);
    controls.pad.level = 1.0;
    controls.pad.reverb_mix = 1.0;
    controls.perc.filter = 1.0;
    controls.kick.click = 0.2;
    controls.kick.drive = 1.0;
    controls.kick.filter = 1.0;
    controls.tonal.randomness = 1.0;
    controls.clap.filter = 1.0;
    controls.clap.body = 1.0;
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
    assert!(next.kick.click > 0.0 && next.kick.click < 0.2);
    assert!(next.kick.drive > 0.0 && next.kick.drive < 1.0);
    assert!(next.kick.filter > 0.0 && next.kick.filter < 1.0);
    assert!(next.tonal.randomness > 0.0 && next.tonal.randomness < 1.0);
    assert!(next.clap.filter > 0.5 && next.clap.filter < 1.0);
    assert!(next.clap.body > 0.0 && next.clap.body < 1.0);
    assert!(next.master.level > 0.0 && next.master.level < 0.5);
    assert!(next.master.drive > 0.0 && next.master.drive < 1.0);
    assert!(next.bass.drive > 0.0 && next.bass.drive < 1.0);
}

#[test]
fn gain_smoothers_cover_every_unique_gain_spec() {
    let controls = FluidControls::default();
    let smoothers = GainSmoothers::new(&controls);
    let expected: std::collections::BTreeSet<_> = all_specs()
        .filter(|spec| spec.kind == ControlKind::Gain)
        .map(|spec| spec.id)
        .collect();
    let actual: std::collections::BTreeSet<_> = smoothers
        .smoothers
        .iter()
        .map(|smoother| smoother.spec.unwrap().id)
        .collect();

    assert_eq!(actual, expected);
}

#[test]
fn chords_tab_shows_progression_row_with_letter_display() {
    let mut controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[5].label, "Chord Count");
    assert_eq!(rows[5].display, "8");
    assert_eq!(rows[6].label, "Progression");
    assert_eq!(rows[6].display, "A");

    controls.pad.progression = 2.0;
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[6].display, "C");

    controls.pad.progression = CUSTOM_PROGRESSION_INDEX as f32;
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[6].display, "Custom");
}

#[test]
fn tonal_tab_separates_rate_from_cycle() {
    let rows = tab_controls(Tab::Tonal, &FluidControls::default());

    assert_eq!(rows[3].id, "tonal.synth_type");
    assert_eq!(rows[3].label, "Type");
    assert_eq!(rows[3].display, "Sine");
    assert_eq!(rows[4].id, "tonal.octave");
    assert_eq!(rows[4].label, "Octave");
    assert_eq!(rows[5].id, "tonal.phrase");
    assert_eq!(rows[5].label, "Phrase");
    assert_eq!(rows[6].id, "tonal.rate_beats");
    assert_eq!(rows[6].label, "Rate");
    assert_eq!(rows[6].display, "0.50 beats");
    assert_eq!(rows[7].id, "tonal.step_interval_beats");
    assert_eq!(rows[7].label, "Cycle");
    assert_eq!(rows[7].display, "16.00 beats");
}

#[test]
fn chords_progression_adjusts_and_clamps() {
    let mut controls = FluidControls::default();

    apply_delta(Tab::Chords, 6, 1.0, &mut controls);
    assert_close(controls.pad.progression, 1.0);

    controls.pad.progression = CUSTOM_PROGRESSION_INDEX as f32;
    apply_delta(Tab::Chords, 6, 1.0, &mut controls);
    assert_close(controls.pad.progression, CUSTOM_PROGRESSION_INDEX as f32);

    controls.pad.progression = 0.0;
    apply_delta(Tab::Chords, 6, -1.0, &mut controls);
    assert_close(controls.pad.progression, 0.0);

    controls.pad.progression = 2.0;
    apply_min(Tab::Chords, 6, &mut controls);
    assert_close(controls.pad.progression, 0.0);
}

#[test]
fn chords_tab_controls_none_shows_only_base_params() {
    let controls = FluidControls::default();
    let rows = chords_tab_controls(&controls, ChordDrill::None);
    assert_eq!(rows.len(), 11);
    assert_eq!(rows[0].id, "pad.level");
    assert_eq!(rows[6].id, "pad.progression");
    assert_eq!(rows[2].id, "pad.release_time");
    assert!(rows.iter().all(|r| !r.label.contains("Root")));
}

#[test]
fn chords_tab_controls_progression_lists_active_slot_roots() {
    let mut controls = FluidControls::default();

    controls.pad.chord_count = 3.0;
    let rows = chords_tab_controls(&controls, ChordDrill::Progression);
    assert_eq!(
        rows.iter().map(|r| r.label).collect::<Vec<_>>(),
        vec!["Chord 1 Root", "Chord 2 Root", "Chord 3 Root"]
    );

    controls.pad.chord_count = 8.0;
    let rows = chords_tab_controls(&controls, ChordDrill::Progression);
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[7].label, "Chord 8 Root");
}

#[test]
fn chords_tab_controls_slot_shows_accidental_extension_inversion() {
    let controls = FluidControls::default();
    let rows = chords_tab_controls(&controls, ChordDrill::Slot(2));
    assert_eq!(
        rows.iter().map(|r| r.label).collect::<Vec<_>>(),
        vec![
            "Chord 3 Accidental",
            "Chord 3 Extension",
            "Chord 3 Inversion"
        ]
    );
}

#[test]
fn chords_flat_index_maps_visible_rows_to_chords_controls_indices() {
    assert_eq!(chords_flat_index(ChordDrill::None, 4), 4);
    assert_eq!(chords_flat_index(ChordDrill::Progression, 0), 11);
    assert_eq!(chords_flat_index(ChordDrill::Progression, 2), 19);
    assert_eq!(chords_flat_index(ChordDrill::Slot(2), 0), 20);

    let controls = FluidControls::default();
    let expected = tab_controls(Tab::Chords, &controls)[20].id;
    assert_eq!(expected, "pad.chord3_accidental");
}

#[test]
fn chords_footer_signals_drill_depth() {
    assert_eq!(chords_footer(Tab::Chords, ChordDrill::None), None);
    assert_eq!(chords_footer(Tab::Master, ChordDrill::Progression), None);
    assert_eq!(
        chords_footer(Tab::Chords, ChordDrill::Progression),
        Some("Progression   Enter: open chord   Esc: back".to_string())
    );
    assert_eq!(
        chords_footer(Tab::Chords, ChordDrill::Slot(2)),
        Some("Chord 3   Esc: back".to_string())
    );
}

#[test]
fn render_fluid_shows_chords_drill_breadcrumb_and_footer() {
    let controls = FluidControls::default();
    let fluid = FluidState::new();
    let automation = AutomationState::default();
    let rows = chords_tab_controls(&controls, ChordDrill::Slot(1));
    let footer = chords_footer(Tab::Chords, ChordDrill::Slot(1));

    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render(
                f,
                &rows,
                Tab::Chords,
                0,
                0,
                0.0,
                NumericDisplay::empty(),
                &fluid,
                &automation,
                &controls,
                footer.as_deref(),
                &FlippedUnits::new(),
                ChordDrill::Slot(1),
                &[None; 9],
            )
        })
        .unwrap();

    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("Chords › Chord 2"));
    assert!(text.contains("Chord 2   Esc: back"));
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
    let pad = PadControls::default();
    assert_eq!(bass_root_note(0, 0, &pad), 45);
    // Progression A's authored line diverges from the chord's lowest
    // tone at step 3 (B chord's min is 47) — proves the bass line is
    // independent data, not derived from PROGRESSIONS.
    assert_eq!(bass_root_note(0, 3, &pad), 43);
    assert_eq!(bass_root_note(2, 3, &pad), 43);
}

#[test]
fn bass_root_note_follows_custom_chord_slot_root_when_selected() {
    let mut pad = PadControls::default();
    pad.chord_slots[3].degree = -1.0;
    pad.chord_slots[3].accidental = 1.0;

    let root = bass_root_note(CUSTOM_PROGRESSION_INDEX, 3, &pad);
    assert_eq!(root, pad_chord_root_note(&pad.chord_slots[3]));
}

#[test]
fn bass_defaults_are_silent_quarter_note_a() {
    let controls = BassControls::default();
    assert_close(controls.level, 0.0);
    assert_close(controls.voice_type, 0.0);
    assert_close(controls.rhythm, 0.0);
    assert_close(controls.octave, -1.0);
    assert_close(controls.interval_beats, 4.0);
}

#[test]
fn bass_tab_shows_type_and_rhythm_rows_with_letter_display() {
    let mut controls = FluidControls::default();
    let rows = tab_controls(Tab::Bass, &controls);
    assert_eq!(rows[4].id, "bass.type");
    assert_eq!(rows[4].label, "Type");
    assert_eq!(rows[4].display, "Sub");
    assert_eq!(rows[7].label, "Rhythm");
    assert_eq!(rows[7].display, "A");

    controls.bass.voice_type = 1.0;
    let rows = tab_controls(Tab::Bass, &controls);
    assert_eq!(rows[4].display, "Saw");

    controls.bass.voice_type = 2.0;
    let rows = tab_controls(Tab::Bass, &controls);
    assert_eq!(rows[4].display, "Pluck");

    controls.bass.rhythm = 3.0;
    let rows = tab_controls(Tab::Bass, &controls);
    assert_eq!(rows[7].display, "D");
}

#[test]
fn bass_controls_adjust_and_clamp() {
    let mut controls = FluidControls::default();

    apply_delta(Tab::Bass, 7, 1.0, &mut controls);
    assert_close(controls.bass.rhythm, 1.0);

    controls.bass.rhythm = 3.0;
    apply_delta(Tab::Bass, 7, 1.0, &mut controls);
    assert_close(controls.bass.rhythm, 3.0);

    controls.bass.octave = -1.0;
    apply_delta(Tab::Bass, 8, -1.0, &mut controls);
    apply_delta(Tab::Bass, 8, -1.0, &mut controls);
    assert_close(controls.bass.octave, -3.0);

    apply_min(Tab::Bass, 0, &mut controls);
    assert_close(controls.bass.level, 0.0);

    controls.bass.decay_time = 0.4;
    apply_delta(Tab::Bass, 3, 1.0, &mut controls);
    assert!(controls.bass.decay_time > 0.4);

    apply_min(Tab::Bass, 3, &mut controls);
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
fn bass_engine_is_monophonic_and_hard_cuts_on_retrigger() {
    let sample_rate = 48_000.0;
    let mut bass = BassEngine::new(sample_rate);
    let pad = PadControls::default();
    let bass_controls = BassControls {
        interval_beats: 1.0,
        rhythm: 0.0,     // quarter notes: hits every beat
        decay_time: 5.0, // deliberately long: a pool would still be ringing
        attack_time: 0.001,
        ..BassControls::default()
    };
    let mut clock = TempoClock::new(sample_rate, 120.0);

    // 120bpm quarter notes land every 0.5s; run long enough to guarantee at
    // least two hits plus the short anti-click fade window has fully elapsed
    // after the second one.
    for _ in 0..(sample_rate * 1.2) as usize {
        let timing = clock.tick(120.0);
        bass.next(&bass_controls, &pad, 0.0, timing);
    }

    // Only one active voice remains: the previous hit's fade-out slot has
    // already rung fully silent, regardless of the 5s decay_time — that's
    // the mono guarantee (no audible overlap between consecutive hits).
    assert!(bass.voice.is_some());
    assert!(bass.fading_voice.is_none());
}

#[test]
fn bass_voice_decays_to_silence_without_sustaining() {
    let sample_rate = 48_000.0;
    let mut voice = BassVoice::new(0, 110.0, 0.005, 0.05, 0.0, sample_rate);

    // Well past attack+decay (0.055s); a sustaining envelope would still
    // be holding at ~0.85 gain here, an AD envelope has decayed to ~0.
    for _ in 0..(sample_rate * 0.5) as usize {
        voice.next();
    }

    assert!(voice.next().abs() < 0.001);
}

#[test]
fn bass_type_zero_matches_legacy_sub_voice_exactly() {
    let sample_rate = 48_000.0;
    let mut dispatched = BassVoice::new(0, 110.0, 0.01, 0.05, 0.15, sample_rate);
    let mut legacy = SubBassVoice::new(110.0, 0.01, 0.05, 0.15, sample_rate);

    for _ in 0..(sample_rate * 0.3) as usize {
        assert_eq!(dispatched.next(), legacy.next());
    }
}

#[test]
fn bass_types_produce_differing_but_comparably_balanced_audio() {
    let sample_rate = 48_000.0;
    let samples = (sample_rate * 0.4) as usize;

    let rms = |voice_type: usize| -> f32 {
        let mut voice = BassVoice::new(voice_type, 110.0, 0.01, 0.3, 0.0, sample_rate);
        let mut sum_sq = 0.0f32;
        for _ in 0..samples {
            let s = voice.next();
            sum_sq += s * s;
        }
        (sum_sq / samples as f32).sqrt()
    };

    let sub_rms = rms(0);
    let saw_rms = rms(1);
    let pluck_rms = rms(2);

    // Each character actually differs in level (not exactly zero would be a
    // trivially "different" but useless voice).
    assert!(sub_rms > 0.0 && saw_rms > 0.0 && pluck_rms > 0.0);

    // Types must sound different from one another, not just be scaled
    // copies produced by the shared drive/panner tail; sample a few points
    // in the decay and confirm at least one differs beyond float noise.
    let mut sub = BassVoice::new(0, 110.0, 0.01, 0.3, 0.0, sample_rate);
    let mut saw = BassVoice::new(1, 110.0, 0.01, 0.3, 0.0, sample_rate);
    let mut pluck = BassVoice::new(2, 110.0, 0.01, 0.3, 0.0, sample_rate);
    let mut any_diff_sub_saw = false;
    let mut any_diff_sub_pluck = false;
    for _ in 0..samples {
        let sl = sub.next();
        let wl = saw.next();
        let pl = pluck.next();
        if (sl - wl).abs() > 1e-6 {
            any_diff_sub_saw = true;
        }
        if (sl - pl).abs() > 1e-6 {
            any_diff_sub_pluck = true;
        }
    }
    assert!(any_diff_sub_saw);
    assert!(any_diff_sub_pluck);

    // Authored gains keep the three characters at a comparable perceived
    // level: no type should be more than 2x (~6 dB) louder or quieter than
    // the others.
    let levels = [sub_rms, saw_rms, pluck_rms];
    let max = levels.iter().cloned().fold(f32::MIN, f32::max);
    let min = levels.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        max / min < 2.0,
        "bass types not level-matched: sub={sub_rms}, saw={saw_rms}, pluck={pluck_rms}"
    );
}

#[test]
fn pad_type_zero_matches_legacy_warm_tone_exactly() {
    let sample_rate = 48_000.0;
    let mut dispatched = PadTone::new(0, 220.0, 0.2, 0.15, 0.5, 1.0, sample_rate);
    let mut legacy = WarmPadTone::new(220.0, 0.2, 0.15, 0.5, 1.0, sample_rate);

    for _ in 0..(sample_rate * 0.3) as usize {
        assert_eq!(
            dispatched.next_stereo(0.8, 0.5, 0.5),
            legacy.next_stereo(0.8, 0.5, 0.5)
        );
    }
}

#[test]
fn pad_types_produce_differing_but_comparably_balanced_audio() {
    let sample_rate = 48_000.0;
    let samples = (sample_rate * 0.4) as usize;

    let rms = |character: usize| -> f32 {
        let mut tone = PadTone::new(character, 220.0, 0.0, 0.15, 0.05, 1.0, sample_rate);
        let mut sum_sq = 0.0f32;
        for _ in 0..samples {
            let (l, r) = tone.next_stereo(0.8, 0.5, 0.5);
            sum_sq += l * l + r * r;
        }
        (sum_sq / (samples as f32 * 2.0)).sqrt()
    };

    let warm_rms = rms(0);
    let dark_rms = rms(1);
    let glass_rms = rms(2);

    assert!(warm_rms > 0.0 && dark_rms > 0.0 && glass_rms > 0.0);

    let mut warm = PadTone::new(0, 220.0, 0.0, 0.15, 0.05, 1.0, sample_rate);
    let mut dark = PadTone::new(1, 220.0, 0.0, 0.15, 0.05, 1.0, sample_rate);
    let mut glass = PadTone::new(2, 220.0, 0.0, 0.15, 0.05, 1.0, sample_rate);
    let mut any_diff_warm_dark = false;
    let mut any_diff_warm_glass = false;
    for _ in 0..samples {
        let (wl, _) = warm.next_stereo(0.8, 0.5, 0.5);
        let (dl, _) = dark.next_stereo(0.8, 0.5, 0.5);
        let (gl, _) = glass.next_stereo(0.8, 0.5, 0.5);
        if (wl - dl).abs() > 1e-6 {
            any_diff_warm_dark = true;
        }
        if (wl - gl).abs() > 1e-6 {
            any_diff_warm_glass = true;
        }
    }
    assert!(any_diff_warm_dark);
    assert!(any_diff_warm_glass);

    // Authored gains keep the three characters at a comparable perceived
    // level: no type should be more than 2x (~6 dB) louder or quieter than
    // the others.
    let levels = [warm_rms, dark_rms, glass_rms];
    let max = levels.iter().cloned().fold(f32::MIN, f32::max);
    let min = levels.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        max / min < 2.0,
        "pad types not level-matched: warm={warm_rms}, dark={dark_rms}, glass={glass_rms}"
    );
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
fn chords_reverb_mix_row_shifted_to_index_four() {
    let controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[7].label, "Reverb Mix");
}

#[test]
fn chords_release_row_present_with_lowered_attack_floor() {
    let controls = FluidControls::default();
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[1].label, "Attack");
    assert_close(rows[1].min, 0.05);
    assert_eq!(rows[2].label, "Release");
    assert_close(rows[2].value, 8.0);
    assert_close(rows[2].min, 0.05);
    assert_close(rows[2].max, 20.0);
}

#[test]
fn chords_attack_and_release_adjust_and_clamp_low() {
    let mut controls = FluidControls::default();

    // Exp-tapered dials step in position space, so a single press near the
    // floor is a small move (fine control), not a jump to the min. Stepping
    // down lowers the value but stays in range; apply_min snaps to the floor;
    // and stepping down again at the floor holds there.
    controls.pad.attack_time = 0.1;
    apply_delta(Tab::Chords, 1, -1.0, &mut controls);
    assert!(controls.pad.attack_time < 0.1 && controls.pad.attack_time >= 0.05);
    apply_min(Tab::Chords, 1, &mut controls);
    assert_close(controls.pad.attack_time, 0.05);
    apply_delta(Tab::Chords, 1, -1.0, &mut controls);
    assert_close(controls.pad.attack_time, 0.05);

    controls.pad.release_time = 0.1;
    apply_delta(Tab::Chords, 2, -1.0, &mut controls);
    assert!(controls.pad.release_time < 0.1 && controls.pad.release_time >= 0.05);
    apply_min(Tab::Chords, 2, &mut controls);
    assert_close(controls.pad.release_time, 0.05);
    apply_delta(Tab::Chords, 2, -1.0, &mut controls);
    assert_close(controls.pad.release_time, 0.05);
}

#[test]
fn kick_interval_floor_is_eighth_beat() {
    let mut controls = FluidControls::default();
    controls.kick.interval_beats = 1.0;
    apply_min(Tab::Kick, 4, &mut controls);
    assert_close(controls.kick.interval_beats, 0.125);

    controls.kick.interval_beats = 0.125;
    apply_delta(Tab::Kick, 4, -1.0, &mut controls);
    assert_close(controls.kick.interval_beats, 0.125);
}

#[test]
fn perc_continuous_mode_pushes_no_hits() {
    let controls = PercControls {
        level: 1.0,
        interval_beats: 4.25,
        ..Default::default()
    };

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
    let controls = PercControls {
        level: 1.0,
        interval_beats: 4.25,
        ..Default::default()
    };

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
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[3].label, "Interval");
    assert_close(rows[3].min, 0.125);
    assert_close(rows[3].max, 4.25);
    assert_eq!(rows[4].label, "Offset");
    assert_close(rows[4].min, 0.0);
    assert_close(rows[4].max, 4.0);
}

#[test]
fn perc_interval_displays_continuous_at_top() {
    let mut controls = FluidControls::default();
    controls.perc.interval_beats = 4.25;
    let rows = tab_controls(Tab::Perc, &controls);
    assert_eq!(rows[3].display, "Continuous");
}

#[test]
fn perc_interval_and_offset_adjust_and_clamp() {
    let mut controls = FluidControls::default();

    apply_delta(Tab::Perc, 3, 1.0, &mut controls);
    assert_close(controls.perc.interval_beats, 0.5);

    controls.perc.interval_beats = 0.25;
    apply_delta(Tab::Perc, 3, -1.0, &mut controls);
    assert_close(controls.perc.interval_beats, 0.125);
    apply_delta(Tab::Perc, 3, 1.0, &mut controls);
    assert_close(controls.perc.interval_beats, 0.25);

    controls.perc.interval_beats = 4.25;
    apply_delta(Tab::Perc, 3, 1.0, &mut controls);
    assert_close(controls.perc.interval_beats, 4.25);

    apply_delta(Tab::Perc, 4, 1.0, &mut controls);
    assert_close(controls.perc.offset_beats, 0.125);

    controls.perc.offset_beats = 4.0;
    apply_delta(Tab::Perc, 4, 1.0, &mut controls);
    assert_close(controls.perc.offset_beats, 4.0);

    apply_min(Tab::Perc, 3, &mut controls);
    assert_close(controls.perc.interval_beats, 0.125);

    apply_min(Tab::Perc, 4, &mut controls);
    assert_close(controls.perc.offset_beats, 0.0);
}

#[test]
fn offset_grid_keeps_true_zero_reachable_below_the_floor() {
    // Offsets have a genuine 0 = "no shift" minimum, unlike intervals (whose
    // minimum is the 0.125 floor itself): 0 must survive as an extra rung
    // below the floor, with sixteenths taking over above it.
    let mut controls = FluidControls::default();
    apply_value(Tab::Perc, 4, 0.03, &mut controls);
    assert_close(controls.perc.offset_beats, 0.0);

    apply_value(Tab::Perc, 4, 0.09, &mut controls);
    assert_close(controls.perc.offset_beats, 0.125);

    apply_value(Tab::Perc, 4, 0.3, &mut controls);
    assert_close(controls.perc.offset_beats, 0.25);

    controls.perc.offset_beats = 0.125;
    apply_delta(Tab::Perc, 4, -1.0, &mut controls);
    assert_close(controls.perc.offset_beats, 0.0);
    apply_delta(Tab::Perc, 4, -1.0, &mut controls);
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
fn pad_chord_notes_with_slot_builds_notes_from_root_extension_and_inversion() {
    let slot = ChordSlotControls {
        degree: 1.0,
        accidental: -1.0,
        extension: 2.0,
        inversion: 1.0,
    };

    let notes = pad_chord_notes_with_slot(&slot);

    assert_eq!(notes, pad_chord_notes_with_slot(&slot));
    assert_eq!(notes.len(), 4);
    // Strictly ascending: inversion/accidental never collapse two voices
    // onto the same (or a crossed) pitch.
    assert!(notes.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn pad_chord_root_note_applies_degree_and_accidental() {
    let flat_default = ChordSlotControls::default();
    assert_eq!(pad_chord_root_note(&flat_default), 45); // A2 tonic

    let sharp_second = ChordSlotControls {
        degree: 1.0,
        accidental: 1.0,
        ..ChordSlotControls::default()
    };
    // A2 up one diatonic degree (B2, MIDI 47) plus a sharp.
    assert_eq!(pad_chord_root_note(&sharp_second), 48);
}

#[test]
fn custom_progression_pad_bass_and_arp_read_the_same_chord_source() {
    let mut pad = PadControls {
        progression: CUSTOM_PROGRESSION_INDEX as f32,
        chord_count: 4.0,
        ..PadControls::default()
    };
    pad.chord_slots[2].degree = 3.0;
    pad.chord_slots[2].accidental = -1.0;
    pad.chord_slots[2].extension = 1.0;

    let tones = pad_chord_tones(&pad, 2);
    let root = bass_root_note(CUSTOM_PROGRESSION_INDEX, 2, &pad);

    // Bass's root matches the pad chord's own root voice, and both derive
    // from the same slot data via the shared chord-source path.
    assert_eq!(root, pad_chord_root_note(&pad.chord_slots[2]));
    assert_eq!(root, tones[0]);

    // Arp cycles the same 4 chord tones the pad voices (span 1 = no octave
    // duplication), proving it reads through the identical path.
    assert_eq!(arp_cycle_notes(tones, 1), {
        let mut sorted = tones;
        sorted.sort_unstable();
        sorted.to_vec()
    });
}

#[test]
fn pad_chord_count_gates_step_wrap_only_in_custom_mode() {
    let built_in = PadControls {
        progression: 0.0,
        chord_count: 2.0, // inert: built-ins always wrap at 8
        ..PadControls::default()
    };
    assert_eq!(pad_chord_count(&built_in), 8);

    let custom = PadControls {
        progression: CUSTOM_PROGRESSION_INDEX as f32,
        chord_count: 2.0,
        ..PadControls::default()
    };
    assert_eq!(pad_chord_count(&custom), 2);
}

#[test]
fn bass_engine_step_index_wraps_at_pad_chord_count_in_custom_mode() {
    let sample_rate = 48_000.0;
    let mut bass = BassEngine::new(sample_rate);
    let pad = PadControls {
        chord_bars: 1.0,
        progression: CUSTOM_PROGRESSION_INDEX as f32,
        chord_count: 2.0,
        ..PadControls::default()
    };
    let bass_controls = BassControls::default();

    for chord in 1..=5 {
        let sample = chord * sample_rate as u64 * 2;
        let timing = timing(sample, 120.0);
        bass.next(&bass_controls, &pad, 0.0, timing);
        assert!(bass.step_index < 2);
    }
}

#[test]
fn pad_engine_step_index_wraps_at_pad_chord_count_in_custom_mode() {
    let controls = PadControls {
        chord_bars: 1.0,
        progression: CUSTOM_PROGRESSION_INDEX as f32,
        chord_count: 2.0,
        attack_time: 1.0,
        ..PadControls::default()
    };
    let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

    for chord in 1..=5 {
        let sample = chord * SAMPLE_RATE as u64 * 2;
        let _ = pad.next(&controls, 0.0, timing(sample, 120.0));
        assert!(pad.step_index < 2);
    }
}

#[test]
fn pad_engine_chord_slot_edit_retriggers_immediately() {
    let mut controls = PadControls {
        progression: CUSTOM_PROGRESSION_INDEX as f32,
        chord_bars: 64.0,
        attack_time: 0.001,
        ..PadControls::default()
    };
    let mut pad = PadEngine::new(SAMPLE_RATE, &controls, Arc::new(FluidTelemetry::default()));

    for sample in 0..10 {
        let _ = pad.next(&controls, 0.0, timing(sample, 120.0));
    }
    let layers_before = pad.layers.len();

    controls.chord_slots[0].degree = 1.0;
    let _ = pad.next(&controls, 0.0, timing(10, 120.0));

    assert!(
        pad.layers.len() > layers_before,
        "editing the current chord slot must push a new layer immediately"
    );
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

fn max_hit_gap(hit_beats: &[f64], total_beats: f64) -> f64 {
    let mut max_gap = 0.0f64;
    let mut prev = 0.0f64;
    for &beat in hit_beats {
        max_gap = max_gap.max(beat - prev);
        prev = beat;
    }
    max_gap.max(total_beats - prev)
}

#[test]
fn grid_trigger_survives_continuous_interval_sweep() {
    let total_beats = 32.0;
    let samples = (total_beats * 60.0 / 120.0 * f64::from(SAMPLE_RATE)) as u64;
    let mut trigger = GridTrigger::new();
    let mut hit_beats = Vec::new();

    for sample in 0..samples {
        let t = timing(sample, 120.0);
        let interval = 1.0 + 0.75 * (std::f64::consts::TAU * t.beat / 8.0).sin() as f32;
        if trigger.pop(t, interval, 0.0) {
            hit_beats.push(t.beat);
        }
    }

    let max_gap = max_hit_gap(&hit_beats, total_beats);
    // Bound is a hair above one peak interval (1.75): the anti-double-fire floor
    // (half an interval after the last hit) is slightly more conservative than
    // an unbounded pull-earlier, so the worst-case gap under a continuous
    // interval LFO sits just over 2 beats. Still nowhere near a stall.
    assert!(
        max_gap <= 2.25,
        "trigger starved during interval sweep: max gap {max_gap:.2} beats"
    );
}

#[test]
fn grid_trigger_survives_sliding_offset() {
    let total_beats = 32.0;
    let samples = (total_beats * 60.0 / 120.0 * f64::from(SAMPLE_RATE)) as u64;
    let mut trigger = GridTrigger::new();
    let mut hit_beats = Vec::new();

    for sample in 0..samples {
        let t = timing(sample, 120.0);
        let offset = 2.0 + 2.0 * (std::f64::consts::TAU * t.beat / 8.0).sin() as f32;
        if trigger.pop(t, 1.0, offset) {
            hit_beats.push(t.beat);
        }
    }

    let max_gap = max_hit_gap(&hit_beats, total_beats);
    assert!(
        max_gap <= 1.5,
        "trigger starved during offset slide: max gap {max_gap:.2} beats"
    );
}

fn min_hit_gap(hit_beats: &[f64]) -> f64 {
    hit_beats
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(f64::INFINITY, f64::min)
}

/// Cranking swing while the grid runs must never re-fire the slot that just
/// sounded: consecutive hits stay at least half an interval apart. This is the
/// double-trigger / flam bug that a live timing tweak used to produce.
#[test]
fn grid_trigger_no_double_fire_when_swing_ramps() {
    let interval = 0.5f32;
    let total_beats = 32.0;
    let samples = (total_beats * 60.0 / 120.0 * f64::from(SAMPLE_RATE)) as u64;
    let mut trigger = GridTrigger::new();
    let mut hit_beats = Vec::new();

    for sample in 0..samples {
        let t = timing(sample, 120.0);
        // Sweep swing 0 -> 1 across the run so the reshape lands at every phase.
        let swing = (t.beat / total_beats) as f32;
        if trigger.pop_swung(t, interval, 0.0, swing) {
            hit_beats.push(t.beat);
        }
    }

    let min_gap = min_hit_gap(&hit_beats);
    assert!(
        min_gap >= f64::from(interval) * 0.5 - 1e-6,
        "swing ramp double-fired: min gap {min_gap:.4} beats < half interval"
    );
    // Still musically dense — the guard must not have stalled the grid.
    assert!(hit_beats.len() as f64 >= total_beats / f64::from(interval) * 0.5);
}

/// The same guard for a live offset jump: nudging offset must not squeeze two
/// hits closer than half an interval.
#[test]
fn grid_trigger_no_double_fire_when_offset_steps() {
    let interval = 1.0f32;
    let total_beats = 32.0;
    let samples = (total_beats * 60.0 / 120.0 * f64::from(SAMPLE_RATE)) as u64;
    let mut trigger = GridTrigger::new();
    let mut hit_beats = Vec::new();

    for sample in 0..samples {
        let t = timing(sample, 120.0);
        // Step the offset every few beats, straddling slot boundaries.
        let offset = 0.1 * (t.beat as f32 * 0.5).floor();
        if trigger.pop(t, interval, offset) {
            hit_beats.push(t.beat);
        }
    }

    let min_gap = min_hit_gap(&hit_beats);
    assert!(
        min_gap >= f64::from(interval) * 0.5 - 1e-6,
        "offset step double-fired: min gap {min_gap:.4} beats < half interval"
    );
}

/// A live offset nudge landing right as a hit fires must not squeeze the next
/// hit closer than half an interval (the same guard as the ramp/step tests
/// above, exercised against the exact live-edit gesture that was reported).
#[test]
fn grid_trigger_no_double_hit_after_offset_nudge() {
    let bpm = 120.0;
    let interval = 1.0f32;
    let total_samples = (SAMPLE_RATE as u64) * 8;
    let mut trigger = GridTrigger::new();
    let mut offset = 0.0f32;
    let mut nudged = false;
    let mut hit_beats = Vec::new();

    for sample in 0..total_samples {
        let t = timing(sample, bpm);
        if trigger.pop(t, interval, offset) {
            hit_beats.push(t.beat);
            if !nudged {
                // ~10ms nudge, matching the reported live-edit gesture.
                offset += (0.010 * bpm as f64 / 60.0) as f32;
                nudged = true;
            }
        }
    }

    assert!(nudged, "test never reached a hit to nudge after");
    let min_gap = min_hit_gap(&hit_beats);
    assert!(
        min_gap >= f64::from(interval) * 0.5 - 1e-6,
        "double hit after offset nudge: min gap {min_gap:.4} beats < half interval"
    );
}

/// Same guard for a live rate nudge landing right as a hit fires.
#[test]
fn grid_trigger_no_double_hit_after_rate_nudge() {
    let bpm = 120.0;
    let total_samples = (SAMPLE_RATE as u64) * 8;
    let mut trigger = GridTrigger::new();
    let mut interval = 1.0f32;
    let mut nudged = false;
    let mut hit_beats = Vec::new();

    for sample in 0..total_samples {
        let t = timing(sample, bpm);
        if trigger.pop(t, interval, 0.0) {
            hit_beats.push(t.beat);
            if !nudged {
                // A quick rate tweak right as a hit fires.
                interval *= 0.4;
                nudged = true;
            }
        }
    }

    assert!(nudged, "test never reached a hit to nudge after");
    let min_gap = min_hit_gap(&hit_beats);
    assert!(
        min_gap >= f64::from(interval) * 0.5 - 1e-6,
        "double hit after rate nudge: min gap {min_gap:.4} beats < half interval"
    );
}

fn automation_with_route(
    target_id: &'static str,
    depth_ratio: f32,
    cycle_beats: f32,
) -> AutomationState {
    let mut automation = AutomationState::default();
    automation.set_route(
        ControlAddress::new(target_id),
        LfoRoute {
            depth_ratio,
            cycle_beats,
            phase_offset_beats: 0.0,
            shape: LfoShape::Sine,
            ..LfoRoute::default()
        },
    );
    automation
}

#[test]
fn modulated_control_value_snaps_like_the_engine() {
    let spec = spec_by_id("kick.interval_beats").unwrap();
    let route = LfoRoute {
        depth_ratio: 0.4,
        cycle_beats: 8.0,
        phase_offset_beats: 0.0,
        shape: LfoShape::Sine,
        ..LfoRoute::default()
    };

    // Peak of the sine: raw value 1.0 + 0.4 * 3.875 = 2.55 must land on 2.0.
    let peak = modulated_control_value(spec, &route, 1.0, 2.0);
    assert_close(peak, 2.0);

    // Trough: 1.0 - 1.5 clamps to the minimum subdivision.
    let trough = modulated_control_value(spec, &route, 1.0, 6.0);
    assert_close(trough, 0.125);
}

#[test]
fn lfo_interval_modulation_snaps_to_power_of_two() {
    let mut controls = FluidControls::default();
    controls.kick.interval_beats = 1.0;
    let automation = automation_with_route("kick.interval_beats", 0.4, 8.0);

    for sample in (0..(SAMPLE_RATE as u64 * 16)).step_by(64) {
        let mut effective = controls.clone();
        apply_automation(&mut effective, &automation, timing(sample, 120.0));
        let v = effective.kick.interval_beats;
        assert!(
            [0.125f32, 0.25, 0.5, 1.0, 2.0, 4.0]
                .iter()
                .any(|&q| (v - q).abs() < 1e-4),
            "modulated interval {v} is not a power-of-two subdivision"
        );
    }
}

#[test]
fn lfo_offset_modulation_snaps_to_eighth_beats() {
    let mut controls = FluidControls::default();
    controls.kick.offset_beats = 2.0;
    let automation = automation_with_route("kick.offset_beats", 0.4, 8.0);

    for sample in (0..(SAMPLE_RATE as u64 * 16)).step_by(64) {
        let mut effective = controls.clone();
        apply_automation(&mut effective, &automation, timing(sample, 120.0));
        let v = effective.kick.offset_beats;
        let snapped = (v / 0.125).round() * 0.125;
        assert!(
            (v - snapped).abs() < 1e-4,
            "modulated offset {v} is not on the 0.125-beat grid"
        );
    }
}

#[test]
fn lfo_interval_sweep_plays_on_grid_breakdown() {
    let mut controls = FluidControls::default();
    controls.kick.interval_beats = 1.0;
    controls.kick.offset_beats = 0.0;
    let automation = automation_with_route("kick.interval_beats", 0.4, 8.0);

    let total_beats = 32.0;
    let samples = (total_beats * 60.0 / 120.0 * f64::from(SAMPLE_RATE)) as u64;
    let mut trigger = GridTrigger::new();
    let mut hit_beats = Vec::new();

    for sample in 0..samples {
        let t = timing(sample, 120.0);
        let mut effective = controls.clone();
        apply_automation(&mut effective, &automation, t);
        if trigger.pop(
            t,
            effective.kick.interval_beats,
            effective.kick.offset_beats,
        ) {
            hit_beats.push(t.beat);
        }
    }

    // Every hit stays locked to the absolute 16th grid.
    for &beat in &hit_beats {
        let snapped = (beat / 0.125).round() * 0.125;
        assert!(
            (beat - snapped).abs() < 1e-3,
            "hit at beat {beat:.4} is off the 0.125 grid"
        );
    }

    // The sweep actually breaks down through multiple subdivisions.
    let mut gaps: Vec<i64> = hit_beats
        .windows(2)
        .map(|w| ((w[1] - w[0]) / 0.125).round() as i64)
        .collect();
    gaps.sort_unstable();
    gaps.dedup();
    assert!(
        gaps.len() >= 3,
        "expected at least 3 distinct hit spacings, got {gaps:?}"
    );

    let max_gap = max_hit_gap(&hit_beats, total_beats);
    assert!(
        max_gap <= 2.0 + 1e-3,
        "trigger starved during breakdown sweep: max gap {max_gap:.2} beats"
    );
}

// ============================================================
// Modulator shapes, random determinism, envelopes
// ============================================================

fn lfo_shape(shape: LfoShape) -> LfoRoute {
    LfoRoute {
        shape,
        cycle_beats: 1.0,
        depth_ratio: 1.0,
        ..LfoRoute::default()
    }
}

fn env_ctx(beat: f64) -> ModContext {
    ModContext {
        beat,
        kick_interval_beats: 1.0,
        kick_offset_beats: 0.0,
    }
}

#[test]
fn render_fluid_draws_envelope_submenu_and_lane() {
    let controls = FluidControls::default();
    let fluid = FluidState::new();
    let items = tab_controls(Tab::Chords, &controls);
    let mut automation = AutomationState::default();
    let address = ControlAddress::new(items[3].id); // Reverb Mix
    // A random LFO plus an open envelope editor exercises both new lanes.
    automation.open_or_create(address).shape = LfoShape::SampleHold;
    automation.open_or_create_envelope(address).amount = 0.5;

    let backend = TestBackend::new(120, 44);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render(
                f,
                &items,
                Tab::Chords,
                3,
                1,
                2.5,
                NumericDisplay::empty(),
                &fluid,
                &automation,
                &controls,
                None,
                &FlippedUnits::new(),
                ChordDrill::None,
                &[None; 9],
            )
        })
        .unwrap();

    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("attack"));
    assert!(text.contains("decay"));
    assert!(text.contains("trigger"));
}

#[test]
fn lfo_shapes_match_reference_curves() {
    // cycle_beats == 1.0 means beat value equals phase in 0..1.
    let tri = lfo_shape(LfoShape::Triangle);
    assert_near(tri.wave_at(0.0), 0.0);
    assert_near(tri.wave_at(0.25), 1.0);
    assert_near(tri.wave_at(0.5), 0.0);
    assert_near(tri.wave_at(0.75), -1.0);

    let up = lfo_shape(LfoShape::RampUp);
    assert_near(up.wave_at(0.0), -1.0);
    assert_near(up.wave_at(0.5), 0.0);
    assert_near(up.wave_at(0.75), 0.5);

    let down = lfo_shape(LfoShape::RampDown);
    assert_near(down.wave_at(0.0), 1.0);
    assert_near(down.wave_at(0.5), 0.0);
    assert_near(down.wave_at(0.75), -0.5);

    let square = lfo_shape(LfoShape::Square);
    assert!(square.wave_at(0.25) > 0.99, "square high near +1");
    assert!(square.wave_at(0.75) < -0.99, "square low near -1");
}

#[test]
fn sample_hold_is_stepped_and_seeded() {
    let route = LfoRoute {
        shape: LfoShape::SampleHold,
        cycle_beats: 1.0,
        depth_ratio: 1.0,
        seed: 12345,
        ..LfoRoute::default()
    };

    // Constant within one cycle, so what the marker shows holds until the step.
    assert_close(route.wave_at(0.1), route.wave_at(0.9));
    // Stepping to the next cycle almost always lands on a different value.
    assert!((route.wave_at(0.5) - route.wave_at(1.5)).abs() > 1e-6);

    // Same seed reproduces the same trajectory exactly.
    let twin = route;
    for i in 0..64 {
        let beat = f64::from(i) * 0.5;
        assert_close(route.wave_at(beat), twin.wave_at(beat));
    }
}

#[test]
fn random_drift_is_deterministic_for_a_seed() {
    let a = LfoRoute {
        shape: LfoShape::RandomDrift,
        cycle_beats: 2.0,
        depth_ratio: 1.0,
        seed: 777,
        ..LfoRoute::default()
    };
    let b = a;
    for i in 0..128 {
        let beat = f64::from(i) * 0.3;
        assert_close(a.wave_at(beat), b.wave_at(beat));
        assert!(a.wave_at(beat).abs() <= 1.0 + 1e-6);
    }
}

#[test]
fn reseed_changes_pattern_but_stays_repeatable() {
    let base = LfoRoute {
        shape: LfoShape::SampleHold,
        cycle_beats: 1.0,
        depth_ratio: 1.0,
        seed: 5,
        ..LfoRoute::default()
    };
    let sample =
        |route: &LfoRoute| -> Vec<f32> { (0..32).map(|i| route.wave_at(f64::from(i))).collect() };

    let original = sample(&base);
    let mut rolled = base;
    rolled.reseed();
    let mut rolled_again = base;
    rolled_again.reseed();

    // Reseed is deterministic: two routes reseeded from the same start match,
    // so `render --seed` stays byte-identical.
    assert_eq!(sample(&rolled), sample(&rolled_again));
    // ...and it actually produced a different pattern.
    assert_ne!(sample(&base), sample(&rolled));
    let _ = original;
}

#[test]
fn envelope_level_follows_attack_then_decay() {
    let env = EnvelopeRoute {
        amount: 1.0,
        attack_beats: 2.0,
        decay_beats: 2.0,
        trigger: EnvTrigger::Once,
    };
    assert_near(env.level_at(env_ctx(0.0)), 0.0);
    assert_near(env.level_at(env_ctx(1.0)), 0.5);
    assert_near(env.level_at(env_ctx(2.0)), 1.0);
    assert_near(env.level_at(env_ctx(3.0)), 0.5);
    assert_near(env.level_at(env_ctx(4.0)), 0.0);
    assert_near(env.level_at(env_ctx(9.0)), 0.0);
}

#[test]
fn envelope_macro_holds_at_peak_when_decay_is_zero() {
    let env = EnvelopeRoute {
        amount: 1.0,
        attack_beats: 4.0,
        decay_beats: 0.0,
        trigger: EnvTrigger::Once,
    };
    assert_near(env.level_at(env_ctx(2.0)), 0.5);
    assert_near(env.level_at(env_ctx(4.0)), 1.0);
    assert_near(env.level_at(env_ctx(400.0)), 1.0);
}

#[test]
fn envelope_every_n_beats_retriggers() {
    let env = EnvelopeRoute {
        amount: 1.0,
        attack_beats: 0.0,
        decay_beats: 4.0,
        trigger: EnvTrigger::EveryBeats(4.0),
    };
    // Instant attack, so the sweep is at its peak right after each trigger.
    assert_near(env.level_at(env_ctx(0.0)), 1.0);
    assert!(env.level_at(env_ctx(3.9)) < 0.1);
    // Beat 4 wraps back to the start of a fresh one-shot.
    assert_near(env.level_at(env_ctx(4.0)), 1.0);
}

#[test]
fn envelope_on_kick_tracks_the_kick_grid() {
    let env = EnvelopeRoute {
        amount: 1.0,
        attack_beats: 0.0,
        decay_beats: 1.0,
        trigger: EnvTrigger::OnKick,
    };
    let ctx = |beat: f64| ModContext {
        beat,
        kick_interval_beats: 2.0,
        kick_offset_beats: 0.0,
    };
    // Kicks land on beats 0, 2, 4, ...; the one-shot peaks right at each hit.
    assert_near(env.level_at(ctx(2.0)), 1.0);
    assert_near(env.level_at(ctx(2.5)), 0.5);
    assert_near(env.level_at(ctx(4.0)), 1.0);
}

#[test]
fn envelope_amount_zero_is_audible_neutral() {
    let mut controls = FluidControls::default();
    controls.master.level = 0.5;
    let mut automation = AutomationState::default();
    automation.open_or_create_envelope(ControlAddress::new("master.level"));

    apply_automation(
        &mut controls,
        &automation,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 1.0),
    );

    assert_close(controls.master.level, 0.5);
}

#[test]
fn open_or_create_envelope_defaults_to_neutral_amount() {
    let mut automation = AutomationState::default();
    let address = ControlAddress::new("master.level");

    let env = automation.open_or_create_envelope(address);

    assert_close(env.amount, 0.0);
    assert_eq!(automation.active_kind(), Some(ModKind::Envelope));
}

#[test]
fn close_editor_deletes_zero_amount_envelope() {
    let mut automation = AutomationState::default();
    let address = ControlAddress::new("master.level");
    automation.open_or_create_envelope(address);

    automation.close_editor();
    assert!(automation.envelope(address).is_none());

    automation.open_or_create_envelope(address).amount = 0.5;
    automation.close_editor();
    assert!(automation.envelope(address).is_some());
}

#[test]
fn lfo_and_envelope_coexist_on_one_control() {
    let mut automation = AutomationState::default();
    let address = ControlAddress::new("pad.reverb_mix");
    automation.open_or_create(address).depth_ratio = 0.3;
    automation.open_or_create_envelope(address).amount = 0.4;

    assert!(automation.route(address).is_some());
    assert!(automation.envelope(address).is_some());
    assert_eq!(automation.active_kind(), Some(ModKind::Envelope));
}

#[test]
fn combined_lfo_and_envelope_sum_and_clamp() {
    let mut controls = FluidControls::default();
    controls.master.level = 0.5;
    let address = ControlAddress::new("master.level");
    let mut automation = AutomationState::default();
    automation.set_route(
        address,
        LfoRoute {
            depth_ratio: 0.5,
            cycle_beats: 2.0,
            shape: LfoShape::Sine,
            ..LfoRoute::default()
        },
    );
    automation.set_envelope(
        address,
        EnvelopeRoute {
            amount: 0.5,
            attack_beats: 0.0,
            decay_beats: 64.0,
            trigger: EnvTrigger::Once,
        },
    );

    // Beat 0.5 is the sine peak (cycle 2) and near the envelope peak; both push
    // up from 0.5, so the summed value saturates at the control ceiling.
    apply_automation(
        &mut controls,
        &automation,
        TimingContext::new(f64::from(SAMPLE_RATE), 120.0, 0.5),
    );
    assert_close(controls.master.level, 1.0);
}

#[test]
fn song_code_round_trips_non_sine_lfo_shape() {
    let mut automation = AutomationState::default();
    automation.set_route(
        ControlAddress::new("master.level"),
        LfoRoute {
            cycle_beats: 4.0,
            depth_ratio: 0.4,
            shape: LfoShape::SampleHold,
            phase_offset_beats: 0.0,
            ..LfoRoute::default()
        },
    );
    let song = SongState {
        controls: FluidControls::default(),
        automation,
    };

    let code = song::encode_song_code(&song).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();
    let route = decoded
        .automation
        .route(ControlAddress::new("master.level"))
        .unwrap();

    assert_eq!(route.shape, LfoShape::SampleHold);
}

#[test]
fn song_code_round_trips_envelope_routes() {
    let mut automation = AutomationState::default();
    automation.set_envelope(
        ControlAddress::new("pad.reverb_mix"),
        EnvelopeRoute {
            amount: 0.6,
            attack_beats: 1.5,
            decay_beats: 3.0,
            trigger: EnvTrigger::OnKick,
        },
    );
    let song = SongState {
        controls: FluidControls::default(),
        automation,
    };

    let code = song::encode_song_code(&song).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();
    let env = decoded
        .automation
        .envelope(ControlAddress::new("pad.reverb_mix"))
        .unwrap();

    assert_close(env.amount, 0.6);
    assert_close(env.attack_beats, 1.5);
    assert_close(env.decay_beats, 3.0);
    assert_eq!(env.trigger, EnvTrigger::OnKick);
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

#[test]
fn unit_conversion_round_trips_at_current_bpm() {
    let bpm = 82.0;
    assert_near(beats_to_ms(1.0, 120.0), 500.0);
    assert_near(ms_to_beats(500.0, 120.0), 1.0);
    let beats = 2.125;
    assert_near(ms_to_beats(beats_to_ms(beats, bpm), bpm), beats);
    assert_eq!(
        unit_key("kick.level", Some("lfo.interval")),
        "kick.level#lfo.interval"
    );
    assert_eq!(unit_key("perc.decay_ms", None), "perc.decay_ms");
}

#[test]
fn flipped_time_fields_step_in_ms_and_snap_back_onto_the_beat_grid() {
    let mut c = FluidControls::default();
    c.master.bpm = 120.0; // one beat is exactly 500 ms
    c.perc.decay_ms = 470.0;
    let controls = Arc::new(ArcSwap::from_pointee(c));
    let shared = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let mut automation = PublishedAutomation::new(AutomationState::default(), shared);
    let mut flipped = FlippedUnits::new();

    // Perc interval (native beats) flipped to ms: h/l moves on the 10 ms
    // grid instead of the beat grid. 0.25 beats = 125 ms -> 140 ms.
    flipped.insert(unit_key("perc.interval_beats", None));
    adjust_lfo_or_control(
        &mut automation,
        0,
        &controls,
        Tab::Perc,
        3,
        1.0,
        0.0,
        &flipped,
    );
    assert_near(
        beats_to_ms(controls.load().perc.interval_beats, 120.0),
        140.0,
    );

    // Flipping back to beats lands the value on the control's own grid.
    flipped.remove(&unit_key("perc.interval_beats", None));
    snap_after_unit_flip(&mut automation, 0, &controls, Tab::Perc, 3, false, 0.0);
    assert_close(controls.load().perc.interval_beats, 0.25);

    // An ms-native control flipped to beats rounds to the nearest divided
    // beat: 470 ms at 120 BPM is 0.94 beats -> 1.0 beats -> 500 ms.
    snap_after_unit_flip(&mut automation, 0, &controls, Tab::Perc, 2, true, 0.0);
    assert_near(controls.load().perc.decay_ms, 500.0);

    // And once flipped, it steps on the 0.125-beat grid: 500 ms + an eighth
    // of a beat (62.5 ms) at 120 BPM.
    flipped.insert(unit_key("perc.decay_ms", None));
    adjust_lfo_or_control(
        &mut automation,
        0,
        &controls,
        Tab::Perc,
        2,
        1.0,
        0.0,
        &flipped,
    );
    assert_near(controls.load().perc.decay_ms, 562.5);
}

#[test]
fn flipped_lfo_interval_steps_in_ms_and_keeps_exact_values() {
    let controls = Arc::new(ArcSwap::from_pointee(FluidControls::default()));
    {
        let mut c = FluidControls::clone(&controls.load());
        c.master.bpm = 120.0;
        controls.store(Arc::new(c));
    }
    let shared = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let mut automation = PublishedAutomation::new(AutomationState::default(), shared);
    let address = ControlAddress::new("master.level");
    automation.edit(|state| {
        state.open_or_create(address);
    });
    let mut flipped = FlippedUnits::new();
    flipped.insert(unit_key("master.level", Some("lfo.interval")));

    // Default cycle is 2 beats = 1000 ms at 120 BPM; one flipped step lands
    // on 1010 ms, off the beat grid. Interval is row 2 (amount is row 1).
    adjust_lfo_or_control(
        &mut automation,
        2,
        &controls,
        Tab::Master,
        0,
        1.0,
        0.0,
        &flipped,
    );
    assert_near(
        beats_to_ms(
            automation.state().route(address).unwrap().cycle_beats,
            120.0,
        ),
        1010.0,
    );

    // Un-flipping snaps the interval back onto the sixteenth grid.
    flipped.clear();
    snap_after_unit_flip(&mut automation, 2, &controls, Tab::Master, 0, false, 0.0);
    assert_close(automation.state().route(address).unwrap().cycle_beats, 2.0);
}

// ============================================================
// Automation payload v3: seeds, macro routes, envelopes
// ============================================================

#[test]
fn song_code_v5_round_trips_seed_macro_envelope_and_field_macro() {
    let mut automation = AutomationState::default();
    automation.set_route(
        ControlAddress::new("master.level"),
        LfoRoute {
            cycle_beats: 4.0,
            depth_ratio: 0.4,
            shape: LfoShape::SampleHold,
            phase_offset_beats: 0.5,
            seed: 0xDEAD_BEEF,
        },
    );
    automation.set_field_macro(
        unit_key("master.level", Some("lfo.amount")),
        single_macro_route(1, 0.35),
    );
    // pad.level rides two macro sliders at once, proving persistence keeps
    // every slot, not just one target.
    let mut pad_route = MacroRoute::default();
    pad_route.amounts[2] = -0.55;
    pad_route.amounts[3] = 0.2;
    automation.set_macro_route(ControlAddress::new("pad.level"), pad_route);
    automation.set_envelope(
        ControlAddress::new("pad.reverb_mix"),
        EnvelopeRoute {
            amount: 0.7,
            attack_beats: 1.25,
            decay_beats: 6.0,
            trigger: EnvTrigger::EveryBeats(8.0),
        },
    );
    let song = SongState {
        controls: FluidControls::default(),
        automation,
    };

    let code = song::encode_song_code(&song).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    let route = decoded
        .automation
        .route(ControlAddress::new("master.level"))
        .unwrap();
    assert_close(route.cycle_beats, 4.0);
    assert_close(route.depth_ratio, 0.4);
    assert_eq!(route.shape, LfoShape::SampleHold);
    assert_close(route.phase_offset_beats, 0.5);
    assert_eq!(route.seed, 0xDEAD_BEEF);

    let field_macro = decoded
        .automation
        .field_macro(&unit_key("master.level", Some("lfo.amount")))
        .unwrap();
    assert_close(field_macro.amounts[1], 0.35);

    let macro_route = decoded
        .automation
        .macro_route(ControlAddress::new("pad.level"))
        .unwrap();
    assert_close(macro_route.amounts[2], -0.55);
    assert_close(macro_route.amounts[3], 0.2);

    let env = decoded
        .automation
        .envelope(ControlAddress::new("pad.reverb_mix"))
        .unwrap();
    assert_close(env.amount, 0.7);
    assert_close(env.attack_beats, 1.25);
    assert_close(env.decay_beats, 6.0);
    assert_eq!(env.trigger, EnvTrigger::EveryBeats(8.0));
}

#[test]
fn song_code_decodes_hand_built_v2_automation_payload() {
    // Hand-built payload using the pre-v3 layout: version byte 2, LFO count,
    // then per-route (id, cycle, depth, shape tag, offset) with no seed and
    // no macro/envelope sections. Confirms old song codes keep working.
    let code = song::encode_song_code(&SongState::default()).unwrap();
    let mut payload = Vec::new();
    payload.push(2u8); // AUTOMATION_PAYLOAD_VERSION_V2
    payload.extend_from_slice(&1u16.to_le_bytes());
    write_test_str("master.level", &mut payload);
    payload.extend_from_slice(&4.0f32.to_le_bytes()); // cycle_beats
    payload.extend_from_slice(&0.4f32.to_le_bytes()); // depth_ratio
    payload.push(0); // LFO_SHAPE_SINE
    payload.extend_from_slice(&0.25f32.to_le_bytes()); // phase_offset_beats
    let code = append_record_to_code(&code, song::AUTOMATION_RECORD, &payload);

    let decoded = song::decode_song_code(&code).unwrap();
    let route = decoded
        .automation
        .route(ControlAddress::new("master.level"))
        .unwrap();

    assert_close(route.cycle_beats, 4.0);
    assert_close(route.depth_ratio, 0.4);
    assert_eq!(route.shape, LfoShape::Sine);
    assert_close(route.phase_offset_beats, 0.25);
    // v2 payloads carry no seed; decoding must fall back to the route default.
    assert_eq!(route.seed, 0);
    assert!(decoded.automation.macro_routes().next().is_none());
    assert!(decoded.automation.envelopes().next().is_none());
}

#[test]
fn song_code_decodes_hand_built_v4_single_target_macro_into_one_slot() {
    // Hand-built v4 payload: the pre-v5 macro shape named one target macro
    // slider plus one amount per address. Confirms song codes authored
    // before the "4 independent amounts" model keep decoding, landing in
    // just that one slot of the new per-slider representation.
    let code = song::encode_song_code(&SongState::default()).unwrap();
    let mut payload = Vec::new();
    payload.push(4u8); // AUTOMATION_PAYLOAD_VERSION_V4
    payload.extend_from_slice(&0u16.to_le_bytes()); // no LFO routes
    payload.extend_from_slice(&1u16.to_le_bytes()); // one legacy macro route
    write_test_str("pad.level", &mut payload);
    payload.push(2); // target: macro slider index 2
    payload.extend_from_slice(&(-0.6f32).to_le_bytes()); // amount
    payload.extend_from_slice(&0u16.to_le_bytes()); // no envelopes
    payload.extend_from_slice(&0u16.to_le_bytes()); // no legacy field macros
    let code = append_record_to_code(&code, song::AUTOMATION_RECORD, &payload);

    let decoded = song::decode_song_code(&code).unwrap();
    let route = decoded
        .automation
        .macro_route(ControlAddress::new("pad.level"))
        .unwrap();
    for (i, amount) in route.amounts.iter().enumerate() {
        if i == 2 {
            assert_close(*amount, -0.6);
        } else {
            assert_close(*amount, 0.0);
        }
    }
}

#[test]
fn song_code_does_not_serialize_neutral_macro_routes() {
    let mut automation = AutomationState::default();
    // Every slot at zero: neutral, must not be written.
    automation.set_macro_route(ControlAddress::new("master.level"), MacroRoute::default());
    automation.set_macro_route(ControlAddress::new("pad.level"), MacroRoute::default());
    let song = SongState {
        controls: FluidControls::default(),
        automation,
    };

    let code = song::encode_song_code(&song).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert!(decoded.automation.macro_routes().next().is_none());
    assert!(
        decoded
            .automation
            .macro_route(ControlAddress::new("master.level"))
            .is_none()
    );
    assert!(
        decoded
            .automation
            .macro_route(ControlAddress::new("pad.level"))
            .is_none()
    );
}

#[test]
fn enter_expands_into_the_owning_tab() {
    assert_eq!(tab_owning_control("pad.level"), Some(Tab::Chords));
    assert_eq!(tab_owning_control("bass.level"), Some(Tab::Bass));
    assert_eq!(tab_owning_control("macro.1"), Some(Tab::Macros));
    assert_eq!(tab_owning_control("master.bpm"), Some(Tab::Master));
    assert_eq!(tab_owning_control("nope.nope"), None);
}

#[test]
fn macro_toggle_hides_but_keeps_the_assignment() {
    let controls = FluidControls::default();
    let items = tab_controls(Tab::Master, &controls);
    let shared = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let mut automation = PublishedAutomation::new(AutomationState::default(), shared);
    let address = ControlAddress::new(items[0].id);
    let mut sub = 0usize;

    open_modulator(&mut automation, &items, 0, ModKind::Macro, &mut sub);
    automation.edit(|state| {
        let route = state.macro_route_mut(address).unwrap();
        route.amounts[1] = 0.5;
    });

    open_modulator(&mut automation, &items, 0, ModKind::Macro, &mut sub);
    assert!(!automation.state().is_editor_open());
    let route = automation.state().macro_route(address).unwrap();
    assert_close(route.amounts[1], 0.5);
}

/// Timing harness for the audio hot path with a busy automation state.
/// Run with `cargo test --release engine_hot_path_timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn engine_hot_path_timing() {
    let mut automation = AutomationState::default();
    automation.set_route(ControlAddress::new("pad.level"), LfoRoute::default());
    automation.set_route(
        ControlAddress::new("kick.interval_beats"),
        LfoRoute::default(),
    );
    automation.set_route(ControlAddress::new("tonal.level"), LfoRoute::default());
    automation.set_route(ControlAddress::new("macro.1"), LfoRoute::default());
    automation.set_field_macro(
        unit_key("pad.level", Some("lfo.amount")),
        single_macro_route(0, 0.5),
    );
    automation.set_macro_route(
        ControlAddress::new("perc.level"),
        single_macro_route(0, 0.4),
    );
    automation.set_macro_route(
        ControlAddress::new("bass.level"),
        single_macro_route(1, -0.3),
    );
    automation.set_envelope(
        ControlAddress::new("macro.1"),
        EnvelopeRoute {
            amount: 0.5,
            ..EnvelopeRoute::default()
        },
    );

    let controls = Arc::new(ArcSwap::from_pointee(FluidControls::default()));
    let automation = Arc::new(ArcSwap::from_pointee(automation));
    let telemetry = Arc::new(FluidTelemetry::default());
    let mut engine = FluidEngine::new(SAMPLE_RATE, controls, automation, no_morph(), telemetry);

    let frames = SAMPLE_RATE as u64 * 10;
    let start = Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..frames {
        let (l, r) = engine.next_stereo();
        acc += l + r;
    }
    let elapsed = start.elapsed();
    println!(
        "10 s of audio in {elapsed:?} ({:.1}x realtime, acc {acc})",
        10.0 / elapsed.as_secs_f64()
    );
}

// ============================================================
// Arp
// ============================================================

#[test]
fn arp_defaults_are_silent() {
    assert_close(ArpControls::default().gain, 0.0);
}

#[test]
fn arp_default_voice_type_matches_former_fixed_pluck_profile() {
    // arp.type replaced a hardcoded `TONAL_PIANO_PROFILES[5]` ("Pluck").
    // The default value must resolve to that exact profile so existing
    // songs and a fresh startup render identically to before the control
    // existed.
    let expected = TONAL_PIANO_PROFILES[5];
    let actual = piano_profile(tonal_synth_type_index(ArpControls::default().voice_type));
    assert_eq!(actual.keyframes.len(), expected.keyframes.len());
    for (a, e) in actual.keyframes.iter().zip(expected.keyframes.iter()) {
        assert_eq!(a.midi, e.midi);
        assert_eq!(a.decay_factor, e.decay_factor);
        assert_eq!(a.harmonics, e.harmonics);
    }
    assert_eq!(actual.amplitude, expected.amplitude);
    assert_eq!(actual.body_power, expected.body_power);
    assert_eq!(actual.harmonic_tilt, expected.harmonic_tilt);
    assert_eq!(actual.decay_low, expected.decay_low);
    assert_eq!(actual.decay_high, expected.decay_high);
    assert_eq!(actual.decay_scale, expected.decay_scale);
}

#[test]
fn arp_cycle_notes_duplicates_chord_up_whole_octaves_sorted() {
    let chord = [45, 48, 52, 55];

    assert_eq!(arp_cycle_notes(chord, 1), vec![45, 48, 52, 55]);
    assert_eq!(
        arp_cycle_notes(chord, 2),
        vec![45, 48, 52, 55, 57, 60, 64, 67]
    );
    assert_eq!(
        arp_cycle_notes(chord, 3),
        vec![45, 48, 52, 55, 57, 60, 64, 67, 69, 72, 76, 79]
    );
}

#[test]
fn arp_pattern_labels_and_index_map_round_trip() {
    assert_eq!(arp_pattern_label(0.0), "Up");
    assert_eq!(arp_pattern_label(1.0), "Down");
    assert_eq!(arp_pattern_label(2.0), "Up-Down");
    assert_eq!(arp_pattern_label(3.0), "Random");
    assert_eq!(arp_pattern_from_control(0.0), ArpPattern::Up);
    assert_eq!(arp_pattern_from_control(1.0), ArpPattern::Down);
    assert_eq!(arp_pattern_from_control(2.0), ArpPattern::UpDown);
    assert_eq!(arp_pattern_from_control(3.0), ArpPattern::Random);
}

/// Replays `arp_advance` `count` times from `pos`/`dir` and returns the
/// sequence of *emitted* indices (the index read before each advance),
/// exactly mirroring `ArpEngine::next`'s "read current pos, then advance"
/// order.
fn arp_advance_sequence(
    pattern: ArpPattern,
    len: usize,
    count: usize,
    rng: &mut StdRng,
) -> Vec<usize> {
    let mut pos = 0usize;
    let mut dir = 1i32;
    let mut seq = Vec::with_capacity(count);
    for _ in 0..count {
        seq.push(pos);
        let (next_pos, next_dir) = arp_advance(pos, pattern, len, dir, rng);
        pos = next_pos;
        dir = next_dir;
    }
    seq
}

#[test]
fn arp_pattern_up_cycles_ascending_across_octave_spans() {
    let mut rng = StdRng::seed_from_u64(0);
    assert_eq!(
        arp_advance_sequence(ArpPattern::Up, 4, 9, &mut rng),
        vec![0, 1, 2, 3, 0, 1, 2, 3, 0]
    );
    assert_eq!(
        arp_advance_sequence(ArpPattern::Up, 8, 10, &mut rng),
        vec![0, 1, 2, 3, 4, 5, 6, 7, 0, 1]
    );
    assert_eq!(
        arp_advance_sequence(ArpPattern::Up, 12, 13, &mut rng),
        vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0]
    );
}

#[test]
fn arp_pattern_down_cycles_descending() {
    let mut rng = StdRng::seed_from_u64(0);
    assert_eq!(
        arp_advance_sequence(ArpPattern::Down, 4, 9, &mut rng),
        vec![0, 3, 2, 1, 0, 3, 2, 1, 0]
    );
    assert_eq!(
        arp_advance_sequence(ArpPattern::Down, 8, 5, &mut rng),
        vec![0, 7, 6, 5, 4]
    );
}

#[test]
fn arp_pattern_up_down_ping_pongs_without_repeating_endpoints() {
    let mut rng = StdRng::seed_from_u64(0);
    // len 4: period is 2*(len-1) = 6, bouncing at 0 and 3 without doubling.
    assert_eq!(
        arp_advance_sequence(ArpPattern::UpDown, 4, 13, &mut rng),
        vec![0, 1, 2, 3, 2, 1, 0, 1, 2, 3, 2, 1, 0]
    );
    // A single-tone list (len 1) never bounces off itself.
    assert_eq!(
        arp_advance_sequence(ArpPattern::UpDown, 1, 4, &mut rng),
        vec![0, 0, 0, 0]
    );
}

#[test]
fn arp_random_pattern_is_deterministic_for_a_seed_and_differs_across_seeds() {
    let seq_a = arp_advance_sequence(ArpPattern::Random, 8, 20, &mut StdRng::seed_from_u64(5));
    let seq_b = arp_advance_sequence(ArpPattern::Random, 8, 20, &mut StdRng::seed_from_u64(5));
    let seq_c = arp_advance_sequence(ArpPattern::Random, 8, 20, &mut StdRng::seed_from_u64(6));

    assert_eq!(seq_a, seq_b, "same seed must reproduce the same sequence");
    assert_ne!(
        seq_a, seq_c,
        "different seeds should (almost always) diverge"
    );
    assert!(
        seq_a.iter().all(|&i| i < 8),
        "random index must stay in range"
    );
}

#[test]
fn arp_engine_reseed_via_fluid_engine_reproduces_random_pattern() {
    // Mirrors FluidEngine::reseed's per-voice offset convention (`seed + 5`
    // for arp) so `nooise render --seed N` stays byte-identical when the
    // pattern control is Random.
    let mut a = ArpEngine::new(SAMPLE_RATE);
    a.rng = StdRng::seed_from_u64(9u64.wrapping_add(5));
    let mut b = ArpEngine::new(SAMPLE_RATE);
    b.rng = StdRng::seed_from_u64(9u64.wrapping_add(5));

    let pad = PadControls::default();
    let controls = ArpControls {
        gain: 0.5,
        pattern: 3.0, // Random
        ..ArpControls::default()
    };

    let total = SAMPLE_RATE as u64 * 2;
    let mut out_a = Vec::with_capacity(total as usize * 2);
    let mut out_b = Vec::with_capacity(total as usize * 2);
    for sample in 0..total {
        let (l, r) = a.next(&controls, &pad, 0.0, timing(sample, 120.0));
        out_a.push(l);
        out_a.push(r);
        let (l, r) = b.next(&controls, &pad, 0.0, timing(sample, 120.0));
        out_b.push(l);
        out_b.push(r);
    }

    assert_eq!(
        out_a, out_b,
        "identical reseed must render byte-identical audio"
    );
}

#[test]
fn arp_cycle_notes_duplicates_chord_up_whole_octaves_sorted_extra() {
    // arp_cycle_notes is exercised above via arp_advance_sequence's `len`
    // arguments; this covers the actual chord->list construction directly.
    let chord = [45, 48, 52, 55];
    assert_eq!(arp_cycle_notes(chord, 1).len(), 4);
    assert_eq!(arp_cycle_notes(chord, 2).len(), 8);
    assert_eq!(arp_cycle_notes(chord, 3).len(), 12);
    assert!(arp_cycle_notes(chord, 2).is_sorted());
}

#[test]
fn arp_chord_change_clamps_cycle_position_without_resetting_it() {
    let mut arp = ArpEngine::new(SAMPLE_RATE);
    // Simulate having played deep into a 3-octave (12-tone) cycle.
    arp.cycle_pos = 9;

    let pad = PadControls::default();
    let narrow = ArpControls {
        gain: 0.5,
        octaves: 1.0, // now only a 4-tone list
        pattern: 0.0, // Up, so the post-clamp advance is easy to check
        ..ArpControls::default()
    };

    arp.next(&narrow, &pad, 0.0, timing(0, 120.0));

    // Clamped into range (index 3, the top of a 4-tone list) rather than
    // reset to 0, then advanced one step by the Up pattern: (3+1)%4 = 0.
    assert_eq!(arp.cycle_pos, 0);
}

#[test]
fn arp_decay_sets_note_ring_independent_of_step() {
    // A note's life is `attack + decay`, fully decoupled from the step grid.
    // A short-decay note ends well before the next step; a long-decay note
    // rings past it. The step only spaces the triggers.
    let pad = PadControls::default();
    let t0 = timing(0, 120.0);
    // One long, isolated step so the next trigger is far away and can't be
    // mistaken for a still-ringing note.
    let step_samples = t0.beats_to_samples(ARP_RATE_BEATS_MAX);
    let base = ArpControls {
        gain: 0.5,
        rate_beats: ARP_RATE_BEATS_MAX,
        pattern: 0.0,
        ..ArpControls::default()
    };

    let mut short = ArpEngine::new(SAMPLE_RATE);
    let mut long = ArpEngine::new(SAMPLE_RATE);
    let short_controls = ArpControls {
        decay: 0.1,
        ..base.clone()
    };
    let long_controls = ArpControls { decay: 3.0, ..base };

    short.next(&short_controls, &pad, 0.0, t0);
    long.next(&long_controls, &pad, 0.0, t0);
    assert_eq!(short.voices.len(), 1);
    assert_eq!(long.voices.len(), 1);

    // Advance each voice up to the next step boundary (the `next` calls above
    // already consumed one sample) without letting the engine trigger a new
    // note.
    for _ in 0..step_samples {
        short.voices[0].next();
        long.voices[0].next();
    }

    assert!(
        short.voices[0].is_done(),
        "a short-decay arp note must end long before the next step"
    );
    assert!(
        !long.voices[0].is_done(),
        "a long decay must keep the arp note ringing past its step"
    );
}

#[test]
fn arp_defaults_are_silent_and_do_not_change_default_render() {
    let controls = FluidControls::default();
    assert_close(controls.arp.gain, 0.0);
}

#[test]
fn arp_reuses_shared_ambient_reverb_send_alongside_pad_and_tonal() {
    let mut send = AmbientReverbSend::new(SAMPLE_RATE);
    let arp_mix = ArpControls::default().reverb_mix;
    let frame = send.process((0.0, 0.0), (0.0, 0.0), (1.0, -1.0), 0.0, 0.0, arp_mix);
    assert_near(frame.arp_l, AmbientReverbSend::dry_gain(arp_mix));
    assert_near(frame.arp_r, -AmbientReverbSend::dry_gain(arp_mix));
}

#[test]
fn toggle_mute_zeroes_and_restores_the_track_level() {
    let mut c = FluidControls::default();
    c.perc.level = 0.65;
    let controls = Arc::new(ArcSwap::from_pointee(c));
    let mut mute: MuteState = [None; 9];

    toggle_mute(&controls, Tab::Perc, &mut mute);
    assert_close(controls.load().perc.level, 0.0);
    assert!(mute[Tab::Perc as usize].is_some());

    toggle_mute(&controls, Tab::Perc, &mut mute);
    assert_close(controls.load().perc.level, 0.65);
    assert!(mute[Tab::Perc as usize].is_none());
}

#[test]
fn toggle_mute_on_master_is_independent_of_track_mute() {
    let mut c = FluidControls::default();
    c.master.level = 0.8;
    c.bass.level = 0.5;
    let controls = Arc::new(ArcSwap::from_pointee(c));
    let mut mute: MuteState = [None; 9];

    toggle_mute(&controls, Tab::Master, &mut mute);
    assert_close(controls.load().master.level, 0.0);
    assert_close(controls.load().bass.level, 0.5);

    toggle_mute(&controls, Tab::Bass, &mut mute);
    assert_close(controls.load().bass.level, 0.0);
    // Master stays muted; muting bass didn't disturb it or restore it early.
    assert_close(controls.load().master.level, 0.0);

    toggle_mute(&controls, Tab::Master, &mut mute);
    assert_close(controls.load().master.level, 0.8);
    assert_close(controls.load().bass.level, 0.0);
}

#[test]
fn toggle_mute_on_macros_tab_is_a_no_op() {
    let c = FluidControls::default();
    let controls = Arc::new(ArcSwap::from_pointee(c));
    let mut mute: MuteState = [None; 9];

    toggle_mute(&controls, Tab::Macros, &mut mute);
    assert!(mute[Tab::Macros as usize].is_none());
}

#[test]
fn render_shows_a_mute_marker_on_muted_tabs_only() {
    let controls = FluidControls::default();
    let fluid = FluidState::new();
    let items = tab_controls(Tab::Bass, &controls);
    let automation = AutomationState::default();
    let mut mute: MuteState = [None; 9];
    mute[Tab::Perc as usize] = Some(0.7);

    let backend = TestBackend::new(120, 44);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render(
                f,
                &items,
                Tab::Bass,
                0,
                0,
                0.0,
                NumericDisplay::empty(),
                &fluid,
                &automation,
                &controls,
                None,
                &FlippedUnits::new(),
                ChordDrill::None,
                &mute,
            )
        })
        .unwrap();

    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("Perc (M)"), "muted tab must show a marker");
    assert!(
        !text.contains("Bass (M)"),
        "unmuted tab must not show a marker"
    );
}
