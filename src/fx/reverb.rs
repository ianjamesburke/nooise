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

/// Per-call character, matching the `DelayParams` convention. Comb/allpass
/// buffer lengths stay construction-time because they depend only on the
/// sample rate; Size and Damping are plain coefficients the caller is free to
/// move every sample.
#[derive(Clone, Copy)]
pub(crate) struct ReverbParams {
    pub(crate) room_size: f32,
    pub(crate) damp: f32,
}
