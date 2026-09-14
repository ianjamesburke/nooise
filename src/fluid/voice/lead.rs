//! The Lead voice: a mono, gliding solo synth that plays the Pad's current
//! chord tones, or the progression's scale, from a step lane on its own grid.

use super::*;

pub(crate) const LEAD_RATE_BEATS_MIN: f32 = 0.125;
pub(crate) const LEAD_RATE_BEATS_MAX: f32 = 4.0;
pub(crate) const LEAD_OCTAVE_MIN: f32 = -2.0;
pub(crate) const LEAD_OCTAVE_MAX: f32 = 2.0;
/// Tones a step or a played key can name, past the rest: nine, one per
/// letter of the play row.
pub(crate) const LEAD_TONE_COUNT: usize = 9;

/// The most notes a reach can hold: a full chromatic scale.
const LEAD_REACH_MAX: usize = 12;

/// What the Lead's nine tones index into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeadFollow {
    /// The four tones of the chord sounding now; tones move as chords move.
    Chord,
    /// The progression's own scale from its tonic; tones hold still across
    /// chord changes, so a line can pass through non-chord notes.
    Scale,
}

pub(crate) const LEAD_FOLLOWS: [LeadFollow; 2] = [LeadFollow::Chord, LeadFollow::Scale];

/// The lane's transport. One control, three states, so a Steps LFO can gate
/// the pattern in bars and play mode reaches Record without leaving the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeadPattern {
    /// The lane is silent; only played keys sound. The solo position.
    Off,
    /// The lane plays its steps.
    Play,
    /// The lane plays, and each played key is written into the step nearest
    /// the moment it was pressed, so a line builds up pass by pass.
    Record,
}

pub(crate) const LEAD_PATTERNS: [LeadPattern; 3] =
    [LeadPattern::Off, LeadPattern::Play, LeadPattern::Record];

impl LeadPattern {
    pub(crate) fn from_value(value: f32) -> Self {
        LEAD_PATTERNS[(value.round() as i64).clamp(0, LEAD_PATTERNS.len() as i64 - 1) as usize]
    }

    /// The control value naming this state: its `LEAD_PATTERNS` index.
    pub(crate) const fn value(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Play => 1.0,
            Self::Record => 2.0,
        }
    }

    /// The state one press of the play-mode key reaches: Off, Play, Record,
    /// and around.
    pub(crate) fn next(self) -> Self {
        LEAD_PATTERNS[(self.value() as usize + 1) % LEAD_PATTERNS.len()]
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Play => "Play",
            Self::Record => "Record",
        }
    }
}

impl LeadFollow {
    pub(crate) fn from_value(value: f32) -> Self {
        LEAD_FOLLOWS[(value.round() as i64).clamp(0, LEAD_FOLLOWS.len() as i64 - 1) as usize]
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Chord => "Chord",
            Self::Scale => "Scale",
        }
    }
}

/// The ascending notes tone 1 upward walks through; tones past `len` wrap
/// an octave up, so a four-note reach reads 1 2 3 4 1' 2' 3' 4' 1''.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LeadReach {
    notes: [i32; LEAD_REACH_MAX],
    len: usize,
}

impl LeadReach {
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// The MIDI note of 1-based `tone`, lifted by the layer's `lead.octave`.
    pub(crate) fn note(&self, tone: usize, octave: f32) -> i32 {
        let index = tone.saturating_sub(1);
        self.notes[index % self.len] + 12 * ((index / self.len) as i32 + octave.round() as i32)
    }

    pub(crate) fn label(&self, tone: usize) -> String {
        lead_tone_label(tone, self.len)
    }
}

/// How 1-based `tone` reads against a reach of `reach_len` notes: its
/// degree, with one prime per octave wrapped.
pub(crate) fn lead_tone_label(tone: usize, reach_len: usize) -> String {
    let index = tone.saturating_sub(1);
    let len = reach_len.max(1);
    format!("{}{}", index % len + 1, "'".repeat(index / len))
}

/// The reach the Lead resolves through right now, for both the lane and
/// the play row.
pub(crate) fn lead_reach(
    follow: LeadFollow,
    pad: &PadControls,
    progression: usize,
    step: usize,
) -> LeadReach {
    match follow {
        LeadFollow::Chord => {
            let chord = pad_chord_tones(pad, progression, step);
            let mut notes = [0; LEAD_REACH_MAX];
            notes[..chord.len()].copy_from_slice(&chord);
            LeadReach {
                notes,
                len: chord.len(),
            }
        }
        LeadFollow::Scale => progression_scale(pad, progression),
    }
}

/// A progression's scale: every pitch class its chords touch, laid out
/// ascending from the tonic (its first chord's lowest note). Derived rather
/// than declared, so a custom progression and each built-in table each
/// yield their own scale, and a chord change never moves a key.
pub(crate) fn progression_scale(pad: &PadControls, progression: usize) -> LeadReach {
    let tonic = pad_chord_tones(pad, progression, 0)[0];
    let mut present = [false; 12];
    for step in 0..pad_chord_count(pad) {
        for note in pad_chord_tones(pad, progression, step) {
            present[(note - tonic).rem_euclid(12) as usize] = true;
        }
    }
    let mut notes = [0; LEAD_REACH_MAX];
    let mut len = 0;
    for (interval, _) in present.iter().enumerate().filter(|(_, hit)| **hit) {
        notes[len] = tonic + interval as i32;
        len += 1;
    }
    LeadReach { notes, len }
}

/// A step value (or a play-row key) as a 1-based tone; `None` is the rest
/// at position 0. Out-of-table values clamp to the last tone.
pub(crate) fn lead_step_tone(value: f32) -> Option<usize> {
    match (value.round() as i64).clamp(0, LEAD_TONE_COUNT as i64) as usize {
        0 => None,
        tone => Some(tone),
    }
}

/// How a step value reads on the page, against the reach the song is in.
pub(crate) fn lead_step_label(value: f32, reach: &LeadReach) -> String {
    lead_step_tone(value).map_or_else(|| "Rest".to_string(), |tone| reach.label(tone))
}

/// The reach a page reads its labels against: the lane's tones are named
/// by degree, which depends only on the reach's size, so the chord step
/// need not be known to label the page.
pub(crate) fn lead_page_reach(lead: &LeadControls, pad: &PadControls) -> LeadReach {
    lead_reach(
        LeadFollow::from_value(lead.follow),
        pad,
        progression_index(pad.progression),
        0,
    )
}

/// How many of the lane's steps play: `lead.steps` rounded and clamped.
pub(crate) fn lead_live_step_count(step_count: f32) -> usize {
    (step_count.round() as i64).clamp(1, LEAD_STEP_COUNT as i64) as usize
}

/// The lane step a trigger on `beat` plays. Derived from the transport rather
/// than counted, exactly like the LFO staircase, so a saved song resumes on
/// the step the clock says and there is no position to persist. The quarter
/// step of slack absorbs a swung hit (never later than half a step) and the
/// grid's own epsilon.
pub(crate) fn lead_step_at(beat: f64, rate_beats: f32, offset_beats: f32, count: usize) -> usize {
    let position = (beat - offset_beats as f64) / rate_beats as f64 + 0.25;
    (position.floor() as i64).rem_euclid(count.max(1) as i64) as usize
}

/// The lane step a key pressed on `beat` records into: the nearest one, so a
/// tap slightly ahead of a step lands on it rather than the step before.
pub(crate) fn lead_record_step(
    beat: f64,
    rate_beats: f32,
    offset_beats: f32,
    count: usize,
) -> usize {
    let position = (beat - offset_beats as f64) / rate_beats as f64;
    (position.round() as i64).rem_euclid(count.max(1) as i64) as usize
}

/// Harmonics in a lead recipe's additive stack.
pub(crate) const LEAD_HARMONIC_COUNT: usize = 8;
/// Harmonics above this fraction of Nyquist are left out so high notes do not
/// alias.
const LEAD_NYQUIST_FRACTION: f32 = 0.45;

/// One lead character: an additive stack of the first eight harmonics at
/// these gains (signed, so a triangle's alternating partials are expressible),
/// plus an output trim. Every recipe glides and envelopes identically; a
/// type is only a spectrum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LeadRecipe {
    pub(crate) label: &'static str,
    pub(crate) harmonics: [f32; LEAD_HARMONIC_COUNT],
    /// Balances this stack's summed energy against the other types so
    /// `lead_types_render_at_a_matched_level` holds; retune here, never in
    /// the harmonics.
    pub(crate) trim: f32,
}

/// Lead characters in stored order; `lead.type` wraps into this table.
/// Trims are set so every type lands at the same RMS as `LEAD_LEVEL_TARGET`
/// relative to Bass — a bright lead sounds louder than it measures, so the
/// target sits below the other pitched voices rather than beside them.
pub(crate) const LEAD_TYPES: [LeadRecipe; 5] = [
    LeadRecipe {
        label: "Saw",
        harmonics: [1.0, 0.5, 0.333, 0.25, 0.2, 0.167, 0.143, 0.125],
        trim: 0.165,
    },
    LeadRecipe {
        label: "Square",
        harmonics: [1.0, 0.0, 0.333, 0.0, 0.2, 0.0, 0.143, 0.0],
        trim: 0.188,
    },
    LeadRecipe {
        label: "Soft",
        harmonics: [1.0, 0.0, -0.111, 0.0, 0.04, 0.0, -0.02, 0.0],
        trim: 0.2025,
    },
    LeadRecipe {
        label: "Pure",
        harmonics: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        trim: 0.204,
    },
    LeadRecipe {
        label: "Reed",
        harmonics: [1.0, 0.7, 0.9, 0.4, 0.6, 0.2, 0.3, 0.1],
        trim: 0.1185,
    },
];

pub(crate) fn lead_recipe(value: f32) -> LeadRecipe {
    LEAD_TYPES[wrapped_index(value, LEAD_TYPES.len())]
}

pub(crate) fn lead_type_label(value: f32) -> &'static str {
    lead_recipe(value).label
}

/// The one sounding note: a variable-pitch additive stack with an
/// attack/decay life. Pitch glides toward `target_hz` at a one-pole rate so a
/// new note legato-slides from the last instead of jumping. The recipe is
/// read per sample, so a Type edit reshapes the note already sounding.
pub(crate) struct LeadVoice {
    phase: f32,
    hz: f32,
    target_hz: f32,
    envelope: Adsr,
    sample_rate: f32,
}

/// A note's amplitude life: ramp in over `attack`, then either fall to
/// silence over `decay` (a lane step, or a key the terminal cannot hold) or
/// sustain at the peak while `hold` and fall over `decay` once released.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LeadShape {
    pub(crate) attack: f32,
    pub(crate) decay: f32,
    pub(crate) hold: bool,
}

impl LeadShape {
    fn envelope(self, sample_rate: f32) -> Adsr {
        let sustain = if self.hold { 1.0 } else { 0.0 };
        Adsr::new(self.attack, self.decay, sustain, self.decay, sample_rate)
    }
}

impl LeadVoice {
    pub(crate) fn new(hz: f32, shape: LeadShape, sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            hz,
            target_hz: hz,
            envelope: shape.envelope(sample_rate),
            sample_rate,
        }
    }

    /// Retarget the pitch and restart the envelope from its current level,
    /// without touching the oscillator phase: pitch, amplitude, and waveform
    /// are all continuous across the retrigger, so fast playing cannot click.
    pub(crate) fn retrigger(&mut self, hz: f32, shape: LeadShape) {
        self.target_hz = hz;
        let mut envelope = shape.envelope(self.sample_rate);
        envelope.start_at(self.envelope.level());
        self.envelope = envelope;
    }

    /// End a held note's sustain; a decaying note is unaffected.
    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }

    pub(crate) fn next(&mut self, glide: f32, recipe: &LeadRecipe) -> f32 {
        self.hz += (self.target_hz - self.hz) * glide_coefficient(glide, self.sample_rate);
        let mut raw = 0.0f32;
        let ceiling = self.sample_rate * LEAD_NYQUIST_FRACTION;
        for (index, gain) in recipe.harmonics.iter().enumerate() {
            let n = (index + 1) as f32;
            if self.hz * n > ceiling {
                break;
            }
            if *gain != 0.0 {
                raw += (self.phase * n).sin() * gain;
            }
        }
        self.phase += std::f32::consts::TAU * self.hz / self.sample_rate;
        if self.phase >= std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }
        raw * recipe.trim * self.envelope.next()
    }

    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }

    #[cfg(test)]
    pub(crate) fn hz(&self) -> f32 {
        self.hz
    }
}

/// Per-sample fraction of the remaining pitch distance to close: a one-pole
/// slew whose time constant is a third of `glide` seconds, so the slide is
/// audibly settled by the time `glide` has elapsed. Zero glide jumps.
pub(crate) fn glide_coefficient(glide: f32, sample_rate: f32) -> f32 {
    if glide <= 0.0 {
        return 1.0;
    }
    1.0 - (-3.0 / (glide * sample_rate)).exp()
}

/// The one hands-on note path from the UI to the audio thread: every press in
/// Lead play mode bumps `presses` and names its tone, and the engine plays a
/// tone each time it sees the count move. A count rather than a flag, so two
/// presses between engine polls still read as a press. Not song state: a
/// played note is a gesture, not a setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LeadPlayState {
    pub(crate) presses: u64,
    /// The 1-based tone of the last press.
    pub(crate) tone: usize,
    /// Whether that press's key is still down: a held note sustains, and
    /// the flag dropping is its release.
    pub(crate) held: bool,
}

pub(crate) struct LeadEngine {
    pub(crate) sample_rate: f32,
    pub(crate) progression: ProgressionFollower,
    pub(crate) step_trigger: GridTrigger,
    pub(crate) voice: Option<LeadVoice>,
    /// The last `LeadPlayState::presses` acted on; a snapshot with a higher
    /// count queues one played tone for the next sample.
    presses_seen: u64,
    /// Whether the last press seen is still held; the flag dropping without
    /// a new press queues a release.
    held_seen: bool,
    pending_press: Option<(usize, bool)>,
    pending_release: bool,
}

impl LeadEngine {
    #[cfg(test)]
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self::with_play_state(sample_rate, LeadPlayState::default())
    }

    /// Start already caught up with `play`, so a session that was loaded
    /// mid-gesture does not replay its last press on the first sample.
    pub(crate) fn with_play_state(sample_rate: f32, play: LeadPlayState) -> Self {
        Self {
            sample_rate,
            progression: ProgressionFollower::new(),
            step_trigger: GridTrigger::new(),
            voice: None,
            presses_seen: play.presses,
            held_seen: play.held,
            pending_press: None,
            pending_release: false,
        }
    }

    /// Adopt the latest published play state: a moved press count queues its
    /// tone for the next sample; a held flag that dropped queues a release.
    pub(crate) fn observe(&mut self, play: LeadPlayState) {
        if play.presses != self.presses_seen {
            self.presses_seen = play.presses;
            self.pending_press = Some((play.tone, play.held));
        } else if self.held_seen && !play.held {
            self.pending_release = true;
        }
        self.held_seen = play.held;
    }

    /// Start or slide to `note` now. A note that is still sounding is
    /// retriggered in place so the pitch glides; a finished one starts fresh
    /// at pitch. A held note sustains until `release`.
    pub(crate) fn play(&mut self, note: i32, tune: f32, c: &LeadControls, hold: bool) {
        let hz = note_hz(note, tune);
        let shape = LeadShape {
            attack: c.attack,
            decay: c.decay,
            hold,
        };
        match &mut self.voice {
            Some(voice) if !voice.is_done() => voice.retrigger(hz, shape),
            _ => self.voice = Some(LeadVoice::new(hz, shape, self.sample_rate)),
        }
    }

    /// Let a held note go: it decays from here.
    pub(crate) fn release(&mut self) {
        if let Some(voice) = &mut self.voice {
            voice.release();
        }
    }

    pub(crate) fn next(
        &mut self,
        c: &LeadControls,
        pad: &PadControls,
        tune: f32,
        timing: TimingContext,
    ) -> (f32, f32) {
        // The same chord-source path Pad, Bass, and Arp resolve through.
        let (progression, step) = self.progression.follow(pad, timing);

        let rate_beats = c.rate_beats.clamp(LEAD_RATE_BEATS_MIN, LEAD_RATE_BEATS_MAX);
        if self
            .step_trigger
            .pop_swung(timing, rate_beats, c.offset_beats, c.swing)
        {
            let count = lead_live_step_count(c.step_count);
            let lane_step = lead_step_at(timing.beat, rate_beats, c.offset_beats, count);
            // A silent layer plays nothing, so a Level of exactly 0 (the
            // default) never keeps a voice alive. A rest lets the current
            // note finish its decay untouched.
            // The lane yields to a held key: the player owns the voice
            // until the key comes up. An Off pattern is the solo position:
            // the trigger still ticks so Play resumes on the clock's step.
            if c.level != 0.0
                && LeadPattern::from_value(c.pattern) != LeadPattern::Off
                && !self.held_seen
                && let Some(tone) = lead_step_tone(c.steps[lane_step])
            {
                let reach = lead_reach(LeadFollow::from_value(c.follow), pad, progression, step);
                self.play(reach.note(tone, c.octave), tune, c, false);
            }
        }
        // A played key sounds the moment the engine sees it, on top of (and
        // sliding from) whatever the lane is doing.
        if let Some((pressed, hold)) = self.pending_press.take()
            && c.level != 0.0
            && let Some(tone) = lead_step_tone(pressed as f32)
        {
            let reach = lead_reach(LeadFollow::from_value(c.follow), pad, progression, step);
            self.play(reach.note(tone, c.octave), tune, c, hold);
        }
        if std::mem::take(&mut self.pending_release) {
            self.release();
        }

        let Some(voice) = &mut self.voice else {
            return (0.0, 0.0);
        };
        let sample = voice.next(c.glide, &lead_recipe(c.voice_type));
        if voice.is_done() {
            self.voice = None;
        }
        // Applied to the output, not captured per note, so the fader reaches
        // the note already sounding. Pre-smoothed by `GainSmoothers`.
        let out = sample * c.level;
        (out, out)
    }
}
