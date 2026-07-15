use super::*;

// ============================================================
// Arp engine (follows the Pad's current chord on its own grid)
// ============================================================

pub(crate) const ARP_RATE_BEATS_MIN: f32 = 0.125;
pub(crate) const ARP_RATE_BEATS_MAX: f32 = 4.0;
pub(crate) const ARP_OCTAVES_MIN: f32 = 1.0;
pub(crate) const ARP_OCTAVES_MAX: f32 = 3.0;
pub(crate) const ARP_CHORD_TONES: usize = 4;
/// Fixed synth character for the arp voice — the "Pluck" piano profile
/// (short, dry, staccato), matching `piano_profile`'s type-6 mapping
/// (`TONAL_PIANO_PROFILES[5]`). No user-facing synth-type control; the arp
/// always uses this one warm, non-metallic voice.
pub(crate) const ARP_PROFILE_INDEX: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArpPattern {
    Up,
    Down,
    UpDown,
    Random,
}

pub(crate) fn arp_pattern_index(value: f32) -> usize {
    (value.round() as i64).rem_euclid(4) as usize
}

pub(crate) fn arp_pattern_from_control(value: f32) -> ArpPattern {
    match arp_pattern_index(value) {
        0 => ArpPattern::Up,
        1 => ArpPattern::Down,
        2 => ArpPattern::UpDown,
        _ => ArpPattern::Random,
    }
}

pub(crate) fn arp_pattern_label(value: f32) -> &'static str {
    match arp_pattern_from_control(value) {
        ArpPattern::Up => "Up",
        ArpPattern::Down => "Down",
        ArpPattern::UpDown => "Up-Down",
        ArpPattern::Random => "Random",
    }
}

pub(crate) fn arp_octave_span(value: f32) -> usize {
    (value.round() as i32).clamp(ARP_OCTAVES_MIN as i32, ARP_OCTAVES_MAX as i32) as usize
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
        ArpPattern::Up => ((pos + 1) % len, dir),
        ArpPattern::Down => ((pos + len - 1) % len, dir),
        ArpPattern::UpDown => arp_ping_pong_advance(pos, dir, len),
        ArpPattern::Random => (rng.gen_range(0..len), dir),
    }
}

pub(crate) struct ArpEngine {
    pub(crate) sample_rate: f32,
    pub(crate) chord_trigger: GridTrigger,
    pub(crate) step_index: usize,
    pub(crate) note_trigger: GridTrigger,
    pub(crate) cycle_pos: usize,
    pub(crate) ping_pong_dir: i32,
    pub(crate) voices: Vec<PianoTonalVoice>,
    pub(crate) rng: StdRng,
}

impl ArpEngine {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            chord_trigger: GridTrigger::after_start(),
            step_index: 0,
            note_trigger: GridTrigger::new(),
            cycle_pos: 0,
            ping_pong_dir: 1,
            voices: Vec::with_capacity(8),
            rng: StdRng::from_entropy(),
        }
    }

    pub(crate) fn next(
        &mut self,
        c: &ArpControls,
        pad: &PadControls,
        tune: f32,
        timing: TimingContext,
    ) -> (f32, f32) {
        // Follow the pad's current chord: an independent trigger synced to
        // the same chord-length grid as the pad/bass engines, so this
        // engine's step_index always matches the pad's, without reaching
        // into the pad engine directly. `pad_chord_tones` is the same
        // chord-source path Pad and Bass resolve through, so a custom
        // progression drives all three identically.
        let chord_count = pad_chord_count(pad);
        if self.step_index >= chord_count {
            self.step_index = 0;
        }
        if self.chord_trigger.pop(timing, pad.chord_bars * 4.0, 0.0) {
            self.step_index = (self.step_index + 1) % chord_count;
        }

        let rate_beats = c.rate_beats.clamp(ARP_RATE_BEATS_MIN, ARP_RATE_BEATS_MAX);
        if self.note_trigger.pop(timing, rate_beats, c.offset_beats) {
            let chord = pad_chord_tones(pad, self.step_index);
            let octaves = arp_octave_span(c.octaves);
            let notes = arp_cycle_notes(chord, octaves);
            let len = notes.len().max(1);
            // Chord/octave changes never reset the cycle position — just
            // clamp it into the (possibly resized) list so there's no click.
            self.cycle_pos = self.cycle_pos.min(len - 1);

            let pattern = arp_pattern_from_control(c.pattern);
            let note = notes[self.cycle_pos];

            let (next_pos, next_dir) =
                arp_advance(self.cycle_pos, pattern, len, self.ping_pong_dir, &mut self.rng);
            self.cycle_pos = next_pos;
            self.ping_pong_dir = next_dir;

            let hz = midi_to_hz(note) * tune_ratio(tune);
            let decay_samples = timing.beats_to_samples(rate_beats);
            let pan = self.rng.gen_range(-0.4f32..0.4);
            // A voice captures its gain at trigger time, so a gain of exactly
            // 0 (the default) would stay silent for its whole life — skip
            // creating it. Every RNG draw above still happens, keeping seeded
            // renders byte-identical.
            if c.gain != 0.0 {
                self.voices.push(PianoTonalVoice::new(
                    TONAL_PIANO_PROFILES[ARP_PROFILE_INDEX],
                    note,
                    hz,
                    pan,
                    c.gain,
                    decay_samples,
                    self.sample_rate,
                    c.attack,
                    c.release,
                ));
            }
        }

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for voice in &mut self.voices {
            let (l, r) = voice.next();
            dry_l += l;
            dry_r += r;
        }
        self.voices.retain(|voice| !voice.is_done());

        (dry_l, dry_r)
    }
}
