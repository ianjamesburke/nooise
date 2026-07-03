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
pub(crate) const TONAL_RATE_BEATS_MIN: f32 = 0.25;
pub(crate) const TONAL_RATE_BEATS_MAX: f32 = 4.0;
pub(crate) const TONAL_CYCLE_BEATS_MIN: f32 = TONAL_RATE_BEATS_MIN;
pub(crate) const TONAL_CYCLE_BEATS_MAX: f32 = 16.0;
pub(crate) const TONAL_MAX_LOOP_STEPS: usize = 64;
pub(crate) const TONAL_MAX_EVOLVE_NOTES: usize = 4;
pub(crate) const TONAL_SCALE_MIDI: [i32; 10] = [45, 48, 50, 52, 55, 57, 60, 62, 64, 67];
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
            };
            let hz = tonal_note_hz(note, tune);
            let decay_samples = timing.beats_to_samples(c.note_length_beats);
            let pan = self.rng.gen_range(-0.5f32..0.5);
            self.voices.push(TonalVoice::new(
                hz,
                pan,
                c.level,
                decay_samples,
                self.sample_rate,
            ));
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

pub(crate) struct TonalVoice {
    pub(crate) primary: SineOscillator,
    pub(crate) detuned: SineOscillator,
    pub(crate) samples_remaining: u64,
    pub(crate) total_samples: u64,
    pub(crate) pan: f32,
    pub(crate) level: f32,
}

impl TonalVoice {
    pub(crate) fn new(hz: f32, pan: f32, level: f32, decay_samples: u64, sample_rate: f32) -> Self {
        let total = decay_samples.max(1);
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.004, sample_rate),
            samples_remaining: total,
            total_samples: total,
            pan,
            level,
        }
    }
    pub(crate) fn next(&mut self) -> (f32, f32) {
        if self.samples_remaining == 0 {
            return (0.0, 0.0);
        }
        let gain = (self.samples_remaining as f32 / self.total_samples as f32).sqrt();
        self.samples_remaining -= 1;
        let s =
            soft_clip((self.primary.next() + self.detuned.next() * 0.3) * 0.4) * gain * self.level;
        StereoPanner::equal_power(s, self.pan)
    }
    pub(crate) fn is_done(&self) -> bool {
        self.samples_remaining == 0
    }
}
