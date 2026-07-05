use super::*;

/// Bass line for each progression, authored independently of the Pad's
/// chord voicings (one MIDI note per step, same 8-step indexing as
/// PROGRESSIONS). B/C/D currently mirror their chord's lowest tone; A
/// diverges deliberately (step 3 walks to G2 instead of following the
/// B-chord's root) to give the bass its own melodic movement.
pub(crate) const BASS_LINES: [[i32; 8]; 4] = [
    [45, 47, 45, 43, 52, 53, 45, 45], // A
    [45, 50, 48, 43, 41, 52, 45, 43], // B
    [45, 41, 48, 43, 50, 52, 47, 43], // C
    [45, 41, 48, 43, 50, 52, 47, 43], // D
];

pub(crate) fn bass_root_note(progression: usize, step: usize) -> i32 {
    BASS_LINES[progression % BASS_LINES.len()][step % 8]
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

pub(crate) const MAX_BASS_VOICES: usize = 3;

/// Fixed duration of one rhythm-pattern step (a 16th note). Step timing never
/// changes; `interval_beats` instead crops how many steps of the 16-step
/// phrase play before looping back to step 0 (or extends the loop with
/// trailing silence, for a "gap" feel).
pub(crate) const BASS_STEP_BEATS: f32 = 0.25;

pub(crate) struct BassEngine {
    pub(crate) sample_rate: f32,
    pub(crate) chord_trigger: GridTrigger,
    pub(crate) step_index: usize,
    pub(crate) step_trigger: GridTrigger,
    pub(crate) rhythm_step: usize,
    pub(crate) voices: Vec<BassVoice>,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl BassEngine {
    pub(crate) fn new(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
        Self {
            sample_rate,
            chord_trigger: GridTrigger::after_start(),
            step_index: 0,
            step_trigger: GridTrigger::new(),
            rhythm_step: BASS_RHYTHMS[0].len() - 1,
            voices: Vec::with_capacity(MAX_BASS_VOICES),
            telemetry,
        }
    }

    pub(crate) fn next(
        &mut self,
        c: &BassControls,
        pad: &PadControls,
        tune: f32,
        timing: TimingContext,
    ) -> (f32, f32) {
        let progression = (pad.progression.round() as i64).rem_euclid(4) as usize;
        if self.chord_trigger.pop(timing, pad.chord_bars * 4.0, 0.0) {
            self.step_index = (self.step_index + 1) % 8;
        }

        let loop_len = (c.interval_beats / BASS_STEP_BEATS)
            .round()
            .clamp(1.0, 32.0) as usize;
        if self
            .step_trigger
            .pop(timing, BASS_STEP_BEATS, c.offset_beats)
        {
            self.rhythm_step = (self.rhythm_step + 1) % loop_len;
            let rhythm = (c.rhythm.round() as usize) % BASS_RHYTHMS.len();
            let hit = self.rhythm_step < BASS_RHYTHMS[rhythm].len()
                && BASS_RHYTHMS[rhythm][self.rhythm_step];
            if hit {
                let note =
                    bass_root_note(progression, self.step_index) + (c.octave.round() as i32) * 12;
                let hz = midi_to_hz(note) * tune_ratio(tune);
                self.telemetry.publish_bass_note(hz);
                self.telemetry.bass_pulse.fetch_add(1, Ordering::Relaxed);
                for voice in &mut self.voices {
                    voice.release();
                }
                if self.voices.len() >= MAX_BASS_VOICES {
                    let remove_count = self.voices.len() + 1 - MAX_BASS_VOICES;
                    self.voices.drain(0..remove_count);
                }
                self.voices.push(BassVoice::new(
                    hz,
                    c.attack_time,
                    c.decay_time,
                    c.drive,
                    self.sample_rate,
                ));
            }
        }

        let mut l = 0.0f32;
        let mut r = 0.0f32;
        for voice in &mut self.voices {
            let (vl, vr) = voice.next();
            l += vl;
            r += vr;
        }
        self.voices.retain(|v| !v.is_done());

        (l * c.level, r * c.level)
    }
}

pub(crate) struct BassVoice {
    pub(crate) osc: SineOscillator,
    pub(crate) envelope: Adsr,
    pub(crate) drive: f32,
}

impl BassVoice {
    pub(crate) fn new(
        hz: f32,
        attack_time: f32,
        decay_time: f32,
        drive: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            osc: SineOscillator::new(hz, sample_rate),
            // No sustain — Decay carries the note fully to silence, like the
            // Perc voice's percussive envelope. Decay also doubles as the
            // release curve, smoothing the cutoff if a hit retriggers before
            // the previous note has fully decayed.
            envelope: Adsr::new(attack_time, decay_time, 0.0, decay_time, sample_rate),
            drive,
        }
    }

    pub(crate) fn next(&mut self) -> (f32, f32) {
        let mut s = self.osc.next() * self.envelope.next();
        if self.drive > 0.0 {
            let driven = s * (1.0 + self.drive * 8.0);
            s = driven / (1.0 + driven.abs()) * (1.0 + self.drive * 0.5);
        }
        StereoPanner::equal_power(s, 0.0)
    }

    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }

    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}
