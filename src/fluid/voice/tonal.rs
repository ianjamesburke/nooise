use super::*;

// ============================================================
// Tonal engine (melodic steps with randomness)
// ============================================================

pub(crate) struct TonalEngine {
    pub(crate) sample_rate: f32,
    pub(crate) step_trigger: GridTrigger,
    pub(crate) step_index: usize,
    pub(crate) active_phrase: usize,
    pub(crate) last_cycle: Option<u64>,
    pub(crate) evolved_phrase: Vec<i32>,
    pub(crate) voices: Vec<TonalVoice>,
    pub(crate) low_cut_l: TonalLowCut,
    pub(crate) low_cut_r: TonalLowCut,
    pub(crate) rng: StdRng,
}

pub(crate) const TONAL_LOW_CUT_HZ: f32 = 70.0;
pub(crate) const TONAL_RATE_BEATS_MIN: f32 = 0.125;
pub(crate) const TONAL_RATE_BEATS_MAX: f32 = 4.0;
pub(crate) const TONAL_CYCLE_BEATS_MIN: f32 = TONAL_RATE_BEATS_MIN;
pub(crate) const TONAL_CYCLE_BEATS_MAX: f32 = 16.0;
pub(crate) const TONAL_MAX_LOOP_STEPS: usize = 64;
pub(crate) const TONAL_MAX_EVOLVE_NOTES: usize = 4;
pub(crate) const TONAL_SCALE_MIDI: [i32; 10] = [45, 48, 50, 52, 55, 57, 60, 62, 64, 67];
pub(crate) const TONAL_PIANO_HARMONIC_COUNT: usize = 16;
pub(crate) const TONAL_PIANO_PROFILE_COUNT: usize = 9;
pub(crate) const TONAL_PIANO_A_KEYFRAMES: [PianoKeyframe; 3] = [
    PianoKeyframe {
        midi: 36,
        decay_factor: 3.0,
        harmonics: [
            0.03703704, 0.07407408, 0.22222224, 0.15308644, 0.18024692, 0.21481483, 0.24691358,
            0.04938272, 0.03703704, 0.05679013, 0.11111112, 0.05432099, 0.22716051, 0.04938272,
            0.04938272, 0.03703704,
        ],
    },
    PianoKeyframe {
        midi: 48,
        decay_factor: 2.0,
        harmonics: [
            0.04368932,
            0.43689322,
            0.18786408,
            0.04368932,
            0.1485437,
            0.10048544,
            0.30582526,
            0.034951456,
            0.052427184,
            0.10048544,
            0.04805825,
            0.07427185,
            0.026213592,
            0.10048544,
            0.078640774,
            0.017475728,
        ],
    },
    PianoKeyframe {
        midi: 60,
        decay_factor: 1.0,
        harmonics: [
            0.57937425,
            0.17381229,
            0.06546929,
            0.052143686,
            0.024913093,
            0.052143686,
            0.035921205,
            0.0063731167,
            0.004634994,
            0.0011587485,
            0.0011587485,
            0.0011587485,
            0.00057937426,
            0.00028968713,
            0.00057937426,
            0.00028968713,
        ],
    },
];
pub(crate) const TONAL_MARIMBA_KEYFRAMES: [PianoKeyframe; 4] = [
    PianoKeyframe {
        midi: 36,
        decay_factor: 3.0,
        harmonics: [
            0.6043514,
            0.009669623,
            0.009669623,
            0.030217571,
            0.024174057,
            0.030217571,
            0.018130543,
            0.018130543,
            0.030217571,
            0.25987113,
            0.060435142,
            0.24174057,
            0.060435142,
            0.042304598,
            0.030217571,
            0.030217571,
        ],
    },
    PianoKeyframe {
        midi: 48,
        decay_factor: 2.0,
        harmonics: [
            0.38923132,
            0.011676939,
            0.038923133,
            0.17126179,
            0.02919235,
            0.015569253,
            0.011676939,
            0.0077846264,
            0.038923133,
            0.32695428,
            0.108984776,
            0.019461567,
            0.023353878,
            0.0031138507,
            0.002335388,
            0.0015569254,
        ],
    },
    PianoKeyframe {
        midi: 60,
        decay_factor: 1.0,
        harmonics: [
            0.6660065,
            0.0048003546,
            0.013320129,
            0.17499822,
            0.0066600647,
            0.0,
            0.002664026,
            0.00020980158,
            0.0005268287,
            0.13081412,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ],
    },
    PianoKeyframe {
        midi: 84,
        decay_factor: 1.0,
        harmonics: [
            0.77432626,
            0.0034792372,
            0.00040290528,
            0.0,
            0.018717118,
            0.0030744278,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ],
    },
];
// Electric-piano spectrum: fundamental-dominant with a soft "tine" bump
// around the 5th/6th partial and a fast upper rolloff. Warm, non-metallic —
// the reeds fade quickly so nothing rings out bright.
pub(crate) const TONAL_EP_KEYFRAMES: [PianoKeyframe; 3] = [
    PianoKeyframe {
        midi: 36,
        decay_factor: 2.6,
        harmonics: [
            0.6, 0.168, 0.072, 0.036, 0.084, 0.06, 0.024, 0.012, 0.009, 0.006, 0.0048, 0.003,
            0.0024, 0.0018, 0.0012, 0.0006,
        ],
    },
    PianoKeyframe {
        midi: 48,
        decay_factor: 1.8,
        harmonics: [
            0.6, 0.132, 0.054, 0.03, 0.06, 0.036, 0.018, 0.009, 0.006, 0.0036, 0.0024, 0.0018,
            0.0012, 0.0009, 0.0006, 0.00048,
        ],
    },
    PianoKeyframe {
        midi: 60,
        decay_factor: 1.0,
        harmonics: [
            0.6, 0.09, 0.036, 0.018, 0.03, 0.018, 0.009, 0.0048, 0.003, 0.0018, 0.0012, 0.0006,
            0.00048, 0.0003, 0.00024, 0.00018,
        ],
    },
];
// Types 1-9 (profile index = type - 1). Type 8 / profile[7] is Cloud Keys,
// the keeper, left exactly as tuned. The rest are warm, non-metallic keys
// voiced for ambient techno: soft attacks, unhurried decays, gentle tops.
pub(crate) const TONAL_PIANO_PROFILES: [PianoProfile; TONAL_PIANO_PROFILE_COUNT] = [
    // Rhodes: warm electric piano, quick bell-tine attack, mellow sustain.
    PianoProfile {
        keyframes: &TONAL_EP_KEYFRAMES,
        amplitude: 0.40,
        body_power: 0.35,
        harmonic_tilt: -0.70,
        decay_low: 0.70,
        decay_high: 4.2,
        decay_scale: 0.60,
    },
    // Wurli: reedier electric piano, a touch more bark and shorter tail.
    PianoProfile {
        keyframes: &TONAL_EP_KEYFRAMES,
        amplitude: 0.44,
        body_power: 0.45,
        harmonic_tilt: -0.85,
        decay_low: 0.50,
        decay_high: 3.6,
        decay_scale: 0.50,
    },
    // Felt: muted upright with the soft attack of a felt-covered hammer.
    PianoProfile {
        keyframes: &TONAL_PIANO_A_KEYFRAMES,
        amplitude: 0.36,
        body_power: 0.30,
        harmonic_tilt: -1.05,
        decay_low: 0.45,
        decay_high: 3.4,
        decay_scale: 0.50,
    },
    // Marimba: wooden mallet, percussive body, rounded overtones.
    PianoProfile {
        keyframes: &TONAL_MARIMBA_KEYFRAMES,
        amplitude: 0.30,
        body_power: 0.78,
        harmonic_tilt: -0.95,
        decay_low: 1.0,
        decay_high: 5.8,
        decay_scale: 0.62,
    },
    // Kalimba: plucked lamellophone, fundamental-heavy, gentle ring.
    PianoProfile {
        keyframes: &TONAL_EP_KEYFRAMES,
        amplitude: 0.34,
        body_power: 0.70,
        harmonic_tilt: -0.60,
        decay_low: 1.1,
        decay_high: 6.5,
        decay_scale: 0.50,
    },
    // Pluck: short, dry, staccato key stab for rhythmic ambient patterns.
    PianoProfile {
        keyframes: &TONAL_EP_KEYFRAMES,
        amplitude: 0.36,
        body_power: 0.85,
        harmonic_tilt: -0.70,
        decay_low: 1.2,
        decay_high: 6.0,
        decay_scale: 0.45,
    },
    // Dulcet: clear, sweet upright with a little more shimmer than Felt.
    PianoProfile {
        keyframes: &TONAL_PIANO_A_KEYFRAMES,
        amplitude: 0.38,
        body_power: 0.40,
        harmonic_tilt: -0.70,
        decay_low: 0.70,
        decay_high: 5.0,
        decay_scale: 0.68,
    },
    // Cloud Keys: the keeper. Airy, dark, slow-blooming pad piano.
    PianoProfile {
        keyframes: &TONAL_PIANO_A_KEYFRAMES,
        amplitude: 0.34,
        body_power: 0.18,
        harmonic_tilt: -1.20,
        decay_low: 0.28,
        decay_high: 2.4,
        decay_scale: 0.34,
    },
    // Haze: an even softer, slower sibling of Cloud — a breath of a key.
    PianoProfile {
        keyframes: &TONAL_PIANO_A_KEYFRAMES,
        amplitude: 0.32,
        body_power: 0.16,
        harmonic_tilt: -1.15,
        decay_low: 0.30,
        decay_high: 2.8,
        decay_scale: 0.38,
    },
];
pub(crate) const TONAL_PHRASES: [&[i32]; 8] = [
    &[45, 50, 55, 48, 52, 57, 50, 55],
    &[45, 52, 57, 60, 57, 52, 50, 48, 50, 55, 52, 45],
    &[57, 60, 64, 62, 60, 57, 52, 55],
    &[45, 48, 52, 55, 60, 57, 55, 52, 50, 52, 55, 48],
    &[52, 55, 60, 64, 67, 64, 60, 55],
    &[
        45, 50, 52, 55, 57, 55, 52, 50, 48, 50, 52, 45, 43, 45, 48, 50,
    ],
    &[60, 57, 55, 52, 50, 52, 55, 57],
    &[
        45, 48, 50, 55, 52, 57, 55, 60, 57, 64, 60, 67, 64, 60, 55, 52,
    ],
];

impl TonalEngine {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            step_trigger: GridTrigger::new(),
            step_index: 0,
            active_phrase: 0,
            last_cycle: None,
            evolved_phrase: tonal_phrase(0).to_vec(),
            voices: Vec::with_capacity(8),
            low_cut_l: TonalLowCut::new(sample_rate, TONAL_LOW_CUT_HZ),
            low_cut_r: TonalLowCut::new(sample_rate, TONAL_LOW_CUT_HZ),
            rng: StdRng::from_entropy(),
        }
    }

    pub(crate) fn next(
        &mut self,
        c: &TonalControls,
        tune: f32,
        timing: TimingContext,
    ) -> (f32, f32) {
        let phrase = tonal_phrase_index(c.phrase);
        self.sync_phrase(phrase);

        if self.step_trigger.pop(timing, c.rate_beats, c.offset_beats) {
            let cycle = tonal_cycle_index(timing.beat, c.step_interval_beats, c.offset_beats);
            if self.last_cycle.is_some_and(|last| last != cycle) {
                self.evolve_phrase(c.evolve_rate);
            }
            self.last_cycle = Some(cycle);

            let loop_len = tonal_loop_len(c.step_interval_beats, c.rate_beats);
            self.step_index = tonal_cycle_step(
                timing.beat,
                c.step_interval_beats,
                c.offset_beats,
                c.rate_beats,
            ) % loop_len;
            let note = if self.rng.gen_range(0.0f32..1.0) < c.randomness {
                TONAL_SCALE_MIDI[self.rng.gen_range(0..TONAL_SCALE_MIDI.len())]
            } else {
                self.evolved_phrase[self.step_index % self.evolved_phrase.len()]
            } + (c.octave.round() as i32) * 12;
            let hz = tonal_note_hz(note, tune);
            let decay_samples = timing.beats_to_samples(c.note_length_beats);
            let pan = self.rng.gen_range(-0.5f32..0.5);
            // A voice captures its level at trigger time, so a level of
            // exactly 0 would stay silent for its whole life — skip creating
            // it. Every RNG draw above still happens, keeping seeded renders
            // byte-identical.
            if c.level != 0.0 {
                self.voices.push(TonalVoice::new(
                    tonal_synth_type_index(c.synth_type),
                    note,
                    hz,
                    pan,
                    c.level,
                    decay_samples,
                    self.sample_rate,
                    c.attack,
                    c.release,
                ));
            }
        }

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for v in &mut self.voices {
            let (l, r) = v.next();
            dry_l += l;
            dry_r += r;
        }
        self.voices.retain(|v| !v.is_done());

        (self.low_cut_l.process(dry_l), self.low_cut_r.process(dry_r))
    }

    pub(crate) fn sync_phrase(&mut self, phrase: usize) {
        if phrase != self.active_phrase {
            self.active_phrase = phrase;
            self.evolved_phrase = tonal_phrase(phrase).to_vec();
            self.step_index = 0;
            self.last_cycle = None;
        }
    }

    pub(crate) fn evolve_phrase(&mut self, rate: f32) {
        let count = tonal_evolve_note_count(rate, self.evolved_phrase.len());
        let mut changed_positions = [usize::MAX; TONAL_MAX_EVOLVE_NOTES];
        for changed in 0..count {
            let mut pos = self.rng.gen_range(0..self.evolved_phrase.len());
            while changed_positions[..changed].contains(&pos) {
                pos = self.rng.gen_range(0..self.evolved_phrase.len());
            }
            changed_positions[changed] = pos;

            let old = self.evolved_phrase[pos];
            let mut next = old;
            for _ in 0..8 {
                next = TONAL_SCALE_MIDI[self.rng.gen_range(0..TONAL_SCALE_MIDI.len())];
                if next != old {
                    break;
                }
            }
            self.evolved_phrase[pos] = next;
        }
    }
}

pub(crate) fn tonal_phrase_index(value: f32) -> usize {
    (value.round() as i64).rem_euclid(TONAL_PHRASES.len() as i64) as usize
}

pub(crate) fn tonal_phrase(phrase: usize) -> &'static [i32] {
    TONAL_PHRASES[phrase % TONAL_PHRASES.len()]
}

pub(crate) fn tonal_loop_len(cycle_beats: f32, rate_beats: f32) -> usize {
    (cycle_beats / tonal_rate_beats(rate_beats))
        .round()
        .clamp(1.0, TONAL_MAX_LOOP_STEPS as f32) as usize
}

pub(crate) fn tonal_cycle_index(beat: f64, cycle_beats: f32, offset_beats: f32) -> u64 {
    let cycle = f64::from(tonal_cycle_beats(cycle_beats));
    let offset = f64::from(offset_beats).rem_euclid(cycle);
    ((beat - offset).max(0.0) / cycle).floor() as u64
}

pub(crate) fn tonal_cycle_step(
    beat: f64,
    cycle_beats: f32,
    offset_beats: f32,
    rate_beats: f32,
) -> usize {
    let cycle = f64::from(tonal_cycle_beats(cycle_beats));
    let offset = f64::from(offset_beats).rem_euclid(cycle);
    let local = (beat - offset).rem_euclid(cycle);
    (local / f64::from(tonal_rate_beats(rate_beats))).floor() as usize
}

pub(crate) fn tonal_rate_beats(rate_beats: f32) -> f32 {
    rate_beats.clamp(TONAL_RATE_BEATS_MIN, TONAL_RATE_BEATS_MAX)
}

pub(crate) fn tonal_cycle_beats(cycle_beats: f32) -> f32 {
    cycle_beats.clamp(TONAL_CYCLE_BEATS_MIN, TONAL_CYCLE_BEATS_MAX)
}

pub(crate) fn tonal_evolve_note_count(rate: f32, phrase_len: usize) -> usize {
    if phrase_len == 0 || rate <= 0.0 {
        return 0;
    }
    (rate.clamp(0.0, 1.0) * TONAL_MAX_EVOLVE_NOTES as f32)
        .ceil()
        .min(phrase_len as f32) as usize
}

pub(crate) fn tonal_note_hz(note: i32, tune: f32) -> f32 {
    midi_to_hz(note) * tune_ratio(tune)
}

pub(crate) struct TonalLowCut {
    pub(crate) alpha: f32,
    pub(crate) last_input: f32,
    pub(crate) last_output: f32,
}

impl TonalLowCut {
    pub(crate) fn new(sample_rate: f32, cutoff_hz: f32) -> Self {
        let sample_rate = sample_rate.max(1.0);
        let cutoff_hz = cutoff_hz.max(1.0);
        let dt = 1.0 / sample_rate;
        let rc = 1.0 / (TAU * cutoff_hz);
        Self {
            alpha: rc / (rc + dt),
            last_input: 0.0,
            last_output: 0.0,
        }
    }

    pub(crate) fn process(&mut self, input: f32) -> f32 {
        let output = self.alpha * (self.last_output + input - self.last_input);
        self.last_input = input;
        self.last_output = output;
        output
    }
}

pub(crate) enum TonalVoice {
    Sine(SineTonalVoice),
    Piano(Box<PianoTonalVoice>),
}

impl TonalVoice {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        synth_type: usize,
        midi_note: i32,
        hz: f32,
        pan: f32,
        level: f32,
        decay_samples: u64,
        sample_rate: f32,
        attack_time: f32,
        release_time: f32,
    ) -> Self {
        match synth_type {
            0 => Self::Sine(SineTonalVoice::new(
                hz,
                pan,
                level,
                decay_samples,
                sample_rate,
                attack_time,
                release_time,
            )),
            _ => Self::Piano(Box::new(PianoTonalVoice::new(
                piano_profile(synth_type),
                midi_note,
                hz,
                pan,
                level,
                decay_samples,
                sample_rate,
                attack_time,
                release_time,
            ))),
        }
    }

    pub(crate) fn next(&mut self) -> (f32, f32) {
        match self {
            Self::Sine(voice) => voice.next(),
            Self::Piano(voice) => voice.next(),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Self::Sine(voice) => voice.is_done(),
            Self::Piano(voice) => voice.is_done(),
        }
    }
}

/// Shape of the sine voice's legacy taper: `sqrt(remaining / total)`, i.e. a
/// release curve exponent of 0.5 with no separate attack stage.
const TONAL_SINE_RELEASE_POWER: f32 = 0.5;

/// Attack + release envelope shared by every tonal synth type. `elapsed` and
/// `total` are seconds; `attack`/`release` are user-owned control values
/// (also seconds), each clamped to the note's own duration so a slider set
/// longer than the note still reaches full volume and fully decays. `power`
/// is the release curve's shape exponent — this is the one piece of "harmonic
/// character" each voice keeps (the piano profile's `body_power`, or the
/// sine voice's fixed sqrt taper).
///
/// When `release >= total` (the default reset for every profile), this
/// reduces exactly to the note's original always-releasing shape: attack
/// ramps in linearly, and `(1 - t)^power` decays across the whole note.
pub(crate) fn tonal_envelope_gain(
    elapsed: f32,
    total: f32,
    attack: f32,
    release: f32,
    power: f32,
) -> f32 {
    let total = total.max(1e-6);

    // A control value of exactly 0 means "instant" — an attack of 0 seconds
    // reaches full volume on the very first sample rather than dividing by a
    // vanishingly small ramp time, and a release of 0 seconds holds at unity
    // until the literal last sample of the note.
    let attack_gain = if attack <= 0.0 {
        1.0
    } else {
        let attack_time = attack.min(total);
        (elapsed / attack_time).clamp(0.0, 1.0)
    };

    let release_gain = if release <= 0.0 {
        if elapsed >= total { 0.0 } else { 1.0 }
    } else {
        let release_time = release.min(total);
        let release_start = (total - release_time).max(0.0);
        if elapsed <= release_start {
            1.0
        } else {
            (1.0 - ((elapsed - release_start) / release_time).clamp(0.0, 1.0)).powf(power)
        }
    };

    attack_gain * release_gain
}

pub(crate) struct SineTonalVoice {
    pub(crate) primary: SineOscillator,
    pub(crate) detuned: SineOscillator,
    pub(crate) samples_remaining: u64,
    pub(crate) total_samples: u64,
    pub(crate) total_duration: f32,
    pub(crate) attack_time: f32,
    pub(crate) release_time: f32,
    pub(crate) pan_gains: (f32, f32),
    pub(crate) level: f32,
}

impl SineTonalVoice {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        hz: f32,
        pan: f32,
        level: f32,
        decay_samples: u64,
        sample_rate: f32,
        attack_time: f32,
        release_time: f32,
    ) -> Self {
        let total = decay_samples.max(1);
        let sample_rate = sample_rate.max(1.0);
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.004, sample_rate),
            samples_remaining: total,
            total_samples: total,
            total_duration: total as f32 / sample_rate,
            attack_time,
            release_time,
            pan_gains: StereoPanner::gains(pan),
            level,
        }
    }
    pub(crate) fn next(&mut self) -> (f32, f32) {
        if self.samples_remaining == 0 {
            return (0.0, 0.0);
        }
        let elapsed_samples = self.total_samples - self.samples_remaining;
        let elapsed = elapsed_samples as f32 / self.total_samples as f32 * self.total_duration;
        self.samples_remaining -= 1;
        let gain = tonal_envelope_gain(
            elapsed,
            self.total_duration,
            self.attack_time,
            self.release_time,
            TONAL_SINE_RELEASE_POWER,
        );
        let s =
            soft_clip((self.primary.next() + self.detuned.next() * 0.3) * 0.4) * gain * self.level;
        (s * self.pan_gains.0, s * self.pan_gains.1)
    }
    pub(crate) fn is_done(&self) -> bool {
        self.samples_remaining == 0
    }
}

pub(crate) struct PianoKeyframe {
    pub(crate) midi: i32,
    pub(crate) decay_factor: f32,
    pub(crate) harmonics: [f32; TONAL_PIANO_HARMONIC_COUNT],
}

#[derive(Clone, Copy)]
pub(crate) struct PianoProfile {
    pub(crate) keyframes: &'static [PianoKeyframe],
    pub(crate) amplitude: f32,
    pub(crate) body_power: f32,
    pub(crate) harmonic_tilt: f32,
    pub(crate) decay_low: f32,
    pub(crate) decay_high: f32,
    pub(crate) decay_scale: f32,
}

pub(crate) struct PianoTonalVoice {
    pub(crate) oscillators: [SineOscillator; TONAL_PIANO_HARMONIC_COUNT],
    pub(crate) profile: PianoProfile,
    pub(crate) harmonic_amplitudes: [f32; TONAL_PIANO_HARMONIC_COUNT],
    pub(crate) harmonic_decay_rates: [f32; TONAL_PIANO_HARMONIC_COUNT],
    pub(crate) samples_elapsed: u64,
    pub(crate) total_samples: u64,
    pub(crate) total_duration: f32,
    pub(crate) attack_time: f32,
    pub(crate) release_time: f32,
    pub(crate) pan_gains: (f32, f32),
    pub(crate) level: f32,
}

impl PianoTonalVoice {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        profile: PianoProfile,
        midi_note: i32,
        hz: f32,
        pan: f32,
        level: f32,
        decay_samples: u64,
        sample_rate: f32,
        attack_time: f32,
        release_time: f32,
    ) -> Self {
        let total = decay_samples.max(1);
        let sample_rate = sample_rate.max(1.0);
        Self {
            oscillators: std::array::from_fn(|index| {
                SineOscillator::new(hz * (index + 1) as f32, sample_rate)
            }),
            profile,
            harmonic_amplitudes: piano_harmonic_amplitudes(profile, midi_note),
            harmonic_decay_rates: piano_harmonic_decay_rates(profile, midi_note, hz),
            samples_elapsed: 0,
            total_samples: total,
            total_duration: total as f32 / sample_rate,
            attack_time,
            release_time,
            pan_gains: StereoPanner::gains(pan),
            level,
        }
    }

    pub(crate) fn next(&mut self) -> (f32, f32) {
        if self.samples_elapsed >= self.total_samples {
            return (0.0, 0.0);
        }

        let t = self.samples_elapsed as f32 / self.total_samples as f32;
        let elapsed = t * self.total_duration;
        self.samples_elapsed += 1;

        let mut sample = 0.0f32;
        for index in 0..TONAL_PIANO_HARMONIC_COUNT {
            let harmonic = self.oscillators[index].next();
            let decay = (-t * self.harmonic_decay_rates[index]).exp();
            sample += harmonic * self.harmonic_amplitudes[index] * decay;
        }

        let envelope = tonal_envelope_gain(
            elapsed,
            self.total_duration,
            self.attack_time,
            self.release_time,
            self.profile.body_power,
        );
        let s = soft_clip(sample * self.profile.amplitude) * envelope * self.level;
        (s * self.pan_gains.0, s * self.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.samples_elapsed >= self.total_samples
    }
}

pub(crate) fn piano_profile(synth_type: usize) -> PianoProfile {
    TONAL_PIANO_PROFILES[(synth_type.saturating_sub(1)) % TONAL_PIANO_PROFILES.len()]
}

pub(crate) fn piano_harmonic_amplitudes(
    profile: PianoProfile,
    midi_note: i32,
) -> [f32; TONAL_PIANO_HARMONIC_COUNT] {
    let (low, high) = piano_keyframe_pair(profile, midi_note);
    let t = ease_in_out(lerp_t(low.midi as f32, high.midi as f32, midi_note as f32));
    std::array::from_fn(|index| {
        let harmonic = (index + 1) as f32;
        let tilt = harmonic.powf(profile.harmonic_tilt);
        lerp(low.harmonics[index], high.harmonics[index], t) * tilt
    })
}

pub(crate) fn piano_harmonic_decay_rates(
    profile: PianoProfile,
    midi_note: i32,
    fundamental_hz: f32,
) -> [f32; TONAL_PIANO_HARMONIC_COUNT] {
    let (low, high) = piano_keyframe_pair(profile, midi_note);
    let t = ease_in_out(lerp_t(low.midi as f32, high.midi as f32, midi_note as f32));
    let frame_decay = lerp(low.decay_factor, high.decay_factor, t).max(0.25);
    std::array::from_fn(|index| {
        let harmonic_hz = fundamental_hz * (index + 1) as f32;
        let pitch_decay = lerp_t(80.0, 6_000.0, harmonic_hz);
        lerp(profile.decay_low, profile.decay_high, pitch_decay) * profile.decay_scale / frame_decay
    })
}

fn piano_keyframe_pair(
    profile: PianoProfile,
    midi_note: i32,
) -> (&'static PianoKeyframe, &'static PianoKeyframe) {
    let mut low = &profile.keyframes[0];
    let mut high = &profile.keyframes[profile.keyframes.len() - 1];

    for keyframe in profile.keyframes {
        if keyframe.midi <= midi_note && keyframe.midi >= low.midi {
            low = keyframe;
        }
        if keyframe.midi >= midi_note && keyframe.midi <= high.midi {
            high = keyframe;
        }
    }

    (low, high)
}

fn lerp_t(min: f32, max: f32, value: f32) -> f32 {
    if (max - min).abs() <= f32::EPSILON {
        0.0
    } else {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) * 0.5
    }
}
