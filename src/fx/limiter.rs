//! Master output ceiling: a stereo-linked lookahead peak limiter.
//!
//! Per sample, the gain a peak needs (`CEILING / peak`, else 1) enters a
//! sliding minimum held for `HOLD_SECONDS` (longer than a bass half-cycle, so
//! the gain does not ripple between waveform peaks); that floor recovers
//! toward 1 at the release rate, and a moving average over the lookahead
//! window smooths it. Each averaged value covers the sample `window - 1` behind the input,
//! so delaying the audio by that much makes every output sample's gain at or
//! below what its own peak required: the ceiling holds by construction, and
//! the onset is a `window`-long ramp rather than a step. Storage is sized
//! once at construction; `process` never allocates.

/// -1 dBFS sample peak, matching the downstream mixer's limiter.
pub(crate) const CEILING: f32 = 0.891_250_9;
const LOOKAHEAD_SECONDS: f32 = 0.0015;
const HOLD_SECONDS: f32 = 0.02;
const RELEASE_SECONDS: f32 = 0.1;

pub(crate) struct Limiter {
    /// The last `window` input frames; the oldest is the one leaving now.
    delay: Vec<(f32, f32)>,
    /// Required gains as a monotonic (non-decreasing) queue of
    /// `(sample index, gain)`, a ring of `hold` slots: the front is the
    /// minimum over the hold.
    floor: Vec<(u64, f32)>,
    floor_head: usize,
    floor_len: usize,
    /// The released floor over the last `window` samples, and its sum.
    released: Vec<f32>,
    released_sum: f64,
    /// Entries of `released` below 1; at zero the gain is exactly 1.
    reducing: usize,
    /// The released floor as reduction below unity, `1 - gain`. Kept as a
    /// distance so recovery decays geometrically to exactly zero; an f32
    /// gain stepping toward 1 by `(1 - g) * k` stalls once the step falls
    /// under half an ulp.
    reduction: f32,
    release: f32,
    position: usize,
    index: u64,
    gain: f32,
}

impl Limiter {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let window = ((LOOKAHEAD_SECONDS * sample_rate).ceil() as usize).max(1);
        let hold = ((HOLD_SECONDS * sample_rate).ceil() as usize).max(window);
        Self {
            delay: vec![(0.0, 0.0); window],
            floor: vec![(0, 1.0); hold],
            floor_head: 0,
            floor_len: 0,
            released: vec![1.0; window],
            released_sum: window as f64,
            reducing: 0,
            reduction: 0.0,
            release: (-1.0 / (RELEASE_SECONDS * sample_rate)).exp(),
            position: 0,
            index: 0,
            gain: 1.0,
        }
    }

    /// Output delay in samples.
    #[cfg(test)]
    pub(crate) fn latency(&self) -> usize {
        self.delay.len() - 1
    }

    /// Linear gain applied to the latest output sample; 1.0 is no reduction.
    #[cfg(test)]
    pub(crate) fn gain(&self) -> f32 {
        self.gain
    }

    pub(crate) fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let window = self.delay.len();
        let hold = self.floor.len();
        let peak = l.abs().max(r.abs());
        let required = if peak > CEILING { CEILING / peak } else { 1.0 };

        // Sliding minimum: drop expired gains from the front and larger ones
        // from the back, so the front is the least gain inside the hold and
        // at most `hold` entries are ever queued.
        while self.floor_len > 0 && self.index - self.floor[self.floor_head].0 >= hold as u64 {
            self.floor_head = (self.floor_head + 1) % hold;
            self.floor_len -= 1;
        }
        while self.floor_len > 0 {
            let back = (self.floor_head + self.floor_len - 1) % hold;
            if self.floor[back].1 < required {
                break;
            }
            self.floor_len -= 1;
        }
        self.floor[(self.floor_head + self.floor_len) % hold] = (self.index, required);
        self.floor_len += 1;
        let floor = self.floor[self.floor_head].1;

        // Instant down to the floor, exponential recovery toward 1. Within
        // 0.001 dB of unity it is unity: an exact 1.0 passes audio
        // bit-for-bit instead of crawling toward it for seconds.
        self.reduction *= self.release;
        if self.reduction < 1e-4 {
            self.reduction = 0.0;
        }
        self.reduction = self.reduction.max(1.0 - floor);
        let envelope = 1.0 - self.reduction;

        let leaving = std::mem::replace(&mut self.released[self.position], envelope);
        self.reducing = self.reducing + usize::from(envelope < 1.0) - usize::from(leaving < 1.0);
        self.gain = if self.reducing == 0 {
            self.released_sum = window as f64;
            1.0
        } else {
            self.released_sum += f64::from(envelope) - f64::from(leaving);
            (self.released_sum / window as f64) as f32
        };

        self.delay[self.position] = (l, r);
        // The frame `window - 1` behind the one just written.
        let out = self.delay[(self.position + 1) % window];
        self.position = (self.position + 1) % window;
        self.index += 1;
        // Float rounding in the running sum is the only way past the
        // ceiling; this never shapes the signal.
        (
            (out.0 * self.gain).clamp(-CEILING, CEILING),
            (out.1 * self.gain).clamp(-CEILING, CEILING),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// Deterministic full-band noise in -1..1 (xorshift), no RNG dependency.
    fn noise(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        *seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    #[test]
    fn material_under_the_ceiling_passes_bit_exact_after_the_latency() {
        let mut limiter = Limiter::new(RATE);
        let latency = limiter.latency();
        let mut seed = 1;
        let input: Vec<(f32, f32)> = (0..48_000)
            .map(|_| (noise(&mut seed) * CEILING, noise(&mut seed) * CEILING))
            .collect();
        let output: Vec<(f32, f32)> = input.iter().map(|(l, r)| limiter.process(*l, *r)).collect();
        assert!(output[..latency].iter().all(|frame| *frame == (0.0, 0.0)));
        assert_eq!(&output[latency..], &input[..input.len() - latency]);
        assert_eq!(limiter.gain(), 1.0);
    }

    /// Bursts of noise, square kicks and a low sine far above full scale, at
    /// every supported device rate: no sample ever leaves the ceiling, and
    /// the onset arrives as a ramp, not a jump.
    #[test]
    fn hot_signal_never_exceeds_the_ceiling() {
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut limiter = Limiter::new(rate);
            let mut seed = 7;
            let mut worst = 0.0_f32;
            for n in 0..(rate as usize * 4) {
                let t = n as f32 / rate;
                let burst = if (t * 2.0).fract() < 0.1 { 16.0 } else { 0.3 };
                let kick = if (t * 2.0).fract() < 0.01 { 8.0 } else { 0.0 };
                let sine = 4.0 * (std::f32::consts::TAU * 45.0 * t).sin();
                let (l, r) = limiter.process(
                    noise(&mut seed) * burst + kick + sine,
                    noise(&mut seed) * burst - kick + sine,
                );
                worst = worst.max(l.abs()).max(r.abs());
            }
            assert!(worst <= CEILING, "{rate} Hz peaked at {worst}");
            assert!(
                worst > CEILING * 0.95,
                "{rate} Hz never reached the ceiling"
            );
        }
    }

    /// One loud click: the gain dips to exactly what it required and comes
    /// back to exactly 1 (so ordinary material is untouched again), within
    /// hold plus a few release time constants.
    #[test]
    fn gain_returns_exactly_to_unity_after_a_peak() {
        let mut limiter = Limiter::new(RATE);
        limiter.process(2.0 * CEILING, 0.0);
        let mut least = 1.0_f32;
        let mut back_at = None;
        for n in 1..(2 * RATE as usize) {
            let (l, _) = limiter.process(0.0, 0.0);
            assert!(l.abs() <= CEILING);
            least = least.min(limiter.gain());
            if back_at.is_none() && limiter.gain() == 1.0 && n > limiter.latency() + 1 {
                back_at = Some(n);
            }
        }
        assert!((least - 0.5).abs() < 1e-3, "least gain {least}");
        let seconds = back_at.expect("gain never returned to unity") as f32 / RATE;
        // ln(0.5 / 1e-4) = 8.5 release time constants from half gain to unity.
        assert!(
            seconds < HOLD_SECONDS + 9.0 * RELEASE_SECONDS,
            "{seconds} s"
        );
    }

    #[test]
    fn processing_never_allocates() {
        let mut limiter = Limiter::new(RATE);
        let mut seed = 3;
        let ((), heap) = crate::alloc_count::count(|| {
            for _ in 0..48_000 {
                let x = noise(&mut seed) * 4.0;
                std::hint::black_box(limiter.process(x, -x));
            }
        });
        assert_eq!(heap.allocs + heap.frees, 0);
    }
}
