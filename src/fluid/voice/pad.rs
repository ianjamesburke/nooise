use super::*;

// ============================================================
// Pad engine (chord drones)
// ============================================================

pub(crate) const MAX_PAD_LAYERS: usize = 4;

pub(crate) struct PadEngine {
    pub(crate) sample_rate: f32,
    pub(crate) layers: Vec<PadLayer>,
    pub(crate) chord_trigger: GridTrigger,
    pub(crate) step_index: usize,
    pub(crate) last_progression: usize,
    pub(crate) width_lfo: DriftingLfo,
    pub(crate) air: WhiteNoise,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl PadEngine {
    pub(crate) fn new(sample_rate: f32, c: &PadControls, telemetry: Arc<FluidTelemetry>) -> Self {
        Self {
            sample_rate,
            layers: vec![PadLayer::new(
                0,
                0,
                0.0,
                sample_rate,
                c.attack_time,
                c.release_time,
            )],
            chord_trigger: GridTrigger::after_start(),
            step_index: 0,
            last_progression: 0,
            width_lfo: DriftingLfo::new(1.0 / 54.0, sample_rate),
            air: WhiteNoise::new(),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    pub(crate) fn next(&mut self, c: &PadControls, tune: f32, timing: TimingContext) -> (f32, f32) {
        let progression = (c.progression.round() as i64).rem_euclid(4) as usize;
        let progression_changed = progression != self.last_progression;
        self.last_progression = progression;

        let advance = self.chord_trigger.pop(timing, c.chord_bars * 4.0, 0.0);

        if advance || progression_changed {
            for layer in &mut self.layers {
                layer.release();
            }
            if advance {
                self.step_index = (self.step_index + 1) % 8;
            }
            self.telemetry
                .chord_index
                .store(self.step_index as u64, Ordering::Relaxed);
            if self.layers.len() >= MAX_PAD_LAYERS {
                let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
                self.layers.drain(0..remove_count);
            }
            self.layers.push(PadLayer::new(
                progression,
                self.step_index,
                tune,
                self.sample_rate,
                c.attack_time,
                c.release_time,
            ));
        }

        let width = c.stereo_width
            * (0.58
                + normalized_lfo(self.width_lfo.next(&mut self.rng, 1.0 / 86.0, 1.0 / 38.0))
                    * 0.16);
        let detune_mix = c.detune * 0.84;
        let octave_mix = c.octave_mix * 0.32;

        let mut dry_l = 0.0f32;
        let mut dry_r = 0.0f32;
        for layer in &mut self.layers {
            let (l, r) = layer.next_stereo(width, detune_mix, octave_mix);
            dry_l += l;
            dry_r += r;
        }
        self.layers.retain(|l| !l.is_done());

        let air = self.air.next_filtered(&mut self.rng, 0.0002) * 0.00025;

        (
            (dry_l * 0.58 + air) * c.level,
            (dry_r * 0.58 + air) * c.level,
        )
    }
}

pub(crate) struct PadLayer {
    pub(crate) tones: Vec<PadTone>,
}

impl PadLayer {
    pub(crate) fn new(
        progression: usize,
        step: usize,
        tune: f32,
        sample_rate: f32,
        attack_time: f32,
        release_time: f32,
    ) -> Self {
        Self {
            tones: pad_tones(
                progression,
                step,
                tune,
                sample_rate,
                attack_time,
                release_time,
            ),
        }
    }
    pub(crate) fn next_stereo(
        &mut self,
        width: f32,
        detune_mix: f32,
        octave_mix: f32,
    ) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for t in &mut self.tones {
            let (tl, tr) = t.next_stereo(width, detune_mix, octave_mix);
            l += tl;
            r += tr;
        }
        (l, r)
    }
    pub(crate) fn release(&mut self) {
        for t in &mut self.tones {
            t.release();
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.tones.iter().all(PadTone::is_done)
    }
}

pub(crate) struct PadTone {
    pub(crate) primary: SineOscillator,
    pub(crate) detuned: SineOscillator,
    pub(crate) octave: SineOscillator,
    pub(crate) envelope: Adsr,
    pub(crate) pan: f32,
    pub(crate) gain: f32,
}

impl PadTone {
    pub(crate) fn new(
        hz: f32,
        pan: f32,
        gain: f32,
        attack_time: f32,
        release_time: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            primary: SineOscillator::new(hz, sample_rate),
            detuned: SineOscillator::new(hz * 1.003, sample_rate),
            octave: SineOscillator::new(hz * 2.0, sample_rate),
            envelope: Adsr::new(attack_time, 12.0, 0.86, release_time, sample_rate),
            pan,
            gain,
        }
    }
    pub(crate) fn next_stereo(
        &mut self,
        width: f32,
        detune_mix: f32,
        octave_mix: f32,
    ) -> (f32, f32) {
        let s = self.primary.next()
            + self.detuned.next() * detune_mix
            + self.octave.next() * octave_mix;
        let shaped = soft_clip(s * 0.55) * self.envelope.next() * self.gain;
        StereoPanner::equal_power(shaped, self.pan * width)
    }
    pub(crate) fn release(&mut self) {
        self.envelope.note_off();
    }
    pub(crate) fn is_done(&self) -> bool {
        self.envelope.is_done()
    }
}

pub(crate) fn pad_tones(
    progression: usize,
    step: usize,
    tune: f32,
    sample_rate: f32,
    attack_time: f32,
    release_time: f32,
) -> Vec<PadTone> {
    let freqs = pad_chord(progression, step, tune);
    let pans = [-0.52_f32, -0.18, 0.16, 0.46];
    let gains = [0.17_f32, 0.132, 0.126, 0.098];
    freqs
        .iter()
        .zip(pans)
        .zip(gains)
        .map(|((hz, pan), gain)| {
            PadTone::new(*hz, pan, gain, attack_time, release_time, sample_rate)
        })
        .collect()
}

pub(crate) const PROGRESSIONS: [[[i32; 4]; 8]; 4] = [
    // Progression A: with an 8s release, each chord rings well into the next
    // (and beyond), so voicings are chosen to hold at least one common tone
    // across every step, including the loop back to step 0.
    [
        [45, 50, 55, 60], // Am
        [43, 50, 57, 60], // G   (holds D3/C4 from Am)
        [45, 52, 57, 60], // Am (alt voicing, holds A3/C4 from G)
        [47, 52, 55, 62], // B   (holds E3 from Am)
        [45, 52, 57, 64], // Am (alt voicing, holds E3 from B)
        [43, 50, 55, 62], // G   (parallel shift from Am, glides in stepwise)
        [48, 55, 60, 64], // C   (holds G3 from G)
        [55, 59, 64, 67], // Em (holds G3/C4 from C, and G3 back into Am)
    ],
    [
        [45, 50, 57, 60], // Am
        [50, 53, 57, 62], // Dm
        [48, 55, 60, 64], // C
        [43, 50, 55, 59], // G
        [41, 48, 53, 57], // F
        [52, 59, 64, 67], // Em
        [45, 52, 57, 60], // Am
        [43, 50, 55, 59], // G (non-tonic close, leads back to Am)
    ],
    [
        [45, 48, 52, 55], // Am7
        [41, 45, 48, 52], // Fmaj7
        [48, 52, 55, 59], // Cmaj7
        [43, 47, 50, 53], // G7
        [50, 53, 57, 60], // Dm7
        [52, 55, 59, 62], // Em7
        [47, 50, 53, 57], // Bm7b5 (half-diminished ii)
        [43, 50, 55, 59], // G (non-tonic close)
    ],
    [
        [45, 52, 57, 60], // Am, wide
        [41, 45, 48, 55], // Fmaj9-flavor
        [48, 55, 59, 62], // Cmaj9-flavor
        [43, 50, 53, 57], // G9-flavor
        [50, 57, 60, 64], // Dm9-flavor
        [52, 55, 59, 64], // Em, open
        [47, 53, 57, 64], // Bm7b5, wide (the "ache" chord)
        [43, 50, 55, 64], // G, wide (non-tonic close)
    ],
];

pub(crate) fn pad_chord(progression: usize, step: usize, tune: f32) -> [f32; 4] {
    PROGRESSIONS[progression % PROGRESSIONS.len()][step % 8]
        .map(|note| midi_to_hz(note) * tune_ratio(tune))
}
