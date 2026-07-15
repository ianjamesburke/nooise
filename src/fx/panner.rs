pub(crate) struct StereoPanner;

impl StereoPanner {
    /// Constant-power gain pair for a fixed pan position. Voices whose pan
    /// never changes after construction compute this once and multiply per
    /// sample instead of paying the two transcendentals every sample.
    pub(crate) fn gains(pan: f32) -> (f32, f32) {
        let normalized = ((pan.clamp(-1.0, 1.0) + 1.0) * 0.5).clamp(0.0, 1.0);
        let angle = normalized * std::f32::consts::FRAC_PI_2;
        (angle.cos(), angle.sin())
    }

    pub(crate) fn equal_power(mono: f32, pan: f32) -> (f32, f32) {
        let (left, right) = Self::gains(pan);
        (mono * left, mono * right)
    }
}
