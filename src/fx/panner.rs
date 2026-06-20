pub(crate) struct StereoPanner;

impl StereoPanner {
    pub(crate) fn equal_power(mono: f32, pan: f32) -> (f32, f32) {
        let normalized = ((pan.clamp(-1.0, 1.0) + 1.0) * 0.5).clamp(0.0, 1.0);
        let angle = normalized * std::f32::consts::FRAC_PI_2;
        (mono * angle.cos(), mono * angle.sin())
    }
}
