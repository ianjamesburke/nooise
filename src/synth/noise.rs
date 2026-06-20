use rand::Rng;

pub(crate) struct WhiteNoise {
    last: f32,
}

impl WhiteNoise {
    pub(crate) fn new() -> Self {
        Self { last: 0.0 }
    }

    pub(crate) fn next_filtered<R: Rng>(&mut self, rng: &mut R, smoothing: f32) -> f32 {
        let white = rng.gen_range(-1.0..1.0);
        self.last += (white - self.last) * smoothing.clamp(0.0, 1.0);
        self.last
    }
}
