//! Multi-operator FM synthesis: the shared primitive every FM-bodied voice
//! builds its timbre from. Owns operators, their pairing into
//! modulator→carrier voices, the parallel stack that sums them, and nothing
//! about any particular instrument — callers supply a base frequency per
//! sample and keep their own pitch envelopes, amplitude envelopes, filters,
//! and panning.
//!
//! The stack is deliberately not a general operator graph. Both shapes this
//! codebase needs are the same shape: a percussive FM body is one
//! modulator→carrier pair, and a formant/choir timbre is several such pairs
//! summed in parallel, each pair's carrier ratio placing one formant peak.
//! An arbitrary patch graph would only pay off if FM patching were exposed in
//! the app, which it is not.

use std::f32::consts::TAU;

/// Carrier waveform. Modulators are always sine: a non-sine modulator's
/// harmonics multiply into the sideband spectrum and the result stops being
/// predictable from ratio and index.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FmWave {
    Sine,
    /// Naive (non-band-limited) triangle: odd harmonics falling off as 1/n²,
    /// consistent with this codebase's additive-approximation oscillators
    /// elsewhere. Thickens a body without adding edge.
    Triangle,
}

impl FmWave {
    /// Must be well-defined for any phase, not just `[0, TAU)`: modulation
    /// pushes a carrier's phase outside the wrapped range.
    #[inline]
    fn shape(self, phase: f32) -> f32 {
        match self {
            Self::Sine => phase.sin(),
            Self::Triangle => {
                let t = phase / TAU;
                4.0 * (t - (t + 0.5).floor()).abs() - 1.0
            }
        }
    }
}

/// One modulator→carrier pair: the smallest unit that produces an FM timbre.
///
/// Both ratios are relative to the base frequency, the standard FM
/// convention. `mod_ratio` decides sideband spacing — integer ratios fuse
/// into one harmonic tone, non-integer ratios read hollow or metallic.
/// `index` is the modulation depth in units of the *carrier* frequency (peak
/// deviation = `index * carrier_ratio * base_hz`), which sets how much
/// spectral width the pair has. `index_decay` is the per-sample multiplier
/// applied to `index`, so a percussive pair can start bright and settle;
/// `1.0` holds the index steady for a sustained timbre.
///
/// `carrier_ratio` is what makes this usable for formants: parking the
/// carrier at a harmonic well above the base (ratio 4–8) with the modulator
/// at ratio 1 produces a resonant peak near the carrier frequency, which is
/// how a vowel-like formant is approximated with two operators.
#[derive(Clone)]
pub(crate) struct FmPair {
    mod_ratio: f32,
    carrier_ratio: f32,
    carrier_wave: FmWave,
    index: f32,
    index_decay: f32,
    /// Multiplied into the base frequency for this pair only, so several
    /// pairs voicing the same note can sit slightly apart. Real ensembles are
    /// never exactly in tune, and exactly-in-tune parallel pairs fuse into
    /// one tone instead of reading as several voices.
    detune: f32,
    /// This pair's contribution to the summed stack output.
    level: f32,
    mod_phase: f32,
    carrier_phase: f32,
}

impl FmPair {
    pub(crate) fn new(mod_ratio: f32, carrier_ratio: f32, index: f32) -> Self {
        Self {
            mod_ratio,
            carrier_ratio,
            carrier_wave: FmWave::Sine,
            index,
            index_decay: 1.0,
            detune: 1.0,
            level: 1.0,
            mod_phase: 0.0,
            carrier_phase: 0.0,
        }
    }

    pub(crate) fn with_wave(mut self, wave: FmWave) -> Self {
        self.carrier_wave = wave;
        self
    }

    /// `tau_samples` is the time constant over which the modulation index
    /// falls to 1/e of its starting value.
    pub(crate) fn with_index_decay(mut self, tau_samples: f32) -> Self {
        self.index_decay = (-1.0 / tau_samples.max(1.0)).exp();
        self
    }

    // Detune and level are the stack's parallel-layering surface: they only
    // do anything once a stack holds more than one pair, and the kick (one
    // pair) is currently the only consumer. Exercised by this module's tests.
    #[allow(dead_code)]
    pub(crate) fn with_detune(mut self, detune: f32) -> Self {
        self.detune = detune;
        self
    }

    #[allow(dead_code)]
    pub(crate) fn with_level(mut self, level: f32) -> Self {
        self.level = level;
        self
    }

    /// Advances the modulator and carrier one sample and returns the
    /// carrier's phase, before waveform shaping.
    ///
    /// The operation order here is load-bearing and must not be rearranged:
    /// the modulator advances and is read, the index decays, and only then
    /// does the carrier advance using that sample's deviation. Kick type 0 is
    /// pinned byte-for-byte against a pre-existing render
    /// (`kick_type_zero_matches_legacy_sub_voice_exactly`), and reordering
    /// these steps changes float rounding even where it is behaviour-neutral.
    #[inline]
    fn next_carrier_phase(&mut self, base_hz: f32, sample_rate: f32) -> f32 {
        let carrier_hz = base_hz * self.carrier_ratio;

        self.mod_phase += TAU * (base_hz * self.mod_ratio) / sample_rate;
        if self.mod_phase >= TAU {
            self.mod_phase -= TAU;
        }
        let deviation = self.mod_phase.sin() * self.index * carrier_hz;
        self.index *= self.index_decay;

        self.carrier_phase += TAU * (carrier_hz + deviation) / sample_rate;
        if self.carrier_phase >= TAU {
            self.carrier_phase -= TAU;
        }
        self.carrier_phase
    }

    #[inline]
    fn next(&mut self, base_hz: f32, sample_rate: f32) -> f32 {
        let phase = self.next_carrier_phase(base_hz * self.detune, sample_rate);
        self.carrier_wave.shape(phase) * self.level
    }
}

/// How many parallel pairs one stack can hold. A percussive body uses one; a
/// formant timbre uses one pair per vowel peak, and four peaks already
/// exceeds what is distinguishable once a chord's worth of them are summed.
pub(crate) const MAX_FM_PAIRS: usize = 4;

/// A fixed set of modulator→carrier pairs summed in parallel. Fixed-size and
/// allocation-free: voices are constructed inside the audio callback on every
/// trigger, so a `Vec` here would allocate on the realtime thread.
///
/// The stack carries no output trim of its own. Loudness matching between
/// configurations belongs to the consumer, which is the only layer that knows
/// where in its own chain (before or after an amplitude envelope, a click
/// layer, a filter) the trim has to sit. What the stack does owe every
/// consumer is that a trim chosen by ear is not good enough: pin it with a
/// test that measures the rendered level, or it drifts silently as the timbre
/// is edited.
#[derive(Clone)]
pub(crate) struct FmStack {
    pairs: [Option<FmPair>; MAX_FM_PAIRS],
    sample_rate: f32,
}

impl FmStack {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            pairs: [const { None }; MAX_FM_PAIRS],
            sample_rate,
        }
    }

    /// Adds a pair. Silently ignores anything past `MAX_FM_PAIRS`; the cap is
    /// a spectral-usefulness bound, not a correctness one, and a stack is
    /// always built from a fixed literal recipe rather than from user input.
    pub(crate) fn with_pair(mut self, pair: FmPair) -> Self {
        if let Some(slot) = self.pairs.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(pair);
        }
        self
    }

    /// One sample of the summed stack at `base_hz`. The caller owns the pitch
    /// envelope, so this is read fresh every sample rather than cached.
    #[inline]
    pub(crate) fn next(&mut self, base_hz: f32) -> f32 {
        let sample_rate = self.sample_rate;
        let mut sum = 0.0;
        for pair in self.pairs.iter_mut().flatten() {
            sum += pair.next(base_hz, sample_rate);
        }
        sum
    }
}

/// Root-mean-square level of a rendered buffer. The measure loudness-matching
/// tests compare against: peak alone is dominated by a single transient
/// sample and does not track what a listener balances by.
#[cfg(test)]
pub(crate) fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(stack: &mut FmStack, base_hz: f32, samples: usize) -> Vec<f32> {
        (0..samples).map(|_| stack.next(base_hz)).collect()
    }

    /// A zero-index pair is a plain oscillator: no modulation means the
    /// carrier is exactly its own waveform, which is what makes the stack
    /// usable for non-FM layers without a separate code path.
    #[test]
    fn a_zero_index_pair_is_a_plain_carrier_oscillator() {
        let mut stack = FmStack::new(48_000.0).with_pair(FmPair::new(1.0, 1.0, 0.0));
        let mut reference_phase = 0.0f32;

        for sample in render(&mut stack, 440.0, 512) {
            reference_phase += TAU * 440.0 / 48_000.0;
            if reference_phase >= TAU {
                reference_phase -= TAU;
            }
            assert!((sample - reference_phase.sin()).abs() < 1e-6);
        }
    }

    /// Index decay is what separates a percussive body from a sustained one:
    /// the spectrum has to narrow over the hit, not hold its opening
    /// brightness.
    #[test]
    fn index_decay_narrows_the_spectrum_over_time() {
        let mut stack =
            FmStack::new(48_000.0).with_pair(FmPair::new(2.0, 1.0, 4.0).with_index_decay(2_000.0));
        let rendered = render(&mut stack, 110.0, 48_000);

        let opening = rms(&rendered[..2_000]);
        let tail = rms(&rendered[40_000..]);
        assert!(
            opening > tail,
            "expected the decaying index to shed energy: {opening} vs {tail}"
        );
    }

    /// Parallel pairs must sum, not replace: the formant technique depends on
    /// several carriers sounding at once.
    #[test]
    fn parallel_pairs_sum_into_one_output() {
        let recipe = || FmPair::new(1.0, 1.0, 0.0).with_level(0.5);
        let mut single = FmStack::new(48_000.0).with_pair(recipe());
        let mut doubled = FmStack::new(48_000.0)
            .with_pair(recipe())
            .with_pair(recipe());

        for (one, two) in
            render(&mut single, 220.0, 256)
                .into_iter()
                .zip(render(&mut doubled, 220.0, 256))
        {
            assert!((two - one * 2.0).abs() < 1e-6);
        }
    }

    /// Detune has to move a pair off the base frequency, or stacked formant
    /// pairs fuse into a single tone instead of an ensemble.
    #[test]
    fn detune_shifts_a_pairs_base_frequency() {
        let mut tuned = FmStack::new(48_000.0).with_pair(FmPair::new(1.0, 1.0, 0.0));
        let mut detuned =
            FmStack::new(48_000.0).with_pair(FmPair::new(1.0, 1.0, 0.0).with_detune(1.01));

        let a = render(&mut tuned, 220.0, 48_000);
        let b = render(&mut detuned, 220.0, 48_000);
        let difference = rms(&a.iter().zip(&b).map(|(x, y)| x - y).collect::<Vec<_>>());
        assert!(difference > 0.1, "detuned pair tracked the tuned one");
    }

    /// The cap is a silent bound by design; overflowing it must not panic or
    /// corrupt the pairs already placed.
    #[test]
    fn pairs_past_the_cap_are_dropped_without_disturbing_the_rest() {
        let recipe = || FmPair::new(1.0, 1.0, 0.0).with_level(1.0);
        let mut capped = FmStack::new(48_000.0);
        for _ in 0..MAX_FM_PAIRS + 3 {
            capped = capped.with_pair(recipe());
        }

        let peak = render(&mut capped, 220.0, 2_000)
            .into_iter()
            .fold(0.0f32, |acc, s| acc.max(s.abs()));
        assert!(peak <= MAX_FM_PAIRS as f32 + 1e-3);
    }
}
