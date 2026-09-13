//! The Lead voice: a mono, gliding solo synth that plays the Pad's current
//! chord tones from a step lane on its own grid.

use super::*;

pub(crate) const LEAD_RATE_BEATS_MIN: f32 = 0.125;
pub(crate) const LEAD_RATE_BEATS_MAX: f32 = 4.0;
pub(crate) const LEAD_OCTAVE_MIN: f32 = -2.0;
pub(crate) const LEAD_OCTAVE_MAX: f32 = 2.0;
/// Chord tones a step or a played key can reach: `pad_chord_tones` voices
/// four, so the reach is those four, the same four an octave up, and the
/// root two octaves up.
pub(crate) const LEAD_CHORD_TONES: usize = 4;

/// What one step value (or one performance key) plays. Index 0 is a rest;
/// the rest name a chord tone (0-based into the pad's four voiced tones) and
/// an octave lift above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LeadTone {
    pub(crate) label: &'static str,
    pub(crate) chord_tone: usize,
    pub(crate) octave: i32,
}

/// Step values in stored order; `lead.stepN` and the performance keys both
/// index it. Position 0 is the rest, so a fresh lane silences by stepping
/// left.
pub(crate) const LEAD_STEP_TONES: [Option<LeadTone>; 10] = [
    None,
    Some(LeadTone {
        label: "1",
        chord_tone: 0,
        octave: 0,
    }),
    Some(LeadTone {
        label: "2",
        chord_tone: 1,
        octave: 0,
    }),
    Some(LeadTone {
        label: "3",
        chord_tone: 2,
        octave: 0,
    }),
    Some(LeadTone {
        label: "4",
        chord_tone: 3,
        octave: 0,
    }),
    Some(LeadTone {
        label: "1'",
        chord_tone: 0,
        octave: 1,
    }),
    Some(LeadTone {
        label: "2'",
        chord_tone: 1,
        octave: 1,
    }),
    Some(LeadTone {
        label: "3'",
        chord_tone: 2,
        octave: 1,
    }),
    Some(LeadTone {
        label: "4'",
        chord_tone: 3,
        octave: 1,
    }),
    Some(LeadTone {
        label: "1''",
        chord_tone: 0,
        octave: 2,
    }),
];

pub(crate) fn lead_step_tone(value: f32) -> Option<LeadTone> {
    LEAD_STEP_TONES[(value.round() as i64).clamp(0, LEAD_STEP_TONES.len() as i64 - 1) as usize]
}

pub(crate) fn lead_step_label(value: f32) -> &'static str {
    lead_step_tone(value).map_or("Rest", |tone| tone.label)
}

/// How many of the lane's steps play: `lead.steps` rounded and clamped.
pub(crate) fn lead_live_step_count(step_count: f32) -> usize {
    (step_count.round() as i64).clamp(1, LEAD_STEP_COUNT as i64) as usize
}

/// The MIDI note a tone resolves to against the chord sounding now, lifted
/// by the tone's own octave and the layer's `lead.octave`.
pub(crate) fn lead_note(tone: LeadTone, chord: [i32; LEAD_CHORD_TONES], octave: f32) -> i32 {
    chord[tone.chord_tone] + 12 * (tone.octave + octave.round() as i32)
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

/// Number of harmonics in the lead's additive saw. Steeper than a true saw's
/// `1/n` would be bright enough already; the Drive module in the layer's
/// factory chain adds the edge.
const LEAD_HARMONIC_COUNT: usize = 8;
/// Output trim balancing the summed stack against the other pitched voices.
const LEAD_OUTPUT_GAIN: f32 = 0.45;
/// Harmonics above this fraction of Nyquist are left out so high notes do not
/// alias.
const LEAD_NYQUIST_FRACTION: f32 = 0.45;

/// The one sounding note: a variable-pitch additive saw with an attack/decay
/// life. Pitch glides toward `target_hz` at a one-pole rate so a new note
/// legato-slides from the last instead of jumping.
pub(crate) struct LeadVoice {
    phase: f32,
    hz: f32,
    target_hz: f32,
    envelope: Adsr,
    sample_rate: f32,
}

impl LeadVoice {
    pub(crate) fn new(hz: f32, attack: f32, decay: f32, sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            hz,
            target_hz: hz,
            envelope: Adsr::new(attack, decay, 0.0, decay, sample_rate),
            sample_rate,
        }
    }

    /// Retarget the pitch and restart the envelope without touching the
    /// oscillator phase, so the slide is continuous and click-free.
    pub(crate) fn retrigger(&mut self, hz: f32, attack: f32, decay: f32) {
        self.target_hz = hz;
        self.envelope = Adsr::new(attack, decay, 0.0, decay, self.sample_rate);
    }

    pub(crate) fn next(&mut self, glide: f32) -> f32 {
        self.hz += (self.target_hz - self.hz) * glide_coefficient(glide, self.sample_rate);
        let mut raw = 0.0f32;
        let ceiling = self.sample_rate * LEAD_NYQUIST_FRACTION;
        for harmonic in 1..=LEAD_HARMONIC_COUNT {
            let n = harmonic as f32;
            if self.hz * n > ceiling {
                break;
            }
            raw += (self.phase * n).sin() / n;
        }
        self.phase += std::f32::consts::TAU * self.hz / self.sample_rate;
        if self.phase >= std::f32::consts::TAU {
            self.phase -= std::f32::consts::TAU;
        }
        raw * LEAD_OUTPUT_GAIN * self.envelope.next()
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
    /// Index into `LEAD_STEP_TONES` of the last press.
    pub(crate) tone: usize,
}

pub(crate) struct LeadEngine {
    pub(crate) sample_rate: f32,
    pub(crate) progression: ProgressionFollower,
    pub(crate) step_trigger: GridTrigger,
    pub(crate) voice: Option<LeadVoice>,
    /// The last `LeadPlayState::presses` acted on; a snapshot with a higher
    /// count queues one played tone for the next sample.
    presses_seen: u64,
    pending_press: Option<usize>,
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
            pending_press: None,
        }
    }

    /// Adopt the latest published play state: a moved press count queues its
    /// tone for the next sample.
    pub(crate) fn observe(&mut self, play: LeadPlayState) {
        if play.presses != self.presses_seen {
            self.presses_seen = play.presses;
            self.pending_press = Some(play.tone);
        }
    }

    /// Start or slide to `note` now. A note that is still sounding is
    /// retriggered in place so the pitch glides; a finished one starts fresh
    /// at pitch.
    pub(crate) fn play(&mut self, note: i32, tune: f32, c: &LeadControls) {
        let hz = note_hz(note, tune);
        match &mut self.voice {
            Some(voice) if !voice.is_done() => voice.retrigger(hz, c.attack, c.decay),
            _ => self.voice = Some(LeadVoice::new(hz, c.attack, c.decay, self.sample_rate)),
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
            if c.level != 0.0
                && let Some(tone) = lead_step_tone(c.steps[lane_step])
            {
                let chord = pad_chord_tones(pad, progression, step);
                self.play(lead_note(tone, chord, c.octave), tune, c);
            }
        }
        // A played key sounds the moment the engine sees it, on top of (and
        // sliding from) whatever the lane is doing.
        if let Some(pressed) = self.pending_press.take()
            && c.level != 0.0
            && let Some(tone) = lead_step_tone(pressed as f32)
        {
            let chord = pad_chord_tones(pad, progression, step);
            self.play(lead_note(tone, chord, c.octave), tune, c);
        }

        let Some(voice) = &mut self.voice else {
            return (0.0, 0.0);
        };
        let sample = voice.next(c.glide);
        if voice.is_done() {
            self.voice = None;
        }
        // Applied to the output, not captured per note, so the fader reaches
        // the note already sounding. Pre-smoothed by `GainSmoothers`.
        let out = sample * c.level;
        (out, out)
    }
}
