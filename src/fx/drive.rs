//! Stateless stereo saturation used by every Drive module slot.

/// Saturate a stereo frame while preserving an exact dry bypass at zero.
///
/// The knob's full 0..1 range only ever reaches the old curve's 0..0.5 half —
/// what used to sit at 50% is now the new maximum — so the same slider travel
/// covers half the old drive range at twice the resolution. Output gain is
/// then pulled back down as drive increases to compensate for the loudness a
/// saturator adds by its nature, rather than stacking a boost on top of it.
pub(crate) fn process(input: (f32, f32), amount: f32) -> (f32, f32) {
    let amount = amount.clamp(0.0, 1.0) * 0.5;
    if amount <= f32::EPSILON {
        return input;
    }
    let makeup = 1.0 / (1.0 + amount);
    let drive = |sample: f32| {
        let driven = sample * (1.0 + amount * 8.0);
        driven / (1.0 + driven.abs()) * makeup
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

    #[test]
    fn max_amount_only_reaches_the_old_half_drive_ceiling() {
        let input = 0.6_f32;
        let (out, _) = process((input, 0.0), 1.0);
        let driven = input * (1.0 + 0.5 * 8.0);
        let saturated = driven / (1.0 + driven.abs());
        let makeup = 1.0 / 1.5;
        assert!((out - saturated * makeup).abs() < 1e-6);
    }

    #[test]
    fn volume_compensation_keeps_heavier_drive_no_louder_than_lighter_drive() {
        let light = process((0.8, 0.0), 0.2).0;
        let heavy = process((0.8, 0.0), 1.0).0;
        assert!(heavy.abs() <= light.abs());
    }
}
