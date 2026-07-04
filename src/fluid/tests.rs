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
    let wrapped_progression = pad_chord(4, 0, 0.0);
    let base_progression = pad_chord(0, 0, 0.0);
    assert_eq!(wrapped_progression, base_progression);

    let wrapped_step = pad_chord(0, 8, 0.0);
    let base_step = pad_chord(0, 0, 0.0);
    assert_eq!(wrapped_step, base_step);
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
        let mut tonal = TonalEngine::new(SAMPLE_RATE, Arc::new(FluidTelemetry::default()));

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
    let mut tonal = TonalEngine::new(SAMPLE_RATE, Arc::new(FluidTelemetry::default()));

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
    let mut tonal = TonalEngine::new(SAMPLE_RATE, Arc::new(FluidTelemetry::default()));
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
    let mut tonal = TonalEngine::new(SAMPLE_RATE, Arc::new(FluidTelemetry::default()));
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
}

#[test]
fn tab_previous_wraps_back_one_tab() {
    assert_eq!(Tab::Master.previous(), Tab::Clap);
    assert_eq!(Tab::Kick.previous(), Tab::Bass);
    assert_eq!(Tab::Bass.previous(), Tab::Chords);
}

#[test]
fn bass_spatial_freq_rises_with_pitch() {
    // A low note gives a long wavelength (fewer rings); higher notes pack more.
    let low = bass_spatial_freq(midi_to_hz(33)); // A0-ish, deep bass
    let mid = bass_spatial_freq(midi_to_hz(45)); // A2
    let high = bass_spatial_freq(midi_to_hz(57)); // A3
    assert!(low < mid, "expected {low} < {mid}");
    assert!(mid < high, "expected {mid} < {high}");
    // Clamped to a bounded, positive range for the field loop.
    assert!(bass_spatial_freq(1.0) >= 3.0);
    assert!(bass_spatial_freq(20_000.0) <= 16.0);
}

#[test]
fn tonal_node_y_rises_for_higher_notes() {
    // Higher pitch sits higher on the screen (smaller y).
    let low = tonal_node_y(midi_to_hz(45));
    let mid = tonal_node_y(midi_to_hz(57));
    let high = tonal_node_y(midi_to_hz(67));
    assert!(high < mid, "expected {high} < {mid}");
    assert!(mid < low, "expected {mid} < {low}");
    // Stays within the visible field.
    assert!((0.0..=1.0).contains(&tonal_node_y(midi_to_hz(45))));
    assert!((0.0..=1.0).contains(&tonal_node_y(midi_to_hz(67))));
}

#[test]
fn silent_field_stays_dark() {
    // With no telemetry activity every node is still; the field brightness
    // must stay near the ambient floor so a listener can tell nothing plays.
    let telemetry = FluidTelemetry::default();
    let mut fluid = FluidState::new();
    for _ in 0..30 {
        fluid.tick(0.05, &telemetry);
    }
    let mut peak = 0.0f32;
    for iy in 0..20 {
        for ix in 0..20 {
            let v = fluid.field(ix as f32 / 20.0, iy as f32 / 20.0).value;
            peak = peak.max(v);
        }
    }
    assert!(peak < 0.02, "silent field should be black, peak was {peak}");
}

#[test]
fn triggers_without_level_draw_nothing() {
    // The sequencer keeps firing pulses when voices are muted; with all
    // levels at zero those pulses must not paint anything.
    let telemetry = FluidTelemetry::default();
    let mut fluid = FluidState::new();
    for i in 1..=16u64 {
        use std::sync::atomic::Ordering::Relaxed;
        telemetry.kick_pulse.store(i, Relaxed);
        telemetry.tonal_pulse.store(i, Relaxed);
        telemetry.perc_pulse.store(i, Relaxed);
        telemetry.clap_pulse.store(i, Relaxed);
        fluid.tick(0.05, &telemetry);
    }
    let mut peak = 0.0f32;
    for iy in 0..20 {
        for ix in 0..20 {
            peak = peak.max(fluid.field(ix as f32 / 20.0, iy as f32 / 20.0).value);
        }
    }
    assert!(peak < 0.02, "muted triggers must stay dark, peak was {peak}");
}

#[test]
fn kick_wave_rises_from_bottom_edge() {
    // A kick hit with live level injects a coherent wavefront at the bottom.
    let telemetry = FluidTelemetry::default();
    telemetry.publish_levels(VoiceLevels {
        kick: 0.3,
        ..Default::default()
    });
    let mut fluid = FluidState::new();
    fluid.tick(0.05, &telemetry);
    telemetry
        .kick_pulse
        .store(1, std::sync::atomic::Ordering::Relaxed);
    fluid.tick(0.05, &telemetry);
    let bottom = fluid.field(0.5, 0.93).value;
    let top = fluid.field(0.5, 0.10).value;
    assert!(
        bottom > top + 0.1,
        "kick front should light the bottom (bottom {bottom}, top {top})"
    );
}

#[test]
fn active_voice_lights_its_region() {
    // Publish a strong bass level; the bass node's home region must brighten
    // well above a far-away silent region.
    let telemetry = FluidTelemetry::default();
    telemetry.publish_bass_note(midi_to_hz(45));
    telemetry.publish_levels(VoiceLevels {
        bass: 1.0,
        ..Default::default()
    });
    let mut fluid = FluidState::new();
    for _ in 0..40 {
        fluid.tick(0.05, &telemetry);
    }
    // Sample the brightest cell near the bass home (0.5, 0.80).
    let mut near_peak = 0.0f32;
    for iy in 14..18 {
        for ix in 8..12 {
            let v = fluid.field(ix as f32 / 20.0, iy as f32 / 20.0).value;
            near_peak = near_peak.max(v);
        }
    }
    let far = fluid.field(0.05, 0.05).value;
    assert!(
        near_peak > far + 0.2,
        "bass region ({near_peak}) should outshine a far corner ({far})"
    );
}

#[test]
fn tonal_note_is_surface_only() {
    // A tonal note must appear on the surface layer at its pitch height and
    // contribute nothing to the fluid field.
    let telemetry = FluidTelemetry::default();
    telemetry.publish_tonal_note(440.0);
    telemetry.publish_levels(VoiceLevels {
        tonal: 0.3,
        ..Default::default()
    });
    let mut fluid = FluidState::new();
    fluid.tick(0.05, &telemetry);
    telemetry
        .tonal_pulse
        .store(1, std::sync::atomic::Ordering::Relaxed);
    fluid.tick(0.05, &telemetry);

    let mut field_peak = 0.0f32;
    let mut spark_hits = 0;
    for iy in 0..40 {
        for ix in 0..40 {
            let (nx, ny) = (ix as f32 / 40.0, iy as f32 / 40.0);
            field_peak = field_peak.max(fluid.field(nx, ny).value);
            if fluid.surface(nx, ny).is_some() {
                spark_hits += 1;
            }
        }
    }
    assert!(field_peak < 0.02, "tonal leaked into the field: {field_peak}");
    assert!(spark_hits > 0, "tonal spark missing from the surface layer");
}

#[test]
fn muted_perc_spawns_no_surface_spark() {
    let telemetry = FluidTelemetry::default();
    let mut fluid = FluidState::new();
    telemetry
        .perc_pulse
        .store(1, std::sync::atomic::Ordering::Relaxed);
    fluid.tick(0.05, &telemetry);
    for iy in 0..40 {
        for ix in 0..40 {
            assert!(
                fluid
                    .surface(ix as f32 / 40.0, iy as f32 / 40.0)
                    .is_none(),
                "muted perc must draw nothing"
            );
        }
    }
}

#[test]
fn spark_dies_when_its_voice_decays() {
    // Glint brightness must track the voice's live envelope, not a fixed
    // visual clock: when the perc sound decays to silence, the glint goes
    // dark with it even though its lifetime has barely started.
    let telemetry = FluidTelemetry::default();
    telemetry.publish_levels(VoiceLevels {
        perc: 0.3,
        ..Default::default()
    });
    let mut fluid = FluidState::new();
    telemetry
        .perc_pulse
        .store(1, std::sync::atomic::Ordering::Relaxed);
    fluid.tick(0.05, &telemetry);
    let lit = |fluid: &FluidState| {
        (0..40).any(|iy| {
            (0..40).any(|ix| fluid.surface(ix as f32 / 40.0, iy as f32 / 40.0).is_some())
        })
    };
    assert!(lit(&fluid), "perc glint should appear while the hit sounds");

    telemetry.publish_levels(VoiceLevels::default());
    for _ in 0..3 {
        fluid.tick(0.05, &telemetry);
    }
    assert!(
        !lit(&fluid),
        "glint must go dark when the perc envelope reaches zero"
    );
}

#[test]
fn sustained_tonal_note_keeps_its_spark_alive() {
    // A long tonal decay keeps sounding past a second; its spark must stay
    // visible for as long as the envelope holds, showing the note's length.
    let telemetry = FluidTelemetry::default();
    telemetry.publish_tonal_note(440.0);
    telemetry.publish_levels(VoiceLevels {
        tonal: 0.3,
        ..Default::default()
    });
    let mut fluid = FluidState::new();
    telemetry
        .tonal_pulse
        .store(1, std::sync::atomic::Ordering::Relaxed);
    for _ in 0..26 {
        fluid.tick(0.05, &telemetry);
    }
    let lit = (0..40).any(|iy| {
        (0..40).any(|ix| fluid.surface(ix as f32 / 40.0, iy as f32 / 40.0).is_some())
    });
    assert!(lit, "a sounding tonal note must keep its spark visible");
}

#[test]
fn kick_wave_is_radial_from_a_bottom_point() {
    // The kick wave radiates from a point near the bottom, not a full-width
    // band: at the same height, cells far from the origin stay dark.
    let telemetry = FluidTelemetry::default();
    telemetry.publish_levels(VoiceLevels {
        kick: 0.3,
        ..Default::default()
    });
    let mut fluid = FluidState::new();
    fluid.tick(0.05, &telemetry);
    telemetry
        .kick_pulse
        .store(1, std::sync::atomic::Ordering::Relaxed);
    fluid.tick(0.05, &telemetry);

    let row: Vec<f32> = (0..=40)
        .map(|ix| fluid.field(ix as f32 / 40.0, 0.93).value)
        .collect();
    let (ci, peak) = row
        .iter()
        .enumerate()
        .map(|(i, &v)| (i, v))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let cx = ci as f32 / 40.0;
    let far_x = if cx < 0.5 { cx + 0.45 } else { cx - 0.45 };
    let far = fluid.field(far_x, 0.93).value;
    assert!(
        peak > far + 0.15,
        "kick wave should be local to its origin (peak {peak} at x {cx}, far {far})"
    );
}

#[test]
fn pad_flow_pattern_differs_by_chord() {
    // Each chord shapes the pad flow with its own wave character, not just a
    // hue swap: the brightness pattern itself must differ between chords.
    let grid = |chord: u64| {
        let telemetry = FluidTelemetry::default();
        telemetry
            .chord_index
            .store(chord, std::sync::atomic::Ordering::Relaxed);
        telemetry.publish_levels(VoiceLevels {
            pad: 0.2,
            ..Default::default()
        });
        let mut fluid = FluidState::new();
        for _ in 0..40 {
            fluid.tick(0.05, &telemetry);
        }
        let mut cells = Vec::with_capacity(400);
        for iy in 0..20 {
            for ix in 0..20 {
                cells.push(fluid.field(ix as f32 / 20.0, iy as f32 / 20.0).value);
            }
        }
        cells
    };
    let a = grid(0);
    let b = grid(1);
    let max_diff = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_diff > 0.05,
        "chords should shape the flow differently, max diff {max_diff}"
    );
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
                None,
                false,
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
    for _ in 0..100 {
        route.adjust_field_at(LfoField::Interval, 1.0, 0.0);
    }
    assert_close(route.cycle_beats, 16.0);
    for _ in 0..100 {
        route.adjust_field_at(LfoField::Interval, -1.0, 0.0);
    }
    assert_close(route.cycle_beats, 0.25);

    route.adjust_field_at(LfoField::Offset, -1.0, 0.0);
    assert_close(route.phase_offset_beats, 0.0);
    route.adjust_field_at(LfoField::Offset, 1.0, 0.0);
    assert_close(route.phase_offset_beats, 0.25);
    for _ in 0..100 {
        route.adjust_field_at(LfoField::Offset, 1.0, 0.0);
    }
    assert_close(route.phase_offset_beats, 4.0);
}

#[test]
fn lfo_field_set_snaps_to_quarter_beat_grid() {
    let mut route = LfoRoute::default();

    route.set_field_at(LfoField::Interval, 3.1, 0.0);
    assert_close(route.cycle_beats, 3.0);
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
fn engine_publishes_beat_telemetry() {
    let controls = Arc::new(ArcSwap::from_pointee(FluidControls::default()));
    let automation = Arc::new(ArcSwap::from_pointee(AutomationState::default()));
    let telemetry = Arc::new(FluidTelemetry::default());
    let bpm = f64::from(controls.load().master.bpm);
    let mut engine = FluidEngine::new(44_100.0, controls, automation, Arc::clone(&telemetry));

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

#[test]
fn ambient_reverb_send_ducks_dry_sources_by_mix() {
    let mut send = AmbientReverbSend::new(SAMPLE_RATE);

    let frame = send.process((1.0, -1.0), (0.5, -0.5), 1.0, 0.5);

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
            let frame = send.process(dry, (0.0, 0.0), controls.reverb_mix, 0.0);
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
            note_length_beats: 1.0,
            step_interval_beats: 4.0,
            reverb_mix,
            ..TonalControls::default()
        };
        let mut tonal = TonalEngine::new(SAMPLE_RATE, Arc::new(FluidTelemetry::default()));
        tonal.rng = StdRng::seed_from_u64(11);
        let mut send = AmbientReverbSend::new(SAMPLE_RATE);
        let mut sum = 0.0;
        let mut count = 0;
        let total = SAMPLE_RATE as u64 * 4;
        let warmup = SAMPLE_RATE as u64;

        for sample in 0..total {
            let dry = tonal.next(&controls, 0.0, timing(sample, 120.0));
            let frame = send.process((0.0, 0.0), dry, 0.0, controls.reverb_mix);
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
                    None,
                    false,
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
    assert_close(controls.tonal.note_length_beats, 1.5);
    assert_close(controls.tonal.randomness, 0.5);
    assert_close(controls.tonal.evolve_rate, 0.0);

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
        (Tab::Perc, vec![Gain, Timing, Timing, Timing, Gain]),
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
                Gain, Timing, Timing, Continuous, Timing, Timing, Gain, Gain, Gain, Timing, Gain,
                Gain, Gain,
            ],
        ),
        (
            Tab::Tonal,
            vec![
                Gain, Discrete, Discrete, Timing, Timing, Timing, Gain, Continuous, Timing, Gain,
            ],
        ),
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
            assert!(!spec.id.is_empty(), "{ctx}: empty stable id");
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
fn song_code_round_trips_quantized_snapshot_values() {
    let mut controls = FluidControls::default();
    controls.master.bpm = 123.4;
    controls.pad.chord_bars = 12.0;
    controls.kick.echo_time_beats = 0.33;
    controls.clap.slap_count = 6.6;

    let code = song::encode_song_code(&SongState::from_controls(controls)).unwrap();
    let decoded = song::decode_song_code(&code).unwrap();

    assert_close(decoded.controls.master.bpm, 123.0);
    assert_close(decoded.controls.pad.chord_bars, 16.0);
    assert_close(decoded.controls.kick.echo_time_beats, 0.375);
    assert_close(decoded.controls.clap.slap_count, 7.0);
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
    controls.kick.click = 0.0;
    controls.kick.drive = 0.0;
    controls.kick.filter = 0.0;
    controls.kick.echo_amount = 0.0;
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
    controls.kick.echo_amount = 0.9;
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
    assert!(next.kick.echo_amount > 0.0 && next.kick.echo_amount < 0.9);
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
    assert_eq!(rows[2].label, "Progression");
    assert_eq!(rows[2].display, "A");

    controls.pad.progression = 2.0;
    let rows = tab_controls(Tab::Chords, &controls);
    assert_eq!(rows[2].display, "C");
}

#[test]
fn tonal_tab_separates_rate_from_cycle() {
    let rows = tab_controls(Tab::Tonal, &FluidControls::default());

    assert_eq!(rows[1].id, "tonal.synth_type");
    assert_eq!(rows[1].label, "Type");
    assert_eq!(rows[1].display, "Sine");
    assert_eq!(rows[2].id, "tonal.phrase");
    assert_eq!(rows[2].label, "Phrase");
    assert_eq!(rows[3].id, "tonal.rate_beats");
    assert_eq!(rows[3].label, "Rate");
    assert_eq!(rows[3].display, "0.50 beats");
    assert_eq!(rows[4].id, "tonal.step_interval_beats");
    assert_eq!(rows[4].label, "Cycle");
    assert_eq!(rows[4].display, "16.00 beats");
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
    let mut bass = BassEngine::new(sample_rate, Arc::new(FluidTelemetry::default()));
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
    let controls = PercControls {
        level: 1.0,
        interval_beats: 4.25,
        ..Default::default()
    };

    let mut engine = PercEngine::new(SAMPLE_RATE, Arc::new(FluidTelemetry::default()));
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

    let mut engine = PercEngine::new(SAMPLE_RATE, Arc::new(FluidTelemetry::default()));
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
    assert_eq!(rows.len(), 5);
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
    assert!(
        max_gap <= 2.0,
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
    };

    // Peak of the sine: raw value 1.0 + 0.4 * 3.75 = 2.5 must land on 2.0.
    let peak = modulated_control_value(spec, &route, 1.0, 2.0);
    assert_close(peak, 2.0);

    // Trough: 1.0 - 1.5 clamps to the minimum subdivision.
    let trough = modulated_control_value(spec, &route, 1.0, 6.0);
    assert_close(trough, 0.25);
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
            [0.25f32, 0.5, 1.0, 2.0, 4.0]
                .iter()
                .any(|&q| (v - q).abs() < 1e-4),
            "modulated interval {v} is not a power-of-two subdivision"
        );
    }
}

#[test]
fn lfo_offset_modulation_snaps_to_quarter_beats() {
    let mut controls = FluidControls::default();
    controls.kick.offset_beats = 2.0;
    let automation = automation_with_route("kick.offset_beats", 0.4, 8.0);

    for sample in (0..(SAMPLE_RATE as u64 * 16)).step_by(64) {
        let mut effective = controls.clone();
        apply_automation(&mut effective, &automation, timing(sample, 120.0));
        let v = effective.kick.offset_beats;
        let snapped = (v / 0.25).round() * 0.25;
        assert!(
            (v - snapped).abs() < 1e-4,
            "modulated offset {v} is not on the 0.25-beat grid"
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
        if trigger.pop(t, effective.kick.interval_beats, effective.kick.offset_beats) {
            hit_beats.push(t.beat);
        }
    }

    // Every hit stays locked to the absolute 16th grid.
    for &beat in &hit_beats {
        let snapped = (beat / 0.25).round() * 0.25;
        assert!(
            (beat - snapped).abs() < 1e-3,
            "hit at beat {beat:.4} is off the 0.25 grid"
        );
    }

    // The sweep actually breaks down through multiple subdivisions.
    let mut gaps: Vec<i64> = hit_beats
        .windows(2)
        .map(|w| ((w[1] - w[0]) / 0.25).round() as i64)
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
