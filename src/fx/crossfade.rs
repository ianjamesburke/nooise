//! Insert-style dry/wet crossfading, and the holder that fades a replaced
//! processor out instead of cutting it.

/// Linear crossfade from `dry` to `wet`; `amount` 0.0 is an exact dry pass.
pub(crate) fn mix(dry: f32, wet: f32, amount: f32) -> f32 {
    dry + (wet - dry) * amount
}

pub(crate) fn mix_stereo(dry: (f32, f32), wet: (f32, f32), amount: f32) -> (f32, f32) {
    (mix(dry.0, wet.0, amount), mix(dry.1, wet.1, amount))
}

/// Something a live signal path no longer wants, kept running while its
/// contribution walks from full weight down to nothing over a fixed number of
/// samples. The owner keeps running `inner` on the live input, blends its
/// output in at [`Outgoing::advance`]'s weight, and drops the holder once
/// [`Outgoing::is_done`].
pub(crate) struct Outgoing<T> {
    pub(crate) inner: T,
    /// Weight of the outgoing output, walking 1.0 down to 0.0.
    weight: f32,
    step: f32,
}

impl<T> Outgoing<T> {
    pub(crate) fn start(inner: T, fade_samples: f32) -> Self {
        Self {
            inner,
            weight: 1.0,
            step: 1.0 / fade_samples.max(1.0),
        }
    }

    /// The weight to blend this sample at, then steps toward silence.
    pub(crate) fn advance(&mut self) -> f32 {
        let weight = self.weight;
        self.weight -= self.step;
        weight
    }

    pub(crate) fn is_done(&self) -> bool {
        self.weight <= 0.0
    }
}
