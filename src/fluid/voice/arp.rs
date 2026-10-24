//! The Arp voice: cycles the Pad's current chord tones on its own grid.

use super::*;
use crate::fluid::osc::{OscVoiceEvent, OscVoiceSink, tuned_note};

pub(crate) const ARP_RATE_BEATS_MIN: f32 = 0.125;
pub(crate) const ARP_RATE_BEATS_MAX: f32 = 4.0;
pub(crate) const ARP_OCTAVES_MIN: f32 = 1.0;
pub(crate) const ARP_OCTAVES_MAX: f32 = 3.0;
pub(crate) const ARP_CHORD_TONES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArpPattern {
    Up,
    Down,
    UpDown,
    Random,
    Custom,
}

/// Patterns in stored-value order; `arp.pattern` wraps into this table.
const ARP_PATTERNS: [ArpPattern; 5] = [
    ArpPattern::Up,
    ArpPattern::Down,
    ArpPattern::UpDown,
    ArpPattern::Random,
    ArpPattern::Custom,
];

pub(crate) fn arp_pattern_from_control(value: f32) -> ArpPattern {
    ARP_PATTERNS[wrapped_index(value, ARP_PATTERNS.len())]
}

pub(crate) fn arp_pattern_label(value: f32) -> &'static str {
    match arp_pattern_from_control(value) {
        ArpPattern::Up => "Up",
        ArpPattern::Down => "Down",
        ArpPattern::UpDown => "Up-Down",
        ArpPattern::Random => "Random",
        ArpPattern::Custom => "Custom",
    }
}

pub(crate) const ARP_STEP_LABELS: [&str; ARP_TONE_COUNT] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12",
];

pub(crate) fn arp_live_step_count(value: f32) -> usize {
    (value.round() as i64).clamp(1, ARP_STEP_COUNT as i64) as usize
}

pub(crate) fn arp_step_is_tie(value: f32) -> bool {
    (value.round() as i64).clamp(-1, ARP_TONE_COUNT as i64) < 0
}

pub(crate) fn arp_step_tone(value: f32) -> Option<usize> {
    match (value.round() as i64).clamp(-1, ARP_TONE_COUNT as i64) {
        v if v <= 0 => None,
        tone => Some((tone - 1) as usize),
    }
}

pub(crate) fn arp_step_label(value: f32) -> &'static str {
    if arp_step_is_tie(value) {
        "Tie"
    } else {
        match arp_step_tone(value) {
            None => "Rest",
            Some(tone) => ARP_STEP_LABELS[tone],
        }
    }
}

pub(crate) fn arp_octave_span(value: f32) -> usize {
    (value.round() as i32).clamp(ARP_OCTAVES_MIN as i32, ARP_OCTAVES_MAX as i32) as usize
}

pub(crate) fn arp_generate_steps(pattern: ArpPattern, octaves: f32) -> (u8, [f32; ARP_STEP_COUNT]) {
    let oct = arp_octave_span(octaves);
    let cycle_len = (oct * ARP_CHORD_TONES).clamp(1, ARP_TONE_COUNT);
    let len = if oct >= 3 { 12 } else { 8 };
    let mut rng = StdRng::seed_from_u64(0xA470_5EED ^ (cycle_len as u64));
    let mut pos = if matches!(pattern, ArpPattern::Down) {
        cycle_len.saturating_sub(1)
    } else {
        0
    };
    let mut dir = 1i32;
    let mut steps = [1.0; ARP_STEP_COUNT];
    for step in &mut steps {
        *step = (pos + 1) as f32;
        let (next_pos, next_dir) = arp_advance(
            pos,
            match pattern {
                ArpPattern::Custom => ArpPattern::Up,
                other => other,
            },
            cycle_len,
            dir,
            &mut rng,
        );
        pos = next_pos;
        dir = next_dir;
    }
    (len, steps)
}

/// Build the cycled tone list for a chord: the 4 chord tones duplicated up
/// whole octaves (+12, +24 semitones) for each extra octave of span, sorted
/// ascending. Span 1 keeps just the 4 chord tones; span 3 yields 12 tones.
pub(crate) fn arp_cycle_notes(chord: [i32; ARP_CHORD_TONES], octaves: usize) -> Vec<i32> {
    let mut notes = Vec::with_capacity(ARP_CHORD_TONES * octaves.max(1));
    for octave in 0..octaves.max(1) {
        for tone in chord {
            notes.push(tone + 12 * octave as i32);
        }
    }
    notes.sort_unstable();
    notes
}

/// Advance the ping-pong (up-down) cursor by one step without repeating
/// either endpoint: bounces at 0 and `len - 1`, reversing direction there.
pub(crate) fn arp_ping_pong_advance(pos: usize, dir: i32, len: usize) -> (usize, i32) {
    if len <= 1 {
        return (0, dir);
    }
    let proposed = pos as i32 + dir;
    if proposed < 0 {
        (1, 1)
    } else if proposed >= len as i32 {
        (len - 2, -1)
    } else {
        (proposed as usize, dir)
    }
}

/// Compute the next cycle position (and, for Up-Down, the next travel
/// direction) after emitting the tone at `pos`. `len` is the current cycle
/// list length; `rng` is only consumed by the Random pattern. Shared by
/// `ArpEngine::next` and its tests so pattern-sequencing logic is exercised
/// exactly as production runs it.
pub(crate) fn arp_advance(
    pos: usize,
    pattern: ArpPattern,
    len: usize,
    dir: i32,
    rng: &mut StdRng,
) -> (usize, i32) {
    match pattern {
        ArpPattern::Up | ArpPattern::Custom => ((pos + 1) % len, dir),
        ArpPattern::Down => ((pos + len - 1) % len, dir),
        ArpPattern::UpDown => arp_ping_pong_advance(pos, dir, len),
        ArpPattern::Random => (rng.gen_range(0..len), dir),
    }
}

pub(crate) struct ArpEngine {
    pub(crate) sample_rate: f32,
    pub(crate) progression: ProgressionFollower,
    pub(crate) note_trigger: GridTrigger,
    pub(crate) cycle_pos: usize,
    pub(crate) ping_pong_dir: i32,
    pub(crate) step_index: usize,
    pub(crate) last_cycle: Option<u64>,
    pub(crate) active_steps_source: [f32; ARP_STEP_COUNT],
    pub(crate) evolved_steps: [f32; ARP_STEP_COUNT],
    pub(crate) evolution_seed: u64,
    pub(crate) evolution_count: u64,
    pub(crate) session_state: Option<LiveSession>,
    pub(crate) voices: Vec<TonalVoice>,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
    osc: OscVoiceSink,
    osc_suppressed: bool,
    osc_active_until_beat: Option<f64>,
    input_held: [bool; 128],
}

impl ArpEngine {
    #[cfg(test)]
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self::with_telemetry(sample_rate, Arc::new(FluidTelemetry::default()))
    }

    #[cfg(test)]
    pub(crate) fn with_telemetry(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
        Self::new_with_live_state(sample_rate, None, telemetry)
    }

    pub(crate) fn new_with_live_state(
        sample_rate: f32,
        session_state: Option<LiveSession>,
        telemetry: Arc<FluidTelemetry>,
    ) -> Self {
        let mut rng = StdRng::from_entropy();
        let evolution_seed = rng.r#gen();
        Self {
            sample_rate,
            progression: ProgressionFollower::new(),
            note_trigger: GridTrigger::new(),
            cycle_pos: 0,
            ping_pong_dir: 1,
            step_index: 0,
            last_cycle: None,
            active_steps_source: DEFAULT_ARP_STEPS,
            evolved_steps: DEFAULT_ARP_STEPS,
            evolution_seed,
            evolution_count: 0,
            session_state,
            voices: Vec::with_capacity(8),
            rng,
            telemetry,
            osc: OscVoiceSink::default(),
            osc_suppressed: false,
            osc_active_until_beat: None,
            input_held: [false; 128],
        }
    }

    fn evolve_steps(&mut self, live_len: usize, max_tone: usize, evolve_rate: f32, cycle: u64) {
        let live_len = live_len.clamp(1, ARP_STEP_COUNT);
        let max_tone = max_tone.clamp(1, ARP_TONE_COUNT) as i32;
        let mutations = ((evolve_rate.clamp(0.0, 1.0) * 3.0).ceil() as usize).max(1);
        let seed = self
            .evolution_seed
            .wrapping_add(
                self.evolution_count
                    .wrapping_add(1)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15),
            )
            .wrapping_add(cycle.rotate_left(13));
        let mut evo_rng = StdRng::seed_from_u64(seed);
        for _ in 0..mutations {
            let idx = evo_rng.gen_range(0..live_len);
            let current = (self.evolved_steps[idx].round() as i32).clamp(-1, max_tone);
            let next = if current <= 0 {
                evo_rng.gen_range(1..=max_tone)
            } else if idx > 0 && evo_rng.gen_bool(0.08) {
                -1
            } else if evo_rng.gen_bool(0.10) {
                0
            } else {
                let delta = if evo_rng.gen_bool(0.5) { 1 } else { -1 };
                (current + delta).rem_euclid(max_tone).max(1)
            };
            self.evolved_steps[idx] = next as f32;
        }
        if self.evolved_steps[0] < 0.0 {
            self.evolved_steps[0] = 1.0;
        }
        self.evolution_count = self.evolution_count.saturating_add(1);
        if let Some(session_state) = &self.session_state {
            let steps = self.evolved_steps;
            session_state.update(|snapshot| {
                snapshot.controls.arp.steps = steps;
                snapshot.controls.arp.pattern = CUSTOM_ARP_PATTERN_INDEX as f32;
            });
        }
    }

    pub(crate) fn set_osc(&mut self, sink: OscVoiceSink) {
        self.osc = sink;
    }

    pub(crate) fn set_osc_suppressed(&mut self, suppressed: bool) {
        if suppressed && self.osc_active_until_beat.is_some() {
            self.osc.send(OscVoiceEvent::ArpOff);
            self.osc_active_until_beat = None;
        }
        self.osc_suppressed = suppressed;
    }

    pub(crate) fn osc_note_on(&mut self, note: u8) {
        self.input_held[usize::from(note)] = true;
    }

    pub(crate) fn osc_note_off(&mut self, note: u8) {
        self.input_held[usize::from(note)] = false;
    }

    pub(crate) fn set_osc_notes(&mut self, notes: &[u8]) {
        self.input_held.fill(false);
        for &note in notes {
            if note < 128 {
                self.input_held[usize::from(note)] = true;
            }
        }
    }

    pub(crate) fn release_input(&mut self) {
        self.input_held.fill(false);
    }

    pub(crate) fn next(
        &mut self,
        c: &ArpControls,
        pad: &PadControls,
        key: MasterKey,
        timing: TimingContext,
    ) -> (f32, f32) {
        // `pad_chord_tones` is the same chord-source path Pad and Bass
        // resolve through, so a custom progression drives all three
        // identically.
        let (progression, slot) = self.progression.follow(pad, timing);
        let tune = key.shift(progression) as f32;
        let osc_enabled = !self.osc_suppressed && self.osc.is_attached();
        if self.osc_active_until_beat.is_some()
            && (!osc_enabled
                || timing.transport == Transport::Stopped
                || self
                    .osc_active_until_beat
                    .is_some_and(|end| timing.beat >= end))
        {
            self.osc.send(OscVoiceEvent::ArpOff);
            self.osc_active_until_beat = None;
        }

        let rate_beats = c.rate_beats.clamp(ARP_RATE_BEATS_MIN, ARP_RATE_BEATS_MAX);
        if self
            .note_trigger
            .pop_swung(timing, rate_beats, c.offset_beats, c.swing)
        {
            let octaves = arp_octave_span(c.octaves);
            let held: Vec<i32> = self
                .input_held
                .iter()
                .enumerate()
                .filter_map(|(note, held)| held.then_some(note as i32))
                .collect();
            let chord = pad_chord_tones(pad, progression, slot);
            let notes = if held.is_empty() {
                arp_cycle_notes(chord, octaves)
            } else {
                let mut notes = Vec::with_capacity(held.len() * octaves);
                for octave in 0..octaves {
                    for &note in &held {
                        notes.push(note + 12 * octave as i32);
                    }
                }
                notes
            };
            let len = notes.len().max(1);
            // Chord/octave changes never reset the cycle position — just
            // clamp it into the (possibly resized) list so there's no click.
            self.cycle_pos = self.cycle_pos.min(len - 1);
            let pattern = arp_pattern_from_control(c.pattern);
            let current_cycle_pos = self.cycle_pos;
            let (next_pos, next_dir) = arp_advance(
                self.cycle_pos,
                pattern,
                len,
                self.ping_pong_dir,
                &mut self.rng,
            );
            self.cycle_pos = next_pos;
            self.ping_pong_dir = next_dir;

            if c.steps == self.evolved_steps {
                self.active_steps_source = c.steps;
            } else if c.steps != self.active_steps_source {
                self.active_steps_source = c.steps;
                self.evolved_steps = c.steps;
                self.last_cycle = None;
            }
            let live_len = arp_live_step_count(c.step_count);
            let cycle = (self.step_index / live_len) as u64;
            if let Some(prev_cycle) = self.last_cycle
                && cycle > prev_cycle
                && c.evolve_rate > 0.0
            {
                self.evolve_steps(live_len, len, c.evolve_rate, cycle);
            }
            self.last_cycle = Some(cycle);

            let step_idx = lane_step_at(timing.beat, rate_beats, c.offset_beats, live_len);
            self.step_index = self.step_index.wrapping_add(1);
            let step_val = self.evolved_steps[step_idx];
            let pan = self.rng.gen_range(-0.4f32..0.4);

            // Preserve direct struct-literal `ArpControls { pattern: 3.0, ..Default::default() }`
            // unit tests that bypass `apply_pattern_preset`.
            let direct_random =
                pattern == ArpPattern::Random && self.evolved_steps == DEFAULT_ARP_STEPS;

            if !direct_random && arp_step_is_tie(step_val) {
                let extra_seconds = (f64::from(rate_beats) * 60.0 / timing.bpm.max(1.0)) as f32;
                if let Some(last_voice) = self.voices.iter_mut().rev().find(|v| !v.is_done()) {
                    last_voice.extend_decay(extra_seconds);
                }
                if let Some(end) = &mut self.osc_active_until_beat {
                    *end = (*end).max(timing.beat) + f64::from(rate_beats);
                }
            } else if let Some(tone_idx) = if direct_random {
                Some(current_cycle_pos)
            } else {
                arp_step_tone(step_val)
            } {
                let note =
                    if !held.is_empty() || LeadFollow::from_value(c.follow) == LeadFollow::Chord {
                        notes[tone_idx % len]
                    } else {
                        let scale = progression_scale(pad, progression);
                        if tone_idx < LEAD_TONE_COUNT {
                            scale.note(tone_idx + 1, 0.0)
                        } else {
                            scale.note((tone_idx - 7) + 1, 1.0)
                        }
                    };
                let gate_beats = rate_beats.min(0.5);
                if osc_enabled && held.is_empty() {
                    self.osc.send(OscVoiceEvent::ArpNote {
                        note: tuned_note(note, tune),
                        gate_beats,
                    });
                    self.osc_active_until_beat = Some(timing.beat + f64::from(gate_beats));
                }
                let hz = note_hz(note, tune);
                if c.gain != 0.0 {
                    self.voices.push(TonalVoice::new(
                        wrapped_index(c.voice_type, TONAL_SYNTH_TYPES.len()),
                        TonalNote {
                            midi: note,
                            hz,
                            pan,
                            sample_rate: self.sample_rate,
                            attack_time: c.attack,
                            decay_time: c.decay,
                        },
                    ));
                }
                if c.gain != 0.0 || osc_enabled {
                    self.telemetry
                        .publish_hit(MusicalHit::Arp, c.gain, pitch_class(note));
                }
            } else if self.osc_active_until_beat.is_some() {
                self.osc.send(OscVoiceEvent::ArpOff);
                self.osc_active_until_beat = None;
            }
        }

        // Applied to the summed voices, not captured per note, so the fader
        // reaches notes that are already sounding. Pre-smoothed by
        // `ControlSmoothers`.
        let (dry_l, dry_r) =
            mix_and_retain(&mut self.voices, TonalVoice::next, TonalVoice::is_done);
        (dry_l * c.gain, dry_r * c.gain)
    }
}
