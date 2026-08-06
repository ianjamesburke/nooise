//! Stateless stereo saturation used by every Drive module slot.

/// Saturate a stereo frame while preserving an exact dry bypass at zero.
pub(crate) fn process(input: (f32, f32), amount: f32) -> (f32, f32) {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= f32::EPSILON {
        return input;
    }
    let drive = |sample: f32| {
        let driven = sample * (1.0 + amount * 8.0);
        driven / (1.0 + driven.abs()) * (1.0 + amount * 0.5)
    };
    (drive(input.0), drive(input.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_an_exact_stereo_bypass() {
        assert_eq!(process((0.4, -0.2), 0.0), (0.4, -0.2));
    }

    #[test]
    fn drive_processes_both_channels_with_the_same_curve() {
        let output = process((0.5, -0.5), 0.75);
        assert_eq!(output.0, -output.1);
        assert_ne!(output.0, 0.5);
    }
}
