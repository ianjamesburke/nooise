//! The Lead voice: a mono, gliding solo synth that plays the Pad's current
//! chord tones, or the progression's scale, from a step lane on its own grid.

use super::*;
use crate::fluid::osc::{OscVoiceEvent, OscVoiceSink, tuned_note};

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

impl LeadFollow {
    pub(crate) const fn value(self) -> f32 {
        self as u8 as f32
    }
}

/// The lane's pattern and transport selector. `Off` silences the lane so only
/// played keys sound (solo mode); `Steady`, `Call`, `Climb`, and `Groove` seed
/// built-in musical contours; `Custom` plays user-authored or captured steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeadPattern {
    /// The lane is silent; only played keys sound. The solo position.
    Off,
    /// Stepwise arch over chord/scale tones (`DEFAULT_LEAD_STEPS`).
    Steady,
    /// Call-and-response phrase with a tied sustain and rests.
    Call,
    /// Two-octave rising and falling arch.
    Climb,
    /// 16-step syncopated melodic groove with ties and rests.
    Groove,
    /// User-edited, evolved, or captured step sequence.
    Custom,
}

pub(crate) const LEAD_PATTERNS: [LeadPattern; 6] = [
    LeadPattern::Off,
    LeadPattern::Steady,
    LeadPattern::Call,
    LeadPattern::Climb,
    LeadPattern::Groove,
    LeadPattern::Custom,
];

impl LeadPattern {
    /// Alias for the default playing preset (`Steady`, index `1.0`).
    #[cfg(test)]
    #[allow(non_upper_case_globals)]
    pub(crate) const Play: Self = Self::Steady;

    pub(crate) fn from_value(value: f32) -> Self {
        LEAD_PATTERNS[(value.round() as i64).clamp(0, LEAD_PATTERNS.len() as i64 - 1) as usize]
    }

    /// The control value naming this state: its `LEAD_PATTERNS` index.
    pub(crate) const fn value(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Steady => 1.0,
            Self::Call => 2.0,
            Self::Climb => 3.0,
            Self::Groove => 4.0,
            Self::Custom => 5.0,
        }
    }

    pub(crate) fn is_playing(self) -> bool {
        self != Self::Off
    }

    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Off => Self::Custom,
            Self::Steady | Self::Call | Self::Climb | Self::Groove | Self::Custom => Self::Off,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Steady => "Steady",
            Self::Call => "Call",
            Self::Climb => "Climb",
            Self::Groove => "Groove",
            Self::Custom => "Custom",
        }
    }
}

pub(crate) fn lead_authored_steps(c: &LeadControls) -> [f32; LEAD_STEP_COUNT] {
    let index = (c.pattern.round() as isize).clamp(0, CUSTOM_LEAD_PATTERN_INDEX as isize) as usize;
    if (2..=LEAD_PATTERN_PRESET_COUNT).contains(&index) {
        LEAD_PATTERN_PRESETS[index - 1].1
    } else {
        c.steps
    }
}

/// One play-mode press as the phrase buffer remembers it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LeadPress {
    pub(crate) beat: f64,
    pub(crate) tone: usize,
}

/// The rolling memory of what was played, so a phrase can be kept after the
/// fact rather than armed before it. Live-only: it regenerates from playing
/// and never enters a song code. Bounded, so it costs nothing to leave on.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LeadPhraseBuffer {
    presses: Vec<LeadPress>,
}

/// Presses the buffer keeps: enough for a 16-step lane of chords' worth of
/// legato at the fastest rate.
const LEAD_PHRASE_PRESSES: usize = 128;

impl LeadPhraseBuffer {
    pub(crate) fn push(&mut self, press: LeadPress) {
        if self.presses.len() == LEAD_PHRASE_PRESSES {
            self.presses.remove(0);
        }
        self.presses.push(press);
    }

    pub(crate) fn presses(&self) -> &[LeadPress] {
        &self.presses
    }
}

/// A phrase lifted off the buffer, ready to become the lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LeadCapture {
    pub(crate) count: usize,
    pub(crate) steps: [f32; LEAD_STEP_COUNT],
}

/// Lane lengths a capture may pick, shortest that fits the phrase.
const LEAD_CAPTURE_LENGTHS: [usize; 3] = [4, 8, 16];
/// A silence this long (in beats: one bar) ends a phrase; anything played
/// before it is not part of what "keep that" means.
const LEAD_PHRASE_GAP_BEATS: f64 = 4.0;

/// The phrase just played, as a lane: the presses since the last bar of
/// silence, each snapped to its nearest step, laid onto the shortest of
/// 4/8/16 steps that holds them. Steps keep their place against the bar
/// (`lane step = absolute step mod length`), so the loop plays back where it
/// was played rather than shifted to start on step 1. Presses older than the
/// lane's longest possible loop fall off; `None` when nothing was played.
pub(crate) fn lead_capture(
    presses: &[LeadPress],
    now: f64,
    rate_beats: f32,
    offset_beats: f32,
) -> Option<LeadCapture> {
    let rate = f64::from(rate_beats.clamp(LEAD_RATE_BEATS_MIN, LEAD_RATE_BEATS_MAX));
    let horizon = now - rate * LEAD_STEP_COUNT as f64;
    let recent: Vec<LeadPress> = presses
        .iter()
        .copied()
        .filter(|press| press.beat >= horizon && press.beat <= now)
        .collect();
    // Walk back from the newest press to the last bar of silence.
    let start = (1..recent.len())
        .rev()
        .find(|&i| recent[i].beat - recent[i - 1].beat >= LEAD_PHRASE_GAP_BEATS)
        .unwrap_or(0);
    let phrase = &recent[start..];
    let step_of =
        |press: &LeadPress| ((press.beat - f64::from(offset_beats)) / rate).round() as i64;
    let first = step_of(phrase.first()?);
    let last = step_of(phrase.last()?);
    let span = (last - first + 1).max(1) as usize;
    let count = LEAD_CAPTURE_LENGTHS
        .into_iter()
        .find(|&length| length >= span)
        .unwrap_or(LEAD_STEP_COUNT);
    let mut steps = [0.0; LEAD_STEP_COUNT];
    for press in phrase {
        let step = step_of(press).rem_euclid(count as i64) as usize;
        steps[step] = press.tone as f32;
    }
    Some(LeadCapture { count, steps })
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
    slot: usize,
) -> LeadReach {
    match follow {
        LeadFollow::Chord => {
            let chord = pad_chord_tones(pad, progression, slot);
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

/// Seven home-key notes, unchanged by chord-window edits or borrowed chords.
pub(crate) fn progression_scale(_pad: &PadControls, progression: usize) -> LeadReach {
    let (tonic, mode, _) = progression_home(progression);
    let mut notes = [0; LEAD_REACH_MAX];
    for (index, interval) in mode.intervals().into_iter().enumerate() {
        notes[index] = tonic + interval;
    }
    LeadReach { notes, len: 7 }
}

pub(crate) const LEAD_STEP_TIE: f32 = -1.0;
pub(crate) const LEAD_STEP_REST: f32 = 0.0;

pub(crate) fn lead_step_is_tie(value: f32) -> bool {
    (value.round() as i64) < LEAD_STEP_REST as i64
}

/// A step value (or a play-row key) as a 1-based tone; `None` is a rest (0)
/// or tie (-1). Out-of-table values clamp to the valid range.
pub(crate) fn lead_step_tone(value: f32) -> Option<usize> {
    let rounded = (value.round() as i64).clamp(-1, LEAD_TONE_COUNT as i64);
    if rounded <= 0 {
        None
    } else {
        Some(rounded as usize)
    }
}

/// How a step value reads on the page, against the reach the song is in.
pub(crate) fn lead_step_label(value: f32, reach: &LeadReach) -> String {
    if lead_step_is_tie(value) {
        "Tie".to_string()
    } else {
        lead_step_tone(value).map_or_else(|| "Rest".to_string(), |tone| reach.label(tone))
    }
}

/// The reach a page reads its labels against: the lane's tones are named
/// by degree, which depends only on the reach's size, so the chord playing
/// need not be known to label the page.
pub(crate) fn lead_page_reach(lead: &LeadControls, pad: &PadControls) -> LeadReach {
    let window = ChordWindow::requested(pad);
    lead_reach(
        LeadFollow::from_value(lead.follow),
        pad,
        window.progression,
        window.slot(0),
    )
}

pub(crate) fn tonal_page_reach(tonal: &TonalControls, pad: &PadControls) -> LeadReach {
    let window = ChordWindow::requested(pad);
    lead_reach(
        LeadFollow::from_value(tonal.follow),
        pad,
        window.progression,
        window.slot(0),
    )
}

/// How many of the lane's steps play: `lead.steps` rounded and clamped.
pub(crate) fn lead_live_step_count(step_count: f32) -> usize {
    (step_count.round() as i64).clamp(1, LEAD_STEP_COUNT as i64) as usize
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
    lane_holding: bool,
    osc: OscVoiceSink,
    osc_suppressed: bool,
    osc_active_note: Option<u8>,
    osc_held: bool,
    osc_gate_remaining_beats: Option<f64>,
    input_note: Option<u8>,
    transport: Transport,
    pub(crate) last_cycle: Option<u64>,
    pub(crate) active_steps_source: [f32; LEAD_STEP_COUNT],
    pub(crate) evolved_steps: [f32; LEAD_STEP_COUNT],
    pub(crate) evolution_seed: u64,
    pub(crate) evolution_count: u64,
    pub(crate) session_state: Option<LiveSession>,
}

impl LeadEngine {
    #[cfg(test)]
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self::with_play_state(sample_rate, LeadPlayState::default())
    }

    /// Start already caught up with `play`, so a session that was loaded
    /// mid-gesture does not replay its last press on the first sample.
    #[cfg(test)]
    pub(crate) fn with_play_state(sample_rate: f32, play: LeadPlayState) -> Self {
        Self::new_with_live_state(sample_rate, play, None)
    }

    pub(crate) fn new_with_live_state(
        sample_rate: f32,
        play: LeadPlayState,
        session_state: Option<LiveSession>,
    ) -> Self {
        let mut rng = StdRng::from_entropy();
        let evolution_seed = rng.r#gen();
        Self {
            sample_rate,
            progression: ProgressionFollower::new(),
            step_trigger: GridTrigger::new(),
            voice: None,
            presses_seen: play.presses,
            held_seen: play.held,
            pending_press: None,
            pending_release: false,
            lane_holding: false,
            osc: OscVoiceSink::default(),
            osc_suppressed: false,
            osc_active_note: None,
            osc_held: false,
            osc_gate_remaining_beats: None,
            input_note: None,
            transport: Transport::Playing,
            last_cycle: None,
            active_steps_source: DEFAULT_LEAD_STEPS,
            evolved_steps: DEFAULT_LEAD_STEPS,
            evolution_seed,
            evolution_count: 0,
            session_state,
        }
    }

    fn evolve_steps(&mut self, live_len: usize, evolve_rate: f32, cycle: u64) {
        let live_len = live_len.clamp(1, LEAD_STEP_COUNT);
        let max_tone = LEAD_TONE_COUNT as i32;
        let mutations = ((evolve_rate.clamp(0.0, 1.0) * 3.0).ceil() as usize).max(1);
        let seed = self
            .evolution_seed
            .wrapping_add(
                self.evolution_count
                    .wrapping_add(1)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15),
            )
            .wrapping_add(cycle.rotate_left(17));
        let mut evo_rng = StdRng::seed_from_u64(seed);
        for _ in 0..mutations {
            let idx = evo_rng.gen_range(0..live_len);
            let current = (self.evolved_steps[idx].round() as i32).clamp(-1, max_tone);
            let next = if current <= 0 {
                evo_rng.gen_range(1..=max_tone)
            } else if idx > 0 && evo_rng.gen_bool(0.10) {
                -1
            } else if evo_rng.gen_bool(0.12) {
                0
            } else {
                let delta = if evo_rng.gen_bool(0.5) { 1 } else { -1 };
                (current + delta).clamp(1, max_tone)
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
                snapshot.controls.lead.steps = steps;
                snapshot.controls.lead.pattern = CUSTOM_LEAD_PATTERN_INDEX as f32;
            });
        }
    }

    pub(crate) fn set_osc(&mut self, sink: OscVoiceSink) {
        self.osc = sink;
    }

    pub(crate) fn set_osc_suppressed(&mut self, suppressed: bool) {
        if suppressed {
            self.release_osc();
        }
        self.osc_suppressed = suppressed;
    }

    /// Inbound OSC notes play the Lead's sound at the received pitch without
    /// echoing back to outbound OSC.
    pub(crate) fn osc_note_on(&mut self, note: u8, c: &LeadControls, tune: f32) {
        self.input_note = Some(note);
        self.lane_holding = false;
        self.play(i32::from(note), tune, c, true);
    }

    pub(crate) fn osc_note_off(&mut self, note: u8) {
        if self.input_note == Some(note) {
            self.input_note = None;
            self.release();
        }
    }

    pub(crate) fn release_input(&mut self) {
        if self.input_note.take().is_some() {
            self.release();
        }
    }

    fn release_osc(&mut self) {
        if let Some(note) = self.osc_active_note.take() {
            self.osc.send(OscVoiceEvent::LeadOff(note));
            self.osc_held = false;
            self.osc_gate_remaining_beats = None;
        }
    }

    fn play_osc(&mut self, note: i32, tune: f32, gate_beats: f32, hold: bool) {
        if !self.osc_suppressed && self.osc.is_attached() {
            let tuned = tuned_note(note, tune);
            self.osc.send(OscVoiceEvent::LeadNote {
                note: tuned,
                gate_beats,
            });
            self.osc_active_note = Some(tuned);
            self.osc_held = hold;
            self.osc_gate_remaining_beats = (!hold).then_some(f64::from(gate_beats));
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
        key: MasterKey,
        timing: TimingContext,
    ) -> (f32, f32) {
        // The same chord-source path Pad, Bass, and Arp resolve through.
        let (progression, slot) = self.progression.follow(pad, timing);
        let tune = key.shift(progression) as f32;
        let osc_enabled = !self.osc_suppressed && self.osc.is_attached();
        if !osc_enabled || timing.transport != self.transport {
            self.release_osc();
        }
        if timing.transport != Transport::Playing && self.lane_holding {
            self.lane_holding = false;
            self.release();
        }
        self.transport = timing.transport;
        if let Some(remaining) = &mut self.osc_gate_remaining_beats {
            *remaining -= timing.bpm / (60.0 * timing.sample_rate);
            if *remaining <= 0.0 {
                self.release_osc();
            }
        }
        let released = std::mem::take(&mut self.pending_release);
        if released && self.osc_held {
            self.release_osc();
        }

        let rate_beats = c.rate_beats.clamp(LEAD_RATE_BEATS_MIN, LEAD_RATE_BEATS_MAX);
        let gate_beats = rate_beats.min(0.5);
        if self
            .step_trigger
            .pop_swung(timing, rate_beats, c.offset_beats, c.swing)
        {
            let authored = lead_authored_steps(c);
            let is_custom = (c.pattern.round() as isize)
                .clamp(0, CUSTOM_LEAD_PATTERN_INDEX as isize) as usize
                >= CUSTOM_LEAD_PATTERN_INDEX;
            if is_custom && c.steps == self.evolved_steps {
                self.active_steps_source = c.steps;
            } else if authored != self.active_steps_source {
                self.active_steps_source = authored;
                self.evolved_steps = authored;
                self.last_cycle = None;
            }
            let count = lead_live_step_count(c.step_count);
            let cycle_beats = (rate_beats * count as f32).max(LEAD_RATE_BEATS_MIN);
            let cycle = (timing.beat.max(0.0) / f64::from(cycle_beats)).floor() as u64;
            if self.last_cycle.is_some_and(|last| cycle > last)
                && c.evolve_rate > 0.0
                && LeadPattern::from_value(c.pattern).is_playing()
            {
                self.evolve_steps(count, c.evolve_rate, cycle);
            }
            self.last_cycle = Some(cycle);

            let lane_step = lane_step_at(timing.beat, rate_beats, c.offset_beats, count);
            // A silent layer plays nothing, so a Level of exactly 0 (the
            // default) never keeps a voice alive. A rest lets the current
            // note finish its decay untouched.
            // The lane yields to a held key: the player owns the voice
            // until the key comes up. An Off pattern is the solo position:
            // the trigger still ticks so Play resumes on the clock's step.
            if (c.level != 0.0 || osc_enabled)
                && LeadPattern::from_value(c.pattern).is_playing()
                && !self.held_seen
                && self.input_note.is_none()
            {
                let step_val = self.evolved_steps[lane_step];
                if let Some(tone) = lead_step_tone(step_val) {
                    let mut tied_steps = 0usize;
                    while tied_steps + 1 < count
                        && lead_step_is_tie(
                            self.evolved_steps[(lane_step + 1 + tied_steps) % count],
                        )
                    {
                        tied_steps += 1;
                    }
                    let hold_for_tie = tied_steps > 0;
                    self.lane_holding = hold_for_tie;
                    let reach =
                        lead_reach(LeadFollow::from_value(c.follow), pad, progression, slot);
                    let note = reach.note(tone, c.octave);
                    if c.level != 0.0 {
                        self.play(note, tune, c, hold_for_tie);
                    }
                    if osc_enabled {
                        let osc_gate = if tied_steps > 0 {
                            rate_beats * (1 + tied_steps) as f32
                        } else {
                            gate_beats
                        };
                        self.play_osc(note, tune, osc_gate, false);
                    }
                } else if lead_step_is_tie(step_val) {
                    let next_is_tie =
                        count > 1 && lead_step_is_tie(self.evolved_steps[(lane_step + 1) % count]);
                    if !next_is_tie && self.lane_holding {
                        self.lane_holding = false;
                        self.release();
                    }
                } else if self.lane_holding {
                    self.lane_holding = false;
                    self.release();
                }
            } else if self.lane_holding {
                self.lane_holding = false;
                self.release();
            }
        }
        // A played key sounds the moment the engine sees it, on top of (and
        // sliding from) whatever the lane is doing.
        if let Some((pressed, hold)) = self.pending_press.take()
            && (c.level != 0.0 || osc_enabled)
            && let Some(tone) = lead_step_tone(pressed as f32)
        {
            self.lane_holding = false;
            let reach = lead_reach(LeadFollow::from_value(c.follow), pad, progression, slot);
            let note = reach.note(tone, c.octave);
            if c.level != 0.0 {
                self.play(note, tune, c, hold);
            }
            if osc_enabled {
                self.play_osc(note, tune, gate_beats, hold);
            }
        }
        if released && self.input_note.is_none() {
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
        // the note already sounding. Pre-smoothed by `ControlSmoothers`.
        let out = sample * c.level;
        (out, out)
    }
}
