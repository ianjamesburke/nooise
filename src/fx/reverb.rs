//! Stereo feedback-delay-network reverb with input diffusion and damped tails.
//! Used by Reverb module slots and the Bloom gesture.

struct DelayLine {
    buffer: Vec<f32>,
    index: usize,
    filter: f32,
}

impl DelayLine {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            index: 0,
            filter: 0.0,
        }
    }

    fn advance(&mut self, input: f32) {
        self.buffer[self.index] = input;
        self.index = (self.index + 1) % self.buffer.len();
    }
}

struct Diffuser {
    line: DelayLine,
}

impl Diffuser {
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.line.buffer[self.line.index];
        let output = delayed - input * 0.7;
        self.line.advance(input + output * 0.7);
        output
    }
}

/// Eight delay lines mixed through an energy-preserving Hadamard matrix.
/// Inputs diffuse before entering the orthogonal feedback network.
pub(crate) struct FdnReverb {
    lines: Box<[DelayLine; 8]>,
    diffusers: Box<[[Diffuser; 3]; 2]>,
    active: bool,
}

impl FdnReverb {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let length = |ms: f32| (sample_rate * ms / 1000.0).round().max(1.0) as usize;
        let lengths = [29.7, 37.1, 41.1, 43.7, 53.1, 59.3, 67.7, 71.9];
        let diffusion = [[4.8, 7.3, 12.7], [5.3, 8.9, 13.7]];
        Self {
            lines: Box::new(std::array::from_fn(|i| DelayLine::new(length(lengths[i])))),
            diffusers: Box::new(std::array::from_fn(|channel| {
                std::array::from_fn(|i| Diffuser {
                    line: DelayLine::new(length(diffusion[channel][i])),
                })
            })),
            active: false,
        }
    }

    pub(crate) fn process(
        &mut self,
        input_left: f32,
        input_right: f32,
        params: ReverbParams,
    ) -> (f32, f32) {
        if !self.active {
            if input_left == 0.0 && input_right == 0.0 {
                return (0.0, 0.0);
            }
            self.active = true;
        }
        let mut input = [input_left, input_right];
        for (sample, chain) in input.iter_mut().zip(self.diffusers.iter_mut()) {
            for diffuser in chain {
                *sample = diffuser.process(*sample);
            }
        }
        let feedback = 0.55 + params.room_size.clamp(0.0, 1.0) * 0.40;
        let damping = params.damp.clamp(0.0, 1.0) * 0.75;
        let mut values = [0.0; 8];
        for (value, line) in values.iter_mut().zip(self.lines.iter_mut()) {
            let delayed = line.buffer[line.index];
            line.filter = delayed * (1.0 - damping) + line.filter * damping;
            *value = line.filter;
        }
        // Orthogonal mixing prevents feedback gain increasing with density.
        let mut mixed = values;
        for stride in [1, 2, 4] {
            for base in (0..8).step_by(stride * 2) {
                for offset in 0..stride {
                    let a = mixed[base + offset];
                    let b = mixed[base + offset + stride];
                    mixed[base + offset] = a + b;
                    mixed[base + offset + stride] = a - b;
                }
            }
        }
        let norm = std::f32::consts::FRAC_1_SQRT_2 / 2.0;
        for (i, line) in self.lines.iter_mut().enumerate() {
            let injection = if i % 2 == 0 { input[0] } else { input[1] };
            line.advance(injection * 0.25 + mixed[i] * norm * feedback);
        }
        let left = values[0] + values[2] - values[4] - values[6];
        let right = values[1] + values[3] - values[5] - values[7];
        (left * 1.05, right * 1.05)
    }

    /// Clear storage incrementally without allocating on the audio thread.
    pub(crate) fn clear_chunk(&mut self, cursor: &mut usize, samples: usize) -> bool {
        let total: usize = self
            .lines
            .iter()
            .map(|line| line.buffer.len())
            .sum::<usize>()
            + self
                .diffusers
                .iter()
                .flatten()
                .map(|d| d.line.buffer.len())
                .sum::<usize>();
        let mut skip = *cursor;
        let mut remaining = samples;
        for line in self
            .lines
            .iter_mut()
            .chain(self.diffusers.iter_mut().flatten().map(|d| &mut d.line))
        {
            if skip >= line.buffer.len() {
                skip -= line.buffer.len();
                continue;
            }
            let count = remaining.min(line.buffer.len() - skip);
            line.buffer[skip..skip + count].fill(0.0);
            remaining -= count;
            skip = 0;
        }
        *cursor = cursor.saturating_add(samples - remaining).min(total);
        if *cursor < total {
            return false;
        }
        for line in self
            .lines
            .iter_mut()
            .chain(self.diffusers.iter_mut().flatten().map(|d| &mut d.line))
        {
            line.index = 0;
            line.filter = 0.0;
        }
        self.active = false;
        true
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ReverbParams {
    pub(crate) room_size: f32,
    pub(crate) damp: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_clear_returns_used_reverb_to_exact_silence() {
        let mut reverb = FdnReverb::new(44_100.0);
        let params = ReverbParams {
            room_size: 0.9,
            damp: 0.5,
        };
        for _ in 0..4_000 {
            reverb.process(0.5, 0.5, params);
        }
        let mut cursor = 0;
        while !reverb.clear_chunk(&mut cursor, 47) {}
        assert_eq!(reverb.process(0.0, 0.0, params), (0.0, 0.0));
    }

    #[test]
    fn impulse_tail_is_stereo_dense_and_stable() {
        for rate in [44_100.0, 48_000.0] {
            let mut reverb = FdnReverb::new(rate);
            let params = ReverbParams {
                room_size: 1.0,
                damp: 0.4,
            };
            let mut nonzero = 0;
            let mut different = 0;
            let mut peak = 0.0_f32;
            for i in 0..(rate as usize * 4) {
                let x = if i == 0 { 1.0 } else { 0.0 };
                let (l, r) = reverb.process(x, x, params);
                assert!(l.is_finite() && r.is_finite());
                peak = peak.max(l.abs()).max(r.abs());
                if i > rate as usize / 2 && l.abs() > 1e-9 {
                    nonzero += 1;
                }
                if (l - r).abs() > 1e-9 {
                    different += 1;
                }
            }
            assert!(peak < 1.0, "peak {peak}");
            assert!(nonzero > rate as usize, "density {nonzero}");
            assert!(different > rate as usize, "stereo {different}");
        }
    }
    #[test]
    fn sustained_low_frequency_and_noise_remain_bounded() {
        for noise in [false, true] {
            let mut reverb = FdnReverb::new(48_000.0);
            let params = ReverbParams {
                room_size: 1.0,
                damp: 0.0,
            };
            let mut seed = 1_u32;
            let mut peak = 0.0_f32;
            let mut energy = 0.0_f64;
            for i in 0..(48_000 * 4) {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let x = if noise {
                    (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.1
                } else {
                    (i as f32 * std::f32::consts::TAU * 80.0 / 48_000.0).sin() * 0.1
                };
                let (l, r) = reverb.process(x, x, params);
                assert!(l.is_finite() && r.is_finite());
                peak = peak.max(l.abs()).max(r.abs());
                if i >= 48_000 {
                    energy += f64::from(l * l + r * r);
                }
            }
            let rms = (energy / (48_000.0 * 3.0 * 2.0)).sqrt();
            assert!(peak < 0.8, "peak {peak} noise={noise}");
            assert!(rms < 0.25, "rms {rms} noise={noise}");
        }
    }

    #[test]
    #[ignore = "writes an impulse response for listening"]
    fn render_reverb_impulse() {
        let path = std::env::var("NOOISE_REVERB_WAV").expect("set NOOISE_REVERB_WAV");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let mut reverb = FdnReverb::new(48_000.0);
        let params = ReverbParams {
            room_size: 0.8,
            damp: 0.4,
        };
        for i in 0..(48_000 * 6) {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = reverb.process(x, x, params);
            writer.write_sample(l).unwrap();
            writer.write_sample(r).unwrap();
        }
        writer.finalize().unwrap();
    }
}
