//! The Bass voice: a monophonic line following the Pad's progression, in
//! six characters over one shared trigger and pitch path.

use super::*;
use crate::fluid::osc::{OscVoiceEvent, OscVoiceSink, tuned_note};
use crate::fx::drive::DriveType;
use crate::synth::filter::SvfLowpass;
use crate::synth::oscillator::PolyBlepSaw;

/// Bass follows the same active progression slot as Pad/Arp. For Custom it
/// reads that slot's root directly. Built-ins retain their authored Bass
/// register/pedal and transpose only by an edited Root/Accidental delta.
pub(crate) fn bass_root_note(progression: usize, slot: usize, pad: &PadControls) -> i32 {
    let slot = slot % CHORD_SLOT_COUNT;
    match PROGRESSIONS.get(progression) {
        Some(built_in) => {
            let built_in_slot = slot % BUILTIN_PROGRESSION_CHORDS;
            let shift = pad_chord_root_note(pad.chord_slot(progression, slot))
                - pad_chord_root_note(&built_in.chords[built_in_slot].preset);
            built_in.bass[built_in_slot] + shift
        }
        None => pad_chord_root_note(&pad.chord_slots[slot]),
    }
}

// ============================================================
// Bass engine (follows the Pad's chord root on a rhythm pattern)
// ============================================================

/// Four 16-step rhythm patterns (one bar at 16th-note resolution: counted
/// "1 e & a 2 e & a 3 e & a 4 e & a"). A/B/C/D selects between them; `true`
/// re-articulates the bass note at that step.
pub(crate) const BASS_RHYTHMS: [[bool; 16]; 4] = [
    // A: quarter notes on the beat
    [
        true, false, false, false, true, false, false, false, true, false, false, false, true,
        false, false, false,
    ],
    // B: syncopated — pickup into 1, push before 3, quick pickups into 4
    [
        true, false, false, true, false, false, true, false, true, true, false, false, true, true,
        false, false,
    ],
    // C: straight eighths — steady walking-bass feel
    [
        true, false, true, false, true, false, true, false, true, false, true, false, true, false,
        true, false,
    ],
    // D: busy 16th groove
    [
        true, false, false, true, false, false, true, false, true, false, false, true, false,
        false, true, false,
    ],
];

pub(crate) fn bass_live_step_count(step_count: f32) -> usize {
    (step_count.round() as i64).clamp(1, BASS_STEP_COUNT as i64) as usize
}

pub(crate) fn bass_pattern_label(value: f32) -> &'static str {
    BASS_PATTERNS[wrapped_index(value, BASS_PATTERNS.len())]
}

pub(crate) fn bass_authored_steps(c: &BassControls) -> [f32; BASS_STEP_COUNT] {
    let rhythm = (c.rhythm.round() as isize).clamp(0, CUSTOM_BASS_RHYTHM_INDEX as isize) as usize;
    if rhythm < BASS_RHYTHM_PRESET_COUNT {
        bass_rhythm_steps(rhythm)
    } else {
        c.steps
    }
}

pub(crate) fn bass_page_reach(bass: &BassControls, pad: &PadControls) -> LeadReach {
    let window = ChordWindow::requested(pad);
    lead_reach(
        LeadFollow::from_value(bass.follow),
        pad,
        window.progression,
        window.slot(0),
    )
}

pub(crate) fn bass_step_note(
    tone: usize,
    follow: LeadFollow,
    progression: usize,
    slot: usize,
    pad: &PadControls,
    octave: f32,
) -> i32 {
    let octave_shift = (octave.round() as i32) * 12;
    match follow {
        LeadFollow::Chord => {
            let root = bass_root_note(progression, slot, pad);
            if tone <= 1 {
                root + octave_shift
            } else {
                let reach = lead_reach(LeadFollow::Chord, pad, progression, slot);
                let semitones_above_first = reach.note(tone, 0.0) - reach.note(1, 0.0);
                root + semitones_above_first + octave_shift
            }
        }
        LeadFollow::Scale => {
            let reach = progression_scale(pad, progression);
            reach.note(tone, octave) - 12
        }
    }
}

pub(crate) fn consecutive_ties_after(steps: &[f32], start_step: usize, live_len: usize) -> usize {
    let len = live_len.min(steps.len());
    if len <= 1 {
        return 0;
    }
    let mut ties = 0usize;
    while ties + 1 < len && lead_step_is_tie(steps[(start_step + 1 + ties) % len]) {
        ties += 1;
    }
    ties
}

/// Bass is monophonic: a rhythm-grid hit hard-cuts whatever is currently
/// sounding and starts the new note fresh, regardless of `decay_time`. The
/// replaced voice isn't dropped instantly (that clicks) — it's handed to
/// `fading_voice` and rung down over this fixed short window, independent of
/// the voice's own envelope. This fade-out is a click guard, not a second
/// voice: it is silent well before the next 16th-note step can land, so
/// consecutive bass hits never audibly overlap.
const BASS_MONO_FADE_SECONDS: f32 = 0.003;

pub(crate) struct BassEngine {
    pub(crate) sample_rate: f32,
    pub(crate) progression: ProgressionFollower,
    pub(crate) step_trigger: GridTrigger,
    pub(crate) voice: Option<BassVoice>,
    pub(crate) fading_voice: Option<BassVoice>,
    pub(crate) fade_samples_remaining: u32,
    pub(crate) fade_total_samples: u32,
    /// Bass voices are mono and always centered; the constant-power center
    /// gain pair is computed once here instead of per voice-sample.
    pub(crate) center_gains: (f32, f32),
    osc: Option<OscVoiceSink>,
    osc_suppressed: bool,
    /// The note this engine has sounding on the OSC output, and the beat
    /// its gate closes.
    osc_active: bool,
    osc_release_beat: f64,
    /// Whether inbound OSC bass notes are driving the Bass voice in place of
    /// its internal rhythm pattern.
    osc_input_active: bool,
    transport: Transport,
    pub(crate) last_cycle: Option<u64>,
    pub(crate) active_steps_source: [f32; BASS_STEP_COUNT],
    pub(crate) evolved_steps: [f32; BASS_STEP_COUNT],
    pub(crate) evolution_seed: u64,
    pub(crate) evolution_count: u64,
    pub(crate) session_state: Option<LiveSession>,
}

impl BassEngine {
    #[cfg(test)]
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self::new_with_live_state(sample_rate, None)
    }

    pub(crate) fn new_with_live_state(
        sample_rate: f32,
        session_state: Option<LiveSession>,
    ) -> Self {
        let mut rng = StdRng::from_entropy();
        let evolution_seed = rng.r#gen();
        Self {
            sample_rate,
            progression: ProgressionFollower::new(),
            step_trigger: GridTrigger::new(),
            voice: None,
            fading_voice: None,
            fade_samples_remaining: 0,
            fade_total_samples: (sample_rate * BASS_MONO_FADE_SECONDS).max(1.0) as u32,
            center_gains: StereoPanner::gains(0.0),
            osc: None,
            osc_suppressed: false,
            osc_active: false,
            osc_release_beat: 0.0,
            osc_input_active: false,
            transport: Transport::Stopped,
            last_cycle: None,
            active_steps_source: DEFAULT_BASS_STEPS,
            evolved_steps: DEFAULT_BASS_STEPS,
            evolution_seed,
            evolution_count: 0,
            session_state,
        }
    }

    fn evolve_steps(&mut self, live_len: usize, evolve_rate: f32, cycle: u64) {
        let live_len = live_len.clamp(1, BASS_STEP_COUNT);
        let max_tone = LEAD_TONE_COUNT as i32;
        let seed = self
            .evolution_seed
            .wrapping_add(
                self.evolution_count
                    .wrapping_add(1)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15),
            )
            .wrapping_add(cycle.rotate_left(17));
        let mutations = evolve_mutation_count(evolve_rate, 3, seed);
        if mutations == 0 {
            return;
        }
        let mut evo_rng = StdRng::seed_from_u64(seed);
        for _ in 0..mutations {
            let idx = evo_rng.gen_range(0..live_len);
            let current = (self.evolved_steps[idx].round() as i32).clamp(-1, max_tone);
            let next = if current <= 0 {
                if evo_rng.gen_bool(0.55) {
                    1
                } else {
                    evo_rng.gen_range(1..=max_tone)
                }
            } else if idx > 0 && evo_rng.gen_bool(0.12) {
                -1
            } else if evo_rng.gen_bool(0.20) {
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
                snapshot.controls.bass.steps = steps;
                snapshot.controls.bass.rhythm = CUSTOM_BASS_RHYTHM_INDEX as f32;
            });
        }
    }

    pub(crate) fn set_osc(&mut self, sink: OscVoiceSink) {
        self.osc = Some(sink);
    }

    pub(crate) fn set_osc_suppressed(&mut self, suppressed: bool) {
        if suppressed && !self.osc_suppressed {
            self.release_osc();
        }
        self.osc_suppressed = suppressed;
    }

    pub(crate) fn release_input(&mut self) {
        self.osc_input_active = false;
    }

    fn release_osc(&mut self) {
        if self.osc_active {
            if let Some(sink) = &self.osc {
                sink.send(OscVoiceEvent::BassOff);
            }
            self.osc_active = false;
        }
    }

    /// Start a note now, handing whatever was sounding to the click-guard
    /// fade-out slot.
    fn start_note(&mut self, hz: f32, c: &BassControls) {
        self.start_note_with_hold(hz, 0.0, c);
    }

    fn start_note_with_hold(&mut self, hz: f32, hold_seconds: f32, c: &BassControls) {
        self.fading_voice = self.voice.take();
        self.fade_samples_remaining = self.fade_total_samples;
        self.voice = Some(BassVoice::new_with_hold(
            wrapped_index(c.voice_type, BASS_TYPES.len()),
            hz,
            c.attack_time,
            c.decay_time,
            hold_seconds,
            c.punch,
            self.sample_rate,
        ));
    }

    /// Inbound OSC input plays the Bass voice at the received pitch in place of
    /// the internal rhythm pattern. It is never echoed to OSC output, and the
    /// voice decays on its own, so a Note Off needs no handling.
    pub(crate) fn osc_note_on(&mut self, note: u8, c: &BassControls, tune: f32) {
        self.osc_input_active = true;
        self.start_note(note_hz(i32::from(note), tune), c);
    }

    pub(crate) fn next(
        &mut self,
        c: &BassControls,
        pad: &PadControls,
        key: MasterKey,
        timing: TimingContext,
    ) -> (f32, f32) {
        let (progression, slot) = self.progression.follow(pad, timing);
        let tune = key.shift(progression) as f32;
        if self.osc_suppressed || timing.transport != self.transport {
            self.release_osc();
        }
        self.transport = timing.transport;
        if self.osc_active && timing.beat >= self.osc_release_beat {
            self.release_osc();
        }

        let rate_beats = c.interval_beats.clamp(0.125, 4.0);
        let live_steps = bass_live_step_count(c.step_count);
        if self
            .step_trigger
            .pop_swung(timing, rate_beats, c.offset_beats, c.swing)
        {
            let authored = bass_authored_steps(c);
            let is_custom = (c.rhythm.round() as usize) >= CUSTOM_BASS_RHYTHM_INDEX;
            if is_custom && c.steps == self.evolved_steps {
                self.active_steps_source = c.steps;
            } else if authored != self.active_steps_source {
                self.active_steps_source = authored;
                self.evolved_steps = authored;
                self.last_cycle = None;
            }
            let cycle_beats = (rate_beats * live_steps as f32).max(0.125);
            let cycle = (timing.beat.max(0.0) / f64::from(cycle_beats)).floor() as u64;
            if self.last_cycle.is_some_and(|last| cycle > last) && c.evolve_rate > 0.0 {
                self.evolve_steps(live_steps, c.evolve_rate, cycle);
            }
            self.last_cycle = Some(cycle);

            let rhythm_step = lane_step_at(timing.beat, rate_beats, c.offset_beats, live_steps);
            // When inbound OSC notes are driving Bass, played notes replace the rhythm pattern.
            if !self.osc_input_active
                && let Some(tone) = lead_step_tone(self.evolved_steps[rhythm_step])
            {
                let follow = LeadFollow::from_value(c.follow);
                let note = bass_step_note(tone, follow, progression, slot, pad, c.octave);
                let ties = consecutive_ties_after(&self.evolved_steps, rhythm_step, live_steps);
                let hold_beats = ties as f32 * rate_beats;
                let hold_seconds = if ties > 0 {
                    (f64::from(hold_beats) * 60.0 / timing.bpm.max(1.0)) as f32
                } else {
                    0.0
                };
                // Hard-cut: whatever was sounding hands off to the fade-out
                // slot (replacing any prior fade in progress) and the new
                // note starts clean and immediately, not layered on top.
                self.start_note_with_hold(note_hz(note, tune), hold_seconds, c);
                if !self.osc_suppressed
                    && let Some(sink) = &self.osc
                {
                    let decay_beats =
                        hold_beats + (f64::from(c.decay_time) * timing.bpm / 60.0) as f32;
                    sink.send(OscVoiceEvent::BassNote {
                        note: tuned_note(note, tune),
                        decay_beats,
                    });
                    self.osc_active = true;
                    // The gate follows the audio life of the note.
                    self.osc_release_beat = timing.beat + f64::from(decay_beats);
                }
            }
        }

        let (gain_l, gain_r) = self.center_gains;
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        if let Some(voice) = &mut self.voice {
            let s = voice.next();
            l += s * gain_l;
            r += s * gain_r;
            if voice.is_done() {
                self.voice = None;
            }
        }
        if let Some(voice) = &mut self.fading_voice {
            let s = voice.next();
            let fade_gain = self.fade_samples_remaining as f32 / self.fade_total_samples as f32;
            l += s * gain_l * fade_gain;
            r += s * gain_r * fade_gain;
            self.fade_samples_remaining = self.fade_samples_remaining.saturating_sub(1);
            if self.fade_samples_remaining == 0 {
                self.fading_voice = None;
            }
        }

        (l * c.level, r * c.level)
    }
}

/// `bass.type` selects the voice character used for every new bass note.
/// Index 0 (`Sub`) is the legacy voice, unchanged and the default; switching
/// type never touches the shared trigger/rhythm, pitch, or reseed paths in
/// `BassEngine::next` above. FM, Fold and Acid each sweep their brightness
/// with the note, so the onset carries mid and high content and the tail
/// settles toward the fundamental.
pub(crate) enum BassVoice {
    Sub(SubBassVoice),
    Saw(SawBassVoice),
    Pluck(PluckBassVoice),
    Fm(FmBassVoice),
    Fold(FoldBassVoice),
    Acid(AcidBassVoice),
}

impl BassVoice {
    #[cfg(test)]
    pub(crate) fn new(
        voice_type: usize,
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        punch: f32,
        sample_rate: f32,
    ) -> Self {
        Self::new_with_hold(
            voice_type,
            hz,
            attack_time,
            decay_time,
            0.0,
            punch,
            sample_rate,
        )
    }

    pub(crate) fn new_with_hold(
        voice_type: usize,
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        hold_seconds: f32,
        punch: f32,
        sample_rate: f32,
    ) -> Self {
        let mut voice = match voice_type {
            0 => Self::Sub(SubBassVoice::new(hz, attack_time, decay_time, sample_rate)),
            1 => Self::Saw(SawBassVoice::new(hz, attack_time, decay_time, sample_rate)),
            2 => Self::Pluck(PluckBassVoice::new(
                hz,
                attack_time,
                decay_time,
                sample_rate,
            )),
            3 => Self::Fm(FmBassVoice::new(
                hz,
                attack_time,
                decay_time,
                punch,
                sample_rate,
            )),
            4 => Self::Fold(FoldBassVoice::new(
                hz,
                attack_time,
                decay_time,
                punch,
                sample_rate,
            )),
            _ => Self::Acid(AcidBassVoice::new(
                hz,
                attack_time,
                decay_time,
                punch,
                sample_rate,
            )),
        };
        if hold_seconds > 0.0 {
            voice
                .core_mut()
                .envelope
                .set_hold(hold_seconds, sample_rate);
        }
        voice.core_mut().sweep = PunchSweep::new(punch, sample_rate);
        voice
    }

    fn core_mut(&mut self) -> &mut BassVoiceCore {
        match self {
            Self::Sub(voice) => &mut voice.core,
            Self::Saw(voice) => &mut voice.core,
            Self::Pluck(voice) => &mut voice.core,
            Self::Fm(voice) => &mut voice.core,
            Self::Fold(voice) => &mut voice.core,
            Self::Acid(voice) => &mut voice.core,
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        match self {
            Self::Sub(voice) => voice.next(),
            Self::Saw(voice) => voice.next(),
            Self::Pluck(voice) => voice.next(),
            Self::Fm(voice) => voice.next(),
            Self::Fold(voice) => voice.next(),
            Self::Acid(voice) => voice.next(),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Self::Sub(voice) => voice.is_done(),
            Self::Saw(voice) => voice.is_done(),
            Self::Pluck(voice) => voice.is_done(),
            Self::Fm(voice) => voice.is_done(),
            Self::Fold(voice) => voice.is_done(),
            Self::Acid(voice) => voice.is_done(),
        }
    }
}

/// At 100% Punch a note starts this many semitones above its pitch...
const PUNCH_MAX_SEMITONES: f32 = 12.0;
/// ...and falls back with this time constant: short enough to read as the
/// note's thump rather than a slide.
const PUNCH_SWEEP_SECONDS: f32 = 0.012;

/// Punch's onset pitch drop, shared by every character. Absent at Punch 0,
/// so a voice without Punch keeps its unscaled oscillator path bit-for-bit.
pub(crate) struct PunchSweep {
    semitones: f32,
    decay: f32,
}

impl PunchSweep {
    fn new(punch: f32, sample_rate: f32) -> Option<Self> {
        (punch > 0.0).then(|| Self {
            semitones: punch.min(1.0) * PUNCH_MAX_SEMITONES,
            decay: (-1.0 / (PUNCH_SWEEP_SECONDS * sample_rate)).exp(),
        })
    }

    fn next_ratio(&mut self) -> f32 {
        let ratio = (self.semitones / 12.0).exp2();
        self.semitones *= self.decay;
        ratio
    }
}

/// Shared envelope construction behind every `bass.type` voice: an
/// attack/decay-only ADSR (no sustain — Decay carries the note fully to
/// silence, like the Perc voice's percussive envelope; Decay also doubles as
/// the release curve, smoothing the cutoff if a hit retriggers before the
/// previous note has fully decayed). Drive belongs to the post-synthesis
/// module chain so every layer uses the same processor.
pub(crate) struct BassVoiceCore {
    pub(crate) envelope: Adsr,
    pub(crate) sweep: Option<PunchSweep>,
}

impl BassVoiceCore {
    pub(crate) fn new(attack_time: f32, decay_time: f32, sample_rate: f32) -> Self {
        Self {
            envelope: Adsr::new(attack_time, decay_time, 0.0, decay_time, sample_rate),
            sweep: None,
        }
    }

    /// This sample's Punch pitch ratio, or `None` without Punch.
    fn pitch_ratio(&mut self) -> Option<f32> {
        self.sweep.as_mut().map(PunchSweep::next_ratio)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

/// One sine sample under an optional Punch ratio. Without one it is the
/// plain `next`, which keeps Punch-free notes bit-for-bit.
fn swept(osc: &mut SineOscillator, ratio: Option<f32>) -> f32 {
    match ratio {
        Some(ratio) => osc.next_scaled(ratio),
        None => osc.next(),
    }
}

/// A per-note brightness envelope: starts at `start` and falls toward
/// `floor` with time constant `seconds`, captured at note start like the
/// amplitude envelope.
struct Brightness {
    excess: f32,
    floor: f32,
    decay: f32,
}

impl Brightness {
    fn new(start: f32, floor: f32, seconds: f32, sample_rate: f32) -> Self {
        Self {
            excess: start - floor,
            floor,
            decay: (-1.0 / (seconds * sample_rate).max(1.0)).exp(),
        }
    }

    fn next(&mut self) -> f32 {
        let value = self.floor + self.excess;
        self.excess *= self.decay;
        value
    }
}

/// How long a character's brightness takes to settle, as a share of the
/// note's Decay, bounded so very short or long notes keep an audible sweep.
fn brightness_seconds(decay_time: f32, share: f32, min: f32, max: f32) -> f32 {
    (decay_time * share).clamp(min, max)
}

/// Type 0 (default): a single sine oscillator through the shared
/// attack/decay envelope. Optional Drive is applied later by the layer's
/// shared post-synthesis module chain.
pub(crate) struct SubBassVoice {
    pub(crate) osc: SineOscillator,
    pub(crate) core: BassVoiceCore,
}

impl SubBassVoice {
    pub(crate) fn new(hz: f32, attack_time: f32, decay_time: f32, sample_rate: f32) -> Self {
        Self {
            osc: SineOscillator::new(hz, sample_rate),
            core: BassVoiceCore::new(attack_time, decay_time, sample_rate),
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        let ratio = self.core.pitch_ratio();
        swept(&mut self.osc, ratio) * self.core.envelope.next()
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// Number of stacked harmonics in the Saw voice's additive stack (fundamental
/// plus three overtones).
const BASS_SAW_HARMONIC_COUNT: usize = 4;
/// Per-harmonic amplitude weights, steeper than a true sawtooth's `1/n`
/// falloff so the extra brightness stays bass-register appropriate (tamed
/// highs) instead of turning harsh. Chosen (together with
/// `BASS_SAW_OUTPUT_GAIN`) so the voice sits at the same perceived level as
/// the Sub and Pluck voices.
const BASS_SAW_HARMONIC_GAINS: [f32; BASS_SAW_HARMONIC_COUNT] = [1.0, 0.46, 0.24, 0.14];
/// Output trim balancing the additive stack's summed energy against the
/// single-oscillator Sub/Pluck voices.
const BASS_SAW_OUTPUT_GAIN: f32 = 0.62;

/// Type 1: a brighter, saw-leaning character. An additive stack of sine
/// harmonics (not a literal bandlimited sawtooth oscillator — none exists in
/// `synth::oscillator` — but a steeper-than-`1/n` weighted approximation)
/// gives it more harmonic content than the Sub voice while a fast tilt keeps
/// the top end tame. Shares the Sub voice's envelope shape.
pub(crate) struct SawBassVoice {
    pub(crate) oscillators: [SineOscillator; BASS_SAW_HARMONIC_COUNT],
    pub(crate) core: BassVoiceCore,
}

impl SawBassVoice {
    pub(crate) fn new(hz: f32, attack_time: f32, decay_time: f32, sample_rate: f32) -> Self {
        Self {
            oscillators: std::array::from_fn(|index| {
                SineOscillator::new(hz * (index + 1) as f32, sample_rate)
            }),
            core: BassVoiceCore::new(attack_time, decay_time, sample_rate),
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        let ratio = self.core.pitch_ratio();
        let mut raw = 0.0f32;
        for (osc, gain) in self.oscillators.iter_mut().zip(BASS_SAW_HARMONIC_GAINS) {
            raw += swept(osc, ratio) * gain;
        }
        raw * BASS_SAW_OUTPUT_GAIN * self.core.envelope.next()
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// Fraction of the control-supplied attack/decay used by the Pluck voice's
/// body envelope — shorter than Sub/Saw so it decays faster while still
/// tracking the shared `bass.attack_time`/`bass.decay_time` controls.
const BASS_PLUCK_ENVELOPE_SCALE: f32 = 0.4;
/// Fixed decay time (seconds) of the Pluck voice's attack transient (an
/// octave-up click that fires once per note, independent of the shared decay
/// control).
const BASS_PLUCK_TRANSIENT_DECAY_SECONDS: f32 = 0.02;
/// Mix level of the transient click relative to the sustained body.
const BASS_PLUCK_TRANSIENT_MIX: f32 = 0.5;
/// Output trim balancing the Pluck voice's body + transient against the Sub
/// voice's single-oscillator level.
const BASS_PLUCK_OUTPUT_GAIN: f32 = 0.82;

/// Type 2: a plucked/shorter character. Faster attack/decay than Sub (scaled
/// via `BASS_PLUCK_ENVELOPE_SCALE`) plus a short octave-up transient at the
/// onset of the note for a percussive pluck attack.
pub(crate) struct PluckBassVoice {
    pub(crate) osc: SineOscillator,
    pub(crate) transient_osc: SineOscillator,
    pub(crate) core: BassVoiceCore,
    pub(crate) transient_samples_remaining: u64,
    pub(crate) transient_total_samples: f32,
}

impl PluckBassVoice {
    pub(crate) fn new(hz: f32, attack_time: f32, decay_time: f32, sample_rate: f32) -> Self {
        let attack_time = (attack_time * BASS_PLUCK_ENVELOPE_SCALE).max(0.001);
        let decay_time = (decay_time * BASS_PLUCK_ENVELOPE_SCALE).max(0.001);
        let transient_total_samples = (BASS_PLUCK_TRANSIENT_DECAY_SECONDS * sample_rate).max(1.0);
        Self {
            osc: SineOscillator::new(hz, sample_rate),
            transient_osc: SineOscillator::new(hz * 2.0, sample_rate),
            core: BassVoiceCore::new(attack_time, decay_time, sample_rate),
            transient_samples_remaining: transient_total_samples as u64,
            transient_total_samples,
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        let ratio = self.core.pitch_ratio();
        let body = swept(&mut self.osc, ratio) * self.core.envelope.next();

        let transient = if self.transient_samples_remaining > 0 {
            let elapsed = self.transient_total_samples - self.transient_samples_remaining as f32;
            let gain = (-elapsed / self.transient_total_samples * 5.0).exp();
            self.transient_samples_remaining -= 1;
            swept(&mut self.transient_osc, ratio) * gain
        } else {
            0.0
        };

        (body + transient * BASS_PLUCK_TRANSIENT_MIX) * BASS_PLUCK_OUTPUT_GAIN
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// FM's onset modulation index, and how much 100% Punch multiplies it.
const BASS_FM_ONSET_INDEX: f32 = 16.0;
const BASS_FM_PUNCH_BOOST: f32 = 2.0;
/// A steady low-index pair under the decaying one keeps some odd-harmonic
/// body after the onset has collapsed toward the fundamental.
const BASS_FM_BODY_INDEX: f32 = 0.8;
const BASS_FM_OUTPUT_GAIN: f32 = 1.45;

/// Type 3: two-operator FM. A 1:1 pair whose index starts high and decays
/// gives a bright, saw-like attack that collapses to a sine; a 1:2 pair at a
/// steady low index keeps a hollow body under it.
pub(crate) struct FmBassVoice {
    pub(crate) stack: FmStack,
    pub(crate) hz: f32,
    pub(crate) core: BassVoiceCore,
}

impl FmBassVoice {
    pub(crate) fn new(
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        punch: f32,
        sample_rate: f32,
    ) -> Self {
        let onset_index = BASS_FM_ONSET_INDEX * (1.0 + punch * BASS_FM_PUNCH_BOOST);
        let index_seconds = brightness_seconds(decay_time, 0.2, 0.015, 0.15);
        Self {
            stack: FmStack::new(sample_rate)
                .with_pair(
                    FmPair::new(1.0, 1.0, onset_index)
                        .with_index_decay(index_seconds * sample_rate)
                        .with_level(0.5),
                )
                .with_pair(FmPair::new(2.0, 1.0, BASS_FM_BODY_INDEX).with_level(0.5)),
            hz,
            core: BassVoiceCore::new(attack_time, decay_time, sample_rate),
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        let hz = self.hz * self.core.pitch_ratio().unwrap_or(1.0);
        self.stack.next(hz) * BASS_FM_OUTPUT_GAIN * self.core.envelope.next()
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// Fold's onset drive into the wavefolder, and how much 100% Punch adds.
const BASS_FOLD_ONSET_GAIN: f32 = 7.0;
const BASS_FOLD_PUNCH_BOOST: f32 = 1.0;
const BASS_FOLD_OUTPUT_GAIN: f32 = 1.05;

/// Type 4: a sine driven into the shared Drive module's triangle wavefolder
/// (antialiased the same way), with the drive decaying back to unity: the
/// onset folds into a growl of upper harmonics and the tail is a clean sine,
/// because a unit sine never reaches a fold point.
pub(crate) struct FoldBassVoice {
    pub(crate) osc: SineOscillator,
    fold_gain: Brightness,
    previous: Option<f64>,
    pub(crate) core: BassVoiceCore,
}

impl FoldBassVoice {
    pub(crate) fn new(
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        punch: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            osc: SineOscillator::new(hz, sample_rate),
            fold_gain: Brightness::new(
                BASS_FOLD_ONSET_GAIN * (1.0 + punch * BASS_FOLD_PUNCH_BOOST),
                1.0,
                brightness_seconds(decay_time, 0.3, 0.02, 0.3),
                sample_rate,
            ),
            previous: None,
            core: BassVoiceCore::new(attack_time, decay_time, sample_rate),
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        let ratio = self.core.pitch_ratio();
        let sine = swept(&mut self.osc, ratio);
        let u = f64::from(sine * self.fold_gain.next());
        let previous = self.previous.replace(u).unwrap_or(u);
        DriveType::Fold.adaa(u, previous) as f32 * BASS_FOLD_OUTPUT_GAIN * self.core.envelope.next()
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// Acid's filter: cutoff as a multiple of the note, from `BASE` plus an
/// envelope that starts `DEPTH` higher (more with Punch) and decays.
const BASS_ACID_CUTOFF_BASE: f32 = 2.0;
const BASS_ACID_CUTOFF_DEPTH: f32 = 24.0;
const BASS_ACID_PUNCH_BOOST: f32 = 1.5;
/// 1/Q of the filter: low enough for the resonant squelch the sweep is for.
const BASS_ACID_RESONANCE: f32 = 0.5;
const BASS_ACID_OUTPUT_GAIN: f32 = 0.8;

/// Type 5: subtractive. A band-limited saw through a resonant lowpass whose
/// cutoff sweeps down with every note, the classic acid-line snap.
pub(crate) struct AcidBassVoice {
    saw: PolyBlepSaw,
    filter: SvfLowpass,
    cutoff: Brightness,
    hz: f32,
    sample_rate: f32,
    pub(crate) core: BassVoiceCore,
}

impl AcidBassVoice {
    pub(crate) fn new(
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        punch: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            saw: PolyBlepSaw::new(sample_rate),
            filter: SvfLowpass::default(),
            cutoff: Brightness::new(
                BASS_ACID_CUTOFF_BASE
                    + BASS_ACID_CUTOFF_DEPTH * (1.0 + punch * BASS_ACID_PUNCH_BOOST),
                BASS_ACID_CUTOFF_BASE,
                brightness_seconds(decay_time, 0.35, 0.03, 0.4),
                sample_rate,
            ),
            hz,
            sample_rate,
            core: BassVoiceCore::new(attack_time, decay_time, sample_rate),
        }
    }

    pub(crate) fn next(&mut self) -> f32 {
        let hz = self.hz * self.core.pitch_ratio().unwrap_or(1.0);
        let saw = self.saw.next(hz);
        let filtered = self.filter.next(
            saw,
            hz * self.cutoff.next(),
            BASS_ACID_RESONANCE,
            self.sample_rate,
        );
        filtered * BASS_ACID_OUTPUT_GAIN * self.core.envelope.next()
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}
