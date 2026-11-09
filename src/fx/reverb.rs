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

/// Short, dark room used by module slots.
///
/// Six damped feedback delays mixed by a Householder matrix (the lossless
/// feedback matrix from the FDN literature), a pair of low-coefficient
/// allpasses so the tail is a wash rather than a few echoes, and quiet
/// early taps. Delay lengths stay fixed: modulation belongs to the hall
/// experiment. A fixed input high-pass keeps the pad's fundamental out of
/// the loops, and the output low-pass plus the Damping control keep the
/// decay dark. Size still maps to decay time, on a room scale. Bloom keeps
/// [`Freeverb`].
pub(crate) struct DarkRoom {
    lines: [RoomLine; 6],
    gains: [f32; 6],
    diffuse_l: [RoomAllpass; 2],
    diffuse_r: [RoomAllpass; 2],
    early: Vec<f32>,
    early_index: usize,
    taps_l: [usize; 4],
    taps_r: [usize; 4],
    hp_l: f32,
    hp_r: f32,
    hp_prev_l: f32,
    hp_prev_r: f32,
    hp_a: f32,
    out_l: f32,
    out_r: f32,
    out_a: f32,
    damp_coeff: f32,
    smooth_a: f32,
    sample_rate: f32,
    primed: bool,
    active: bool,
}

struct RoomLine {
    buffer: Vec<f32>,
    index: usize,
    lowpass: f32,
    bass: f32,
}

struct RoomAllpass {
    buffer: Vec<f32>,
    index: usize,
    feedback: f32,
}

impl RoomAllpass {
    fn new(spec: (usize, f32), scale: f32, offset: usize) -> Self {
        let len = (((spec.0 + offset) as f32 * scale).round() as usize).max(4);
        Self {
            buffer: vec![0.0; len],
            index: 0,
            feedback: spec.1,
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

/// Delays at 44.1 kHz, mutually incommensurate, about 17–42 ms.
const ROOM_DELAYS: [usize; 6] = [743, 887, 1097, 1361, 1601, 1861];
const ROOM_ALLPASS: [(usize, f32); 2] = [(211, 0.30), (347, 0.24)];
/// Early taps, milliseconds. Quiet and sparse on purpose: a cluster of
/// allpasses here is what rings on a transient.
const ROOM_TAPS_MS_L: [f32; 4] = [7.0, 13.0, 19.0, 29.0];
const ROOM_TAPS_MS_R: [f32; 4] = [9.0, 15.0, 23.0, 31.0];
const ROOM_TAP_GAINS: [f32; 4] = [0.08, 0.06, 0.045, 0.03];
const ROOM_INPUT_GAIN: f32 = 0.40;
/// The Householder mix and the dark filters sit well below Freeverb's
/// eight-comb sum. This scalar puts factory Amount 40% back in that room's
/// loudness neighborhood. It is not a per-file normalize.
const ROOM_WET_GAIN: f32 = 2.80;
const ROOM_HIGHPASS_HZ: f32 = 140.0;
const ROOM_OUTPUT_LOWPASS_HZ: f32 = 2_800.0;

impl DarkRoom {
    pub(crate) fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / 44_100.0;
        let taps =
            |times: [f32; 4]| times.map(|ms| ((ms * 0.001 * sample_rate).round() as usize).max(1));
        let taps_l = taps(ROOM_TAPS_MS_L);
        let taps_r = taps(ROOM_TAPS_MS_R);
        let early_len = taps_l
            .into_iter()
            .chain(taps_r)
            .max()
            .unwrap_or(1)
            .saturating_add(2);
        Self {
            lines: std::array::from_fn(|index| {
                let len = ((ROOM_DELAYS[index] as f32 * scale).round() as usize).max(8);
                RoomLine {
                    buffer: vec![0.0; len],
                    index: 0,
                    lowpass: 0.0,
                    bass: 0.0,
                }
            }),
            gains: [0.0; 6],
            diffuse_l: std::array::from_fn(|index| RoomAllpass::new(ROOM_ALLPASS[index], scale, 0)),
            diffuse_r: std::array::from_fn(|index| {
                RoomAllpass::new(ROOM_ALLPASS[index], scale, 19)
            }),
            early: vec![0.0; early_len],
            early_index: 0,
            taps_l,
            taps_r,
            hp_l: 0.0,
            hp_r: 0.0,
            hp_prev_l: 0.0,
            hp_prev_r: 0.0,
            hp_a: (-std::f32::consts::TAU * ROOM_HIGHPASS_HZ / sample_rate).exp(),
            out_l: 0.0,
            out_r: 0.0,
            out_a: 1.0 - (-std::f32::consts::TAU * ROOM_OUTPUT_LOWPASS_HZ / sample_rate).exp(),
            damp_coeff: 0.5,
            smooth_a: 1.0 - (-1.0 / (0.025 * sample_rate)).exp(),
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
        let (early_l, early_r) = self.early((hp_l + hp_r) * 0.5);
        let input = [
            hp_l,
            hp_r,
            0.7 * hp_l + 0.3 * hp_r,
            0.3 * hp_l + 0.7 * hp_r,
            0.5 * (hp_l + hp_r),
            0.5 * (hp_l + hp_r),
        ];
        let mut colored = [0.0_f32; 6];
        for (line, sample) in self.lines.iter_mut().zip(colored.iter_mut()) {
            let delayed = line.buffer[line.index];
            line.lowpass += self.damp_coeff * (delayed - line.lowpass);
            if line.lowpass.abs() < 1e-20 {
                line.lowpass = 0.0;
            }
            *sample = line.lowpass;
        }
        let mixed = householder6(colored);
        let bass_a = 1.0 - (-std::f32::consts::TAU * 120.0 / self.sample_rate.max(1.0)).exp();
        for (index, line) in self.lines.iter_mut().enumerate() {
            let sample = finite(input[index] * ROOM_INPUT_GAIN + mixed[index] * self.gains[index]);
            line.bass += bass_a * (sample - line.bass);
            // Keep some warmth, but don't let the fundamental recirculate at
            // the same decay as the mids.
            let sample = finite(sample - line.bass * 0.72);
            line.buffer[line.index] = sample;
            line.index += 1;
            if line.index >= line.buffer.len() {
                line.index = 0;
            }
        }
        let mut fdn_l = mixed[0] + mixed[2] + mixed[4];
        let mut fdn_r = mixed[1] + mixed[3] + mixed[5];
        for allpass in &mut self.diffuse_l {
            fdn_l = allpass.process(fdn_l);
        }
        for allpass in &mut self.diffuse_r {
            fdn_r = allpass.process(fdn_r);
        }
        self.out_l += self.out_a * ((early_l + fdn_l) - self.out_l);
        self.out_r += self.out_a * ((early_r + fdn_r) - self.out_r);
        (
            finite(self.out_l * ROOM_WET_GAIN),
            finite(self.out_r * ROOM_WET_GAIN),
        )
    }

    // Module retirement drops the processor. Tests still clear it in place.
    #[allow(dead_code)]
    pub(crate) fn clear_chunk(&mut self, cursor: &mut usize, samples: usize) -> bool {
        let total = self
            .lines
            .iter()
            .map(|line| line.buffer.len())
            .sum::<usize>()
            + self
                .diffuse_l
                .iter()
                .chain(&self.diffuse_r)
                .map(|allpass| allpass.buffer.len())
                .sum::<usize>()
            + self.early.len();
        let mut skip = *cursor;
        let mut remaining = samples;
        for line in &mut self.lines {
            clear_buffer_chunk(&mut line.buffer, &mut skip, &mut remaining);
        }
        for allpass in self.diffuse_l.iter_mut().chain(&mut self.diffuse_r) {
            clear_buffer_chunk(&mut allpass.buffer, &mut skip, &mut remaining);
        }
        clear_buffer_chunk(&mut self.early, &mut skip, &mut remaining);
        *cursor = cursor.saturating_add(samples - remaining).min(total);
        if *cursor < total {
            return false;
        }
        for line in &mut self.lines {
            line.index = 0;
            line.lowpass = 0.0;
            line.bass = 0.0;
        }
        for allpass in self.diffuse_l.iter_mut().chain(&mut self.diffuse_r) {
            allpass.index = 0;
        }
        self.early_index = 0;
        self.hp_l = 0.0;
        self.hp_r = 0.0;
        self.hp_prev_l = 0.0;
        self.hp_prev_r = 0.0;
        self.out_l = 0.0;
        self.out_r = 0.0;
        self.primed = false;
        self.active = false;
        true
    }

    fn follow(&mut self, params: ReverbParams) {
        let size = params.room_size.clamp(0.0, 1.0);
        let damp = params.damp.clamp(0.0, 1.0);
        let rt60 = 0.28 + size * 1.05;
        let fc = 6_500.0 + (900.0 - 6_500.0) * damp;
        let coeff = 1.0 - (-std::f32::consts::TAU * fc / self.sample_rate.max(1.0)).exp();
        if !self.primed {
            self.damp_coeff = coeff;
            for (gain, line) in self.gains.iter_mut().zip(&self.lines) {
                *gain = loop_gain(line.buffer.len(), self.sample_rate, rt60);
            }
            self.primed = true;
            return;
        }
        self.damp_coeff += self.smooth_a * (coeff - self.damp_coeff);
        for (gain, line) in self.gains.iter_mut().zip(&self.lines) {
            let target = loop_gain(line.buffer.len(), self.sample_rate, rt60);
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

    fn early(&mut self, input: f32) -> (f32, f32) {
        self.early[self.early_index] = input;
        let read = |tap: usize| {
            let len = self.early.len();
            self.early[(self.early_index + len - tap % len) % len]
        };
        let mut left = 0.0;
        let mut right = 0.0;
        for (index, gain) in ROOM_TAP_GAINS.iter().enumerate() {
            left += read(self.taps_l[index]) * gain;
            right += read(self.taps_r[index]) * gain;
        }
        self.early_index += 1;
        if self.early_index >= self.early.len() {
            self.early_index = 0;
        }
        (left, right)
    }
}

fn loop_gain(delay_samples: usize, sample_rate: f32, rt60: f32) -> f32 {
    let seconds = delay_samples as f32 / sample_rate.max(1.0);
    (-6.907755 * seconds / rt60.max(0.05))
        .exp()
        .clamp(0.0, 0.97)
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
        samples.iter().map(|(l, r)| f64::from(l * l + r * r)).sum()
    }

    fn render_impulse(rate: f32, params: ReverbParams, seconds: f32) -> Vec<(f32, f32)> {
        let mut room = DarkRoom::new(rate);
        let frames = (seconds * rate) as usize;
        (0..frames)
            .map(|frame| {
                let input = if frame == 0 { 1.0 } else { 0.0 };
                let sample = room.process(input, input, params);
                assert!(sample.0.is_finite() && sample.1.is_finite());
                sample
            })
            .collect()
    }

    #[test]
    #[ignore = "writes impulse tails when NOOISE_REVERB_EXPORT is set"]
    fn export_reverb_impulse() {
        let dir = std::env::var("NOOISE_REVERB_EXPORT").expect("NOOISE_REVERB_EXPORT");
        let rate = 44_100.0;
        let frames = (2.0 * rate) as usize;
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate as u32,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        for (name, samples) in [
            ("freeverb", {
                let mut reverb = Freeverb::new(rate);
                (0..frames)
                    .map(|frame| {
                        let input = if frame == 0 { 0.8 } else { 0.0 };
                        reverb.process(input, 0.2, factory())
                    })
                    .collect::<Vec<_>>()
            }),
            ("dark-room", {
                let mut room = DarkRoom::new(rate);
                (0..frames)
                    .map(|frame| {
                        let (left, right) = if frame == 0 { (0.8, 0.2) } else { (0.0, 0.0) };
                        room.process(left, right, factory())
                    })
                    .collect::<Vec<_>>()
            }),
        ] {
            let mut writer = hound::WavWriter::create(format!("{dir}/{name}-impulse.wav"), spec)
                .expect("impulse wav");
            for (left, right) in samples {
                writer
                    .write_sample((left.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .unwrap();
                writer
                    .write_sample((right.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                    .unwrap();
            }
            writer.finalize().unwrap();
        }
    }

    #[test]
    fn dark_room_clears_back_to_exact_silence() {
        let mut room = DarkRoom::new(48_000.0);
        for _ in 0..2_000 {
            room.process(0.4, -0.2, factory());
        }
        let mut cursor = 0;
        while !room.clear_chunk(&mut cursor, 64) {}
        assert_eq!(room.process(0.0, 0.0, factory()), (0.0, 0.0));
    }

    #[test]
    fn dark_room_impulse_decays_at_both_sample_rates() {
        for rate in [44_100.0, 48_000.0] {
            let tail = render_impulse(rate, factory(), 2.5);
            let early = energy(&tail[..(0.15 * rate) as usize]);
            let late = energy(&tail[(2.0 * rate) as usize..]);
            assert!(early > 1e-6, "{rate}: silent");
            assert!(
                late < early * 0.05,
                "{rate}: late energy {late} did not fall under early {early}"
            );
            let stereo = energy(&tail);
            let mono = tail
                .iter()
                .map(|(l, r)| {
                    let m = l + r;
                    f64::from(m * m)
                })
                .sum::<f64>();
            assert!(
                mono > stereo * 0.35,
                "{rate}: mono sum lost the tail ({mono} vs {stereo})"
            );
        }
    }

    #[test]
    fn dark_room_full_size_stays_bounded_and_size_changes_the_tail() {
        let long = render_impulse(
            44_100.0,
            ReverbParams {
                room_size: 1.0,
                damp: 0.0,
            },
            3.0,
        );
        let short = render_impulse(
            44_100.0,
            ReverbParams {
                room_size: 0.0,
                damp: 1.0,
            },
            3.0,
        );
        assert!(long.iter().all(|(l, r)| l.abs() < 2.0 && r.abs() < 2.0));
        let late = |tail: &[(f32, f32)]| energy(&tail[(1.2 * 44_100.0) as usize..]);
        assert!(
            late(&long) > late(&short) * 4.0,
            "size and damping did not move the tail"
        );
    }

    #[test]
    fn dark_room_rejects_low_end_more_than_a_midrange_tone() {
        let rms_of = |hz: f32| {
            let mut room = DarkRoom::new(44_100.0);
            let mut sum = 0.0f64;
            let mut count = 0.0f64;
            for frame in 0..(44_100 * 2) {
                let phase = frame as f32 * hz / 44_100.0;
                let input = (phase * std::f32::consts::TAU).sin() * 0.25;
                let (l, r) = room.process(input, input, factory());
                if frame > 44_100 {
                    sum += f64::from(l * l + r * r);
                    count += 1.0;
                }
            }
            (sum / count).sqrt()
        };
        // Several tones, so one comb peak cannot stand in for the band.
        let low = [40.0, 55.0, 70.0].into_iter().map(rms_of).sum::<f64>() / 3.0;
        let mid = [500.0, 800.0, 1_200.0].into_iter().map(rms_of).sum::<f64>() / 3.0;
        assert!(low < mid * 0.6, "low {low} was not quieter than mid {mid}");
    }

    #[test]
    fn dark_room_tail_is_darker_and_less_peaked_than_freeverb() {
        let stats = |render: Vec<(f32, f32)>| {
            let start = (0.08 * 44_100.0) as usize;
            let end = (0.70 * 44_100.0) as usize;
            let slice = &render[start..end];
            let mut peak = 0.0f32;
            let mut sum = 0.0f64;
            let mut diff = 0.0f64;
            let mut prev = 0.0f32;
            for (index, (l, r)) in slice.iter().enumerate() {
                let mono = 0.5 * (l + r);
                peak = peak.max(mono.abs());
                sum += f64::from(mono * mono);
                if index > 0 {
                    let d = mono - prev;
                    diff += f64::from(d * d);
                }
                prev = mono;
            }
            let rms = (sum / slice.len() as f64).sqrt() as f32;
            let hf = (diff / slice.len() as f64).sqrt() as f32;
            (peak / rms.max(1e-8), hf / rms.max(1e-8))
        };
        let freeverb = {
            let mut reverb = Freeverb::new(44_100.0);
            (0..(44_100.0 * 0.7) as usize)
                .map(|frame| {
                    let input = if frame == 0 { 1.0 } else { 0.0 };
                    reverb.process(input, input, factory())
                })
                .collect()
        };
        let room = render_impulse(44_100.0, factory(), 0.7);
        let (free_crest, free_hf) = stats(freeverb);
        let (room_crest, room_hf) = stats(room);
        assert!(
            room_hf < free_hf,
            "room HF ratio {room_hf} was not below freeverb {free_hf}"
        );
        assert!(
            room_crest < free_crest,
            "room crest {room_crest} was not below freeverb {free_crest}"
        );
    }
}
