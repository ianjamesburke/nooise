struct Comb {
    buffer: Vec<f32>,
    index: usize,
    feedback: f32,
    filter_store: f32,
    damp1: f32,
    damp2: f32,
}

impl Comb {
    fn new(size: usize, feedback: f32, damp: f32) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            index: 0,
            feedback,
            filter_store: 0.0,
            damp1: damp,
            damp2: 1.0 - damp,
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

#[derive(Clone)]
pub(crate) struct CombState {
    pub(crate) buffer: Vec<f32>,
    pub(crate) index: usize,
    pub(crate) feedback: f32,
    pub(crate) filter_store: f32,
    pub(crate) damp1: f32,
    pub(crate) damp2: f32,
}

#[derive(Clone)]
pub(crate) struct AllPassState {
    pub(crate) buffer: Vec<f32>,
    pub(crate) index: usize,
    pub(crate) feedback: f32,
}

#[derive(Clone)]
pub(crate) struct FreeverbState {
    pub(crate) combs_left: Vec<CombState>,
    pub(crate) combs_right: Vec<CombState>,
    pub(crate) allpasses_left: Vec<AllPassState>,
    pub(crate) allpasses_right: Vec<AllPassState>,
    pub(crate) wet: f32,
    pub(crate) active: bool,
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
    wet: f32,
    /// Sticky: false until the first nonzero input sample. While every input
    /// so far has been zero, every comb/allpass buffer and filter holds exact
    /// zeros and the output is exact silence, so `process` skips all work.
    /// Once audio arrives it stays active so the tail always rings out.
    active: bool,
}

impl Freeverb {
    pub(crate) fn new(sample_rate: f32, room_size: f32, damp: f32, wet: f32) -> Self {
        let scale = sample_rate / 44_100.0;
        let comb_tunings = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        let allpass_tunings = [556, 441, 341, 225];
        let feedback = 0.28 + room_size.clamp(0.0, 1.0) * 0.68;
        let damp = damp.clamp(0.0, 1.0) * 0.4;

        let build_combs = |offset: i32| -> Vec<Comb> {
            comb_tunings
                .iter()
                .map(|size| Comb::new(((*size + offset) as f32 * scale) as usize, feedback, damp))
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
            wet: wet.clamp(0.0, 1.0),
            active: false,
        }
    }

    pub(crate) fn process(&mut self, input_left: f32, input_right: f32) -> (f32, f32) {
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

        (left * self.wet * 0.18, right * self.wet * 0.18)
    }

    pub(crate) fn set_character(&mut self, room_size: f32, damp: f32) {
        let feedback = 0.28 + room_size.clamp(0.0, 1.0) * 0.68;
        let damp1 = damp.clamp(0.0, 1.0) * 0.4;
        for comb in self.combs_left.iter_mut().chain(&mut self.combs_right) {
            comb.feedback = feedback;
            comb.damp1 = damp1;
            comb.damp2 = 1.0 - damp1;
        }
    }

    pub(crate) fn state(&self) -> FreeverbState {
        let comb = |value: &Comb| CombState {
            buffer: value.buffer.clone(),
            index: value.index,
            feedback: value.feedback,
            filter_store: value.filter_store,
            damp1: value.damp1,
            damp2: value.damp2,
        };
        let allpass = |value: &AllPass| AllPassState {
            buffer: value.buffer.clone(),
            index: value.index,
            feedback: value.feedback,
        };
        FreeverbState {
            combs_left: self.combs_left.iter().map(comb).collect(),
            combs_right: self.combs_right.iter().map(comb).collect(),
            allpasses_left: self.allpasses_left.iter().map(allpass).collect(),
            allpasses_right: self.allpasses_right.iter().map(allpass).collect(),
            wet: self.wet,
            active: self.active,
        }
    }

    pub(crate) fn from_state(state: FreeverbState) -> Option<Self> {
        let comb = |value: CombState| {
            (!value.buffer.is_empty()).then(|| Comb {
                index: value.index % value.buffer.len(),
                buffer: value.buffer,
                feedback: value.feedback,
                filter_store: value.filter_store,
                damp1: value.damp1,
                damp2: value.damp2,
            })
        };
        let allpass = |value: AllPassState| {
            (!value.buffer.is_empty()).then(|| AllPass {
                index: value.index % value.buffer.len(),
                buffer: value.buffer,
                feedback: value.feedback,
            })
        };
        Some(Self {
            combs_left: state
                .combs_left
                .into_iter()
                .map(comb)
                .collect::<Option<_>>()?,
            combs_right: state
                .combs_right
                .into_iter()
                .map(comb)
                .collect::<Option<_>>()?,
            allpasses_left: state
                .allpasses_left
                .into_iter()
                .map(allpass)
                .collect::<Option<_>>()?,
            allpasses_right: state
                .allpasses_right
                .into_iter()
                .map(allpass)
                .collect::<Option<_>>()?,
            wet: state.wet.clamp(0.0, 1.0),
            active: state.active,
        })
    }
}
