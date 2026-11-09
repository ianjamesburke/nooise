//! Freeverb-style stereo reverb: damped comb filters into allpass diffusers.
//! Used by Reverb module slots.

struct Comb {
    buffer: Vec<f32>,
    index: usize,
    feedback: f32,
    filter_store: f32,
    damp1: f32,
    damp2: f32,
}

impl Comb {
    /// Feedback and damping are left at pass-through values: `Freeverb::process`
    /// writes both from its per-call params before any sample is read.
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            index: 0,
            feedback: 0.0,
            filter_store: 0.0,
            damp1: 0.0,
            damp2: 1.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.index];
        self.filter_store = output * self.damp2 + self.filter_store * self.damp1;
        self.buffer[self.index] = input + self.filter_store * self.feedback;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
        output
    }
}

struct AllPass {
    buffer: Vec<f32>,
    index: usize,
    feedback: f32,
}

impl AllPass {
    fn new(size: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            index: 0,
            feedback,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = -input + buffered;
        self.buffer[self.index] = input + buffered * self.feedback;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
        output
    }
}

pub(crate) struct Freeverb {
    combs_left: Vec<Comb>,
    combs_right: Vec<Comb>,
    allpasses_left: Vec<AllPass>,
    allpasses_right: Vec<AllPass>,
    /// Sticky: false until the first nonzero input sample. While every input
    /// so far has been zero, every comb/allpass buffer and filter holds exact
    /// zeros and the output is exact silence, so `process` skips all work.
    /// Once audio arrives it stays active so the tail always rings out.
    active: bool,
}

impl Freeverb {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / 44_100.0;
        let comb_tunings = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        let allpass_tunings = [556, 441, 341, 225];

        let build_combs = |offset: i32| -> Vec<Comb> {
            comb_tunings
                .iter()
                .map(|size| Comb::new(((*size + offset) as f32 * scale) as usize))
                .collect()
        };
        let combs_left = build_combs(0);
        let combs_right = build_combs(23);
        let build_allpasses = |offset: i32| -> Vec<AllPass> {
            allpass_tunings
                .iter()
                .map(|size| AllPass::new(((*size + offset) as f32 * scale) as usize, 0.5))
                .collect()
        };
        let allpasses_left = build_allpasses(0);
        let allpasses_right = build_allpasses(23);

        Self {
            combs_left,
            combs_right,
            allpasses_left,
            allpasses_right,
            active: false,
        }
    }

    pub(crate) fn process(
        &mut self,
        input_left: f32,
        input_right: f32,
        params: ReverbParams,
    ) -> (f32, f32) {
        self.set_character(params);
        let input = (input_left + input_right) * 0.5;
        if !self.active {
            if input == 0.0 {
                return (0.0, 0.0);
            }
            self.active = true;
        }
        let mut left = self
            .combs_left
            .iter_mut()
            .map(|comb| comb.process(input))
            .sum::<f32>();
        let mut right = self
            .combs_right
            .iter_mut()
            .map(|comb| comb.process(input))
            .sum::<f32>();

        for allpass in &mut self.allpasses_left {
            left = allpass.process(left);
        }
        for allpass in &mut self.allpasses_right {
            right = allpass.process(right);
        }

        (left * 0.18, right * 0.18)
    }

    /// Clear at most `samples` stored samples without allocating. Gesture
    /// retirement uses this to amortize cleanup across audio frames after its
    /// return has faded fully silent.
    pub(crate) fn clear_chunk(&mut self, cursor: &mut usize, samples: usize) -> bool {
        let total = self
            .combs_left
            .iter()
            .chain(&self.combs_right)
            .map(|comb| comb.buffer.len())
            .sum::<usize>()
            + self
                .allpasses_left
                .iter()
                .chain(&self.allpasses_right)
                .map(|allpass| allpass.buffer.len())
                .sum::<usize>();
        let mut skip = *cursor;
        let mut remaining = samples;
        for comb in self.combs_left.iter_mut().chain(&mut self.combs_right) {
            clear_buffer_chunk(&mut comb.buffer, &mut skip, &mut remaining);
        }
        for allpass in self
            .allpasses_left
            .iter_mut()
            .chain(&mut self.allpasses_right)
        {
            clear_buffer_chunk(&mut allpass.buffer, &mut skip, &mut remaining);
        }
        *cursor = cursor.saturating_add(samples - remaining).min(total);
        if *cursor < total {
            return false;
        }
        for comb in self.combs_left.iter_mut().chain(&mut self.combs_right) {
            comb.index = 0;
            comb.filter_store = 0.0;
        }
        for allpass in self
            .allpasses_left
            .iter_mut()
            .chain(&mut self.allpasses_right)
        {
            allpass.index = 0;
        }
        self.active = false;
        true
    }

    fn set_character(&mut self, params: ReverbParams) {
        let feedback = 0.28 + params.room_size.clamp(0.0, 1.0) * 0.68;
        let damp1 = params.damp.clamp(0.0, 1.0) * 0.4;
        for comb in self.combs_left.iter_mut().chain(&mut self.combs_right) {
            comb.feedback = feedback;
            comb.damp1 = damp1;
            comb.damp2 = 1.0 - damp1;
        }
    }
}

fn clear_buffer_chunk(buffer: &mut [f32], skip: &mut usize, remaining: &mut usize) {
    if *remaining == 0 {
        return;
    }
    if *skip >= buffer.len() {
        *skip -= buffer.len();
        return;
    }
    let start = *skip;
    let count = (*remaining).min(buffer.len() - start);
    buffer[start..start + count].fill(0.0);
    *remaining -= count;
    *skip = 0;
}

/// Per-call character, matching the `DelayParams` convention. Comb/allpass
/// buffer lengths stay construction-time because they depend only on the
/// sample rate; Size and Damping are plain coefficients the caller is free to
/// move every sample.
#[derive(Clone, Copy)]
pub(crate) struct ReverbParams {
    pub(crate) room_size: f32,
    pub(crate) damp: f32,
}

/// Long modulated hall used by module slots.
///
/// Six feedback delays, about 43–95 ms, mixed by a Householder matrix and
/// read with a slow fractional-delay drift of a fraction of a millisecond.
/// The drift is what keeps a long decay from locking onto the comb modes of
/// an unmodulated Freeverb. Damping is lighter than a small room, the input
/// is high-passed, and a mild bass cut in the loop stops a long tail from
/// booming. A fixed predelay keeps the onset off the dry signal. Bloom keeps
/// [`Freeverb`].
pub(crate) struct ModulatedHall {
    lines: [HallLine; 6],
    gains: [f32; 6],
    pre_l: Vec<f32>,
    pre_r: Vec<f32>,
    pre_index: usize,
    pre_l_tap: usize,
    pre_r_tap: usize,
    diffuse_l: HallAllpass,
    diffuse_r: HallAllpass,
    hp_l: f32,
    hp_r: f32,
    hp_prev_l: f32,
    hp_prev_r: f32,
    hp_a: f32,
    damp_coeff: f32,
    smooth_a: f32,
    bass_a: f32,
    sample_rate: f32,
    primed: bool,
    active: bool,
}

struct HallLine {
    buffer: Vec<f32>,
    write: usize,
    nominal: f32,
    depth: f32,
    lowpass: f32,
    bass: f32,
    sin: f32,
    cos: f32,
    rot_sin: f32,
    rot_cos: f32,
}

struct HallAllpass {
    buffer: Vec<f32>,
    index: usize,
    feedback: f32,
}

/// Delays at 44.1 kHz. Incommensurate, long enough to read as a hall.
const HALL_DELAYS: [usize; 6] = [1201, 1601, 2111, 2707, 3401, 4211];
const HALL_RATES_HZ: [f32; 6] = [0.11, 0.17, 0.13, 0.23, 0.19, 0.29];
const HALL_DEPTH: [f32; 6] = [10.0, 14.0, 8.0, 16.0, 12.0, 18.0];
const HALL_PHASE: [f32; 6] = [0.2, 1.3, 2.4, 3.5, 4.6, 5.4];
const HALL_INPUT_GAIN: f32 = 0.35;
/// Puts factory Amount 40% in the same loudness neighborhood as the old
/// room. One algorithm constant, not a per-file normalize.
const HALL_WET_GAIN: f32 = 2.40;
const HALL_PREDELAY_MS: (f32, f32) = (10.0, 14.0);

impl HallAllpass {
    fn new(samples: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; samples.max(4)],
            index: 0,
            feedback,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = buffered - input;
        self.buffer[self.index] = input + buffered * self.feedback;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
        output
    }
}

impl HallLine {
    fn new(delay: usize, rate_hz: f32, depth: f32, phase: f32, sample_rate: f32) -> Self {
        let scale = sample_rate / 44_100.0;
        let nominal = (delay as f32 * scale).max(32.0);
        let depth = depth * scale;
        let len = (nominal + depth + 8.0).ceil() as usize;
        let omega = std::f32::consts::TAU * rate_hz / sample_rate.max(1.0);
        Self {
            buffer: vec![0.0; len],
            write: 0,
            nominal,
            depth,
            lowpass: 0.0,
            bass: 0.0,
            sin: phase.sin(),
            cos: phase.cos(),
            rot_sin: omega.sin(),
            rot_cos: omega.cos(),
        }
    }

    fn read(&mut self) -> f32 {
        let sin = self.sin * self.rot_cos + self.cos * self.rot_sin;
        let cos = self.cos * self.rot_cos - self.sin * self.rot_sin;
        self.sin = sin;
        self.cos = cos;
        if self.write.is_multiple_of(1024) {
            let normal = (self.sin * self.sin + self.cos * self.cos).sqrt();
            if normal > 1e-12 {
                self.sin /= normal;
                self.cos /= normal;
            }
        }
        let len = self.buffer.len();
        let delay = (self.nominal + self.depth * self.sin).clamp(8.0, (len - 3) as f32);
        let mut pos = self.write as f32 - delay;
        let len_f = len as f32;
        if pos < 0.0 {
            pos += len_f;
        }
        let index = pos.floor() as usize % len;
        let frac = pos - pos.floor();
        let next = (index + 1) % len;
        self.buffer[index] + (self.buffer[next] - self.buffer[index]) * frac
    }

    fn write(&mut self, sample: f32) {
        self.buffer[self.write] = sample;
        self.write += 1;
        if self.write >= self.buffer.len() {
            self.write = 0;
        }
    }
}

impl ModulatedHall {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / 44_100.0;
        let tap = |ms: f32| ((ms * 0.001 * sample_rate).round() as usize).max(1);
        let pre_l_tap = tap(HALL_PREDELAY_MS.0);
        let pre_r_tap = tap(HALL_PREDELAY_MS.1);
        let pre_len = pre_l_tap.max(pre_r_tap) + 2;
        Self {
            lines: std::array::from_fn(|index| {
                HallLine::new(
                    HALL_DELAYS[index],
                    HALL_RATES_HZ[index],
                    HALL_DEPTH[index],
                    HALL_PHASE[index],
                    sample_rate,
                )
            }),
            gains: [0.0; 6],
            pre_l: vec![0.0; pre_len],
            pre_r: vec![0.0; pre_len],
            pre_index: 0,
            pre_l_tap,
            pre_r_tap,
            diffuse_l: HallAllpass::new(((430.0 * scale) as usize).max(8), 0.42),
            diffuse_r: HallAllpass::new(((470.0 * scale) as usize).max(8), 0.42),
            hp_l: 0.0,
            hp_r: 0.0,
            hp_prev_l: 0.0,
            hp_prev_r: 0.0,
            hp_a: (-std::f32::consts::TAU * 80.0 / sample_rate).exp(),
            damp_coeff: 0.5,
            smooth_a: 1.0 - (-1.0 / (0.030 * sample_rate)).exp(),
            bass_a: 1.0 - (-std::f32::consts::TAU * 100.0 / sample_rate).exp(),
            sample_rate,
            primed: false,
            active: false,
        }
    }

    pub(crate) fn process(
        &mut self,
        input_left: f32,
        input_right: f32,
        params: ReverbParams,
    ) -> (f32, f32) {
        let (left, right) = (finite(input_left), finite(input_right));
        if !self.active {
            if left == 0.0 && right == 0.0 {
                return (0.0, 0.0);
            }
            self.active = true;
        }
        self.follow(params);
        let (hp_l, hp_r) = self.highpass(left, right);
        let (pre_l, pre_r) = self.predelay(hp_l, hp_r);
        let input = [
            pre_l,
            pre_r,
            0.65 * pre_l + 0.35 * pre_r,
            0.35 * pre_l + 0.65 * pre_r,
            0.5 * (pre_l + pre_r),
            0.5 * (pre_l + pre_r),
        ];
        let mut colored = [0.0_f32; 6];
        for (line, sample) in self.lines.iter_mut().zip(colored.iter_mut()) {
            let delayed = line.read();
            line.lowpass += self.damp_coeff * (delayed - line.lowpass);
            if line.lowpass.abs() < 1e-20 {
                line.lowpass = 0.0;
            }
            *sample = line.lowpass;
        }
        let mixed = householder6(colored);
        for (index, line) in self.lines.iter_mut().enumerate() {
            let sample = finite(input[index] * HALL_INPUT_GAIN + mixed[index] * self.gains[index]);
            line.bass += self.bass_a * (sample - line.bass);
            line.write(finite(sample - line.bass * 0.40));
        }
        let fdn_l = self.diffuse_l.process(mixed[0] + mixed[2] + mixed[4]);
        let fdn_r = self.diffuse_r.process(mixed[1] + mixed[3] + mixed[5]);
        (finite(fdn_l * HALL_WET_GAIN), finite(fdn_r * HALL_WET_GAIN))
    }

    // Module retirement drops the processor. Tests still clear it in place.
    #[allow(dead_code)]
    pub(crate) fn clear_chunk(&mut self, cursor: &mut usize, samples: usize) -> bool {
        let total = self
            .lines
            .iter()
            .map(|line| line.buffer.len())
            .sum::<usize>()
            + self.pre_l.len()
            + self.pre_r.len()
            + self.diffuse_l.buffer.len()
            + self.diffuse_r.buffer.len();
        let mut skip = *cursor;
        let mut remaining = samples;
        for line in &mut self.lines {
            clear_buffer_chunk(&mut line.buffer, &mut skip, &mut remaining);
        }
        clear_buffer_chunk(&mut self.pre_l, &mut skip, &mut remaining);
        clear_buffer_chunk(&mut self.pre_r, &mut skip, &mut remaining);
        clear_buffer_chunk(&mut self.diffuse_l.buffer, &mut skip, &mut remaining);
        clear_buffer_chunk(&mut self.diffuse_r.buffer, &mut skip, &mut remaining);
        *cursor = cursor.saturating_add(samples - remaining).min(total);
        if *cursor < total {
            return false;
        }
        for line in &mut self.lines {
            line.write = 0;
            line.lowpass = 0.0;
            line.bass = 0.0;
        }
        self.pre_index = 0;
        self.diffuse_l.index = 0;
        self.diffuse_r.index = 0;
        self.hp_l = 0.0;
        self.hp_r = 0.0;
        self.hp_prev_l = 0.0;
        self.hp_prev_r = 0.0;
        self.primed = false;
        self.active = false;
        true
    }

    fn follow(&mut self, params: ReverbParams) {
        let size = params.room_size.clamp(0.0, 1.0);
        let damp = params.damp.clamp(0.0, 1.0);
        let rt60 = 1.4 + size * 6.4;
        let fc = 10_500.0 + (2_200.0 - 10_500.0) * damp;
        let coeff = 1.0 - (-std::f32::consts::TAU * fc / self.sample_rate.max(1.0)).exp();
        if !self.primed {
            self.damp_coeff = coeff;
            for (gain, line) in self.gains.iter_mut().zip(&self.lines) {
                *gain = loop_gain(line.nominal as usize, self.sample_rate, rt60).min(0.985);
            }
            self.primed = true;
            return;
        }
        self.damp_coeff += self.smooth_a * (coeff - self.damp_coeff);
        for (gain, line) in self.gains.iter_mut().zip(&self.lines) {
            let target = loop_gain(line.nominal as usize, self.sample_rate, rt60).min(0.985);
            *gain += self.smooth_a * (target - *gain);
        }
    }

    fn highpass(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.hp_l = self.hp_a * (self.hp_l + left - self.hp_prev_l);
        self.hp_r = self.hp_a * (self.hp_r + right - self.hp_prev_r);
        self.hp_prev_l = left;
        self.hp_prev_r = right;
        (self.hp_l, self.hp_r)
    }

    fn predelay(&mut self, left: f32, right: f32) -> (f32, f32) {
        let len = self.pre_l.len();
        let read = |buffer: &[f32], tap: usize| buffer[(self.pre_index + len - tap % len) % len];
        let out = (
            read(&self.pre_l, self.pre_l_tap),
            read(&self.pre_r, self.pre_r_tap),
        );
        self.pre_l[self.pre_index] = left;
        self.pre_r[self.pre_index] = right;
        self.pre_index += 1;
        if self.pre_index >= len {
            self.pre_index = 0;
        }
        out
    }
}

fn loop_gain(delay_samples: usize, sample_rate: f32, rt60: f32) -> f32 {
    let seconds = delay_samples as f32 / sample_rate.max(1.0);
    (-6.907755 * seconds / rt60.max(0.05))
        .exp()
        .clamp(0.0, 0.985)
}

fn householder6(sample: [f32; 6]) -> [f32; 6] {
    let sum = sample.iter().sum::<f32>() / 3.0;
    sample.map(|value| value - sum)
}

fn finite(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_clear_returns_used_reverb_to_exact_silence() {
        let mut reverb = Freeverb::new(44_100.0);
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

    fn factory() -> ReverbParams {
        ReverbParams {
            room_size: 0.72,
            damp: 0.45,
        }
    }

    fn energy(samples: &[(f32, f32)]) -> f64 {
        samples
            .iter()
            .map(|(left, right)| f64::from(left * left + right * right))
            .sum()
    }

    fn render(rate: f32, params: ReverbParams, seconds: f32) -> Vec<(f32, f32)> {
        let mut hall = ModulatedHall::new(rate);
        let frames = (seconds * rate) as usize;
        (0..frames)
            .map(|frame| {
                let input = if frame == 0 { 1.0 } else { 0.0 };
                let sample = hall.process(input, input, params);
                assert!(sample.0.is_finite() && sample.1.is_finite());
                sample
            })
            .collect()
    }

    #[test]
    fn hall_clears_back_to_exact_silence() {
        let mut hall = ModulatedHall::new(48_000.0);
        for _ in 0..3_000 {
            hall.process(0.3, -0.2, factory());
        }
        let mut cursor = 0;
        while !hall.clear_chunk(&mut cursor, 128) {}
        assert_eq!(hall.process(0.0, 0.0, factory()), (0.0, 0.0));
    }

    #[test]
    fn hall_impulse_stays_finite_and_still_has_a_tail_at_two_seconds() {
        for rate in [44_100.0, 48_000.0] {
            let tail = render(rate, factory(), 2.5);
            let early = energy(&tail[..(0.5 * rate) as usize]);
            let late = energy(&tail[(1.8 * rate) as usize..(2.4 * rate) as usize]);
            assert!(
                early > 1e-8 && late.is_finite(),
                "{rate}: silent or non-finite"
            );
            // A multi-second decay is quieter than its onset and still present.
            // An unmodulated short room is already gone in this window.
            assert!(late > early * 1e-4, "{rate}: tail died like a small room");
            assert!(late < early, "{rate}: tail grew without bound");
            let mono: f64 = tail
                .iter()
                .map(|(left, right)| {
                    let sum = left + right;
                    f64::from(sum * sum)
                })
                .sum();
            assert!(mono > energy(&tail) * 0.25, "{rate}: mono sum cancelled");
        }
    }

    #[test]
    fn hall_full_size_stays_bounded_and_shorter_than_a_small_room_setting() {
        let long = render(
            44_100.0,
            ReverbParams {
                room_size: 1.0,
                damp: 0.15,
            },
            3.0,
        );
        let short = render(
            44_100.0,
            ReverbParams {
                room_size: 0.0,
                damp: 1.0,
            },
            3.0,
        );
        assert!(
            long.iter()
                .all(|(left, right)| left.abs() < 4.0 && right.abs() < 4.0)
        );
        let late = |tail: &[(f32, f32)]| energy(&tail[(2.2 * 44_100.0) as usize..]);
        assert!(late(&long) > late(&short) * 2.0);
    }

    #[test]
    fn hall_modulation_smears_the_static_comb_peak() {
        let mono = |tail: Vec<(f32, f32)>| -> Vec<f32> {
            tail.into_iter()
                .map(|(left, right)| 0.5 * (left + right))
                .collect()
        };
        let correlation = |series: &[f32], lag: usize| {
            let mut num = 0.0f64;
            let mut left = 0.0f64;
            let mut right = 0.0f64;
            for index in lag..series.len() {
                let a = f64::from(series[index]);
                let b = f64::from(series[index - lag]);
                num += a * b;
                left += a * a;
                right += b * b;
            }
            num / (left.sqrt() * right.sqrt()).max(1e-12)
        };
        let freeverb = {
            let mut reverb = Freeverb::new(44_100.0);
            mono(
                (0..(44_100.0 * 1.6) as usize)
                    .map(|frame| {
                        let input = if frame == 0 { 1.0 } else { 0.0 };
                        reverb.process(input, input, factory())
                    })
                    .collect(),
            )
        };
        let hall = mono(render(44_100.0, factory(), 1.6));
        let start = (0.35 * 44_100.0) as usize;
        let free_peak = correlation(&freeverb[start..], 1557);
        let hall_peak = correlation(&hall[start..], 1201);
        assert!(
            hall_peak < free_peak,
            "hall correlation {hall_peak} was not below freeverb {free_peak}"
        );
    }

    #[test]
    #[ignore = "writes an impulse tail when NOOISE_REVERB_EXPORT is set"]
    fn export_hall_impulse() {
        let dir = std::env::var("NOOISE_REVERB_EXPORT").expect("NOOISE_REVERB_EXPORT");
        let rate = 44_100.0;
        let frames = (3.0 * rate) as usize;
        let mut hall = ModulatedHall::new(rate);
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate as u32,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::create(format!("{dir}/hall-impulse.wav"), spec).expect("wav");
        for frame in 0..frames {
            let (left, right) = if frame == 0 { (0.8, 0.2) } else { (0.0, 0.0) };
            let (out_l, out_r) = hall.process(left, right, factory());
            writer
                .write_sample((out_l.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                .unwrap();
            writer
                .write_sample((out_r.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
    }
}
