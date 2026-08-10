//! Shared slider vocabulary.
//!
//! Every bar the UI draws is a [`Dial`]: a value plus the scale that maps it
//! onto bar position, plus the string to print beside it. Before this module
//! each surface hand-rolled its own ratio arithmetic, so a field's bar could
//! silently disagree with how its value actually moves — envelope times swept
//! 0..512 beats linearly, which buried every musically useful setting in the
//! first 1% of the bar.
//!
//! The scale is the widget's own attribute, so shown position and stored value
//! stay deliberately different things: position sweeps evenly end to end while
//! the value underneath follows the scale's curve.

use super::registry::{Step, Taper, beat_grid_ratio, ordered_step_ratio};

/// How a dial's value maps onto bar position. Mirrors [`Step::ratio`]'s three
/// cases plus an explicit rung ladder, and is the only place a ratio is
/// derived.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DialScale {
    /// Continuous span under a taper. `Taper::Linear` covers plain ranges,
    /// bipolar amounts, and enum positions alike.
    Tapered { min: f32, max: f32, taper: Taper },
    /// Musical beat grid: a 0.125 floor rung, sixteenths above.
    BeatGrid { min: f32, max: f32 },
    /// Explicit ordered rungs, each taking an equal share of the bar, with
    /// values between rungs interpolating inside their segment.
    Rungs(&'static [f32]),
}

impl DialScale {
    pub(crate) const fn tapered(min: f32, max: f32, taper: Taper) -> Self {
        Self::Tapered { min, max, taper }
    }

    pub(crate) const fn linear(min: f32, max: f32) -> Self {
        Self::tapered(min, max, Taper::Linear)
    }

    /// Bipolar amount centred at half throw, for `-1..=1` modulation depths.
    pub(crate) const fn bipolar() -> Self {
        Self::linear(-1.0, 1.0)
    }

    /// Discrete enum position: index 0 sits at the floor, the last variant at
    /// the ceiling. A single-variant enum pins to the floor rather than
    /// dividing by zero.
    pub(crate) fn enumerated(count: usize) -> Self {
        Self::linear(0.0, count.saturating_sub(1).max(1) as f32)
    }

    /// The mapping a registry control already declares.
    pub(crate) fn from_step(min: f32, max: f32, step: Step, taper: Taper) -> Self {
        match step {
            Step::Linear(_) => Self::tapered(min, max, taper),
            Step::PowerOfTwo => Self::tapered(min, max, Taper::Log2),
            Step::BeatGrid => Self::BeatGrid { min, max },
        }
    }

    pub(crate) fn ratio(self, value: f32) -> f32 {
        match self {
            Self::Tapered { min, max, taper } => taper.ratio(value, min, max),
            Self::BeatGrid { min, max } => beat_grid_ratio(value, min, max),
            Self::Rungs(steps) => ordered_step_ratio(value, steps),
        }
    }

    /// Inverse of [`ratio`], defined only where the mapping is invertible.
    /// `BeatGrid` and `Rungs` have no inverse in the crate yet, so callers
    /// that need position-space stepping must use a `Tapered` scale.
    pub(crate) fn value_at(self, ratio: f32) -> Option<f32> {
        match self {
            Self::Tapered { min, max, taper } => Some(taper.value_at(ratio, min, max)),
            Self::BeatGrid { .. } | Self::Rungs(_) => None,
        }
    }

    /// Move `value` by `delta` fractions of the dial's throw and read back the
    /// value that lands on. This is the scale's whole point: a shift means the
    /// same thing wherever it starts, so a modulation depth reads as one
    /// musical amount instead of changing meaning with the base value.
    /// `None` for scales with no inverse.
    pub(crate) fn offset_in_position(self, value: f32, delta: f32) -> Option<f32> {
        self.value_at((self.ratio(value) + delta).clamp(0.0, 1.0))
    }

    /// One press worth of movement in position space, so a tapered dial gets
    /// the same number of steps end to end whatever its range. Returns `None`
    /// for scales that own their own stepping.
    pub(crate) fn step_in_position(
        self,
        value: f32,
        dir: f32,
        steps_per_sweep: f32,
    ) -> Option<f32> {
        self.offset_in_position(value, dir / steps_per_sweep)
    }
}

/// One slider's worth of render state: where the handle sits and what to
/// print beside it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Dial {
    pub(crate) value: f32,
    pub(crate) scale: DialScale,
    pub(crate) display: String,
}

impl Dial {
    pub(crate) fn new(value: f32, scale: DialScale, display: impl Into<String>) -> Self {
        Self {
            value,
            scale,
            display: display.into(),
        }
    }

    pub(crate) fn ratio(&self) -> f32 {
        self.scale.ratio(self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerated_spans_the_bar_and_survives_a_single_variant() {
        let four = DialScale::enumerated(4);
        assert_eq!(four.ratio(0.0), 0.0);
        assert_eq!(four.ratio(3.0), 1.0);
        // A one-variant enum must pin to the floor, not divide by zero.
        assert_eq!(DialScale::enumerated(1).ratio(0.0), 0.0);
    }

    #[test]
    fn bipolar_centres_zero_at_half_throw() {
        let scale = DialScale::bipolar();
        assert_eq!(scale.ratio(0.0), 0.5);
        assert_eq!(scale.ratio(-1.0), 0.0);
        assert_eq!(scale.ratio(1.0), 1.0);
    }

    #[test]
    fn exp_taper_lifts_small_values_off_the_floor_of_a_wide_range() {
        let linear = DialScale::linear(0.0, 512.0);
        let tapered = DialScale::tapered(0.0, 512.0, Taper::Exp(3.0));
        // 4 beats is a musically ordinary envelope time. Linearly it is
        // invisible; tapered it earns real bar.
        assert!(linear.ratio(4.0) < 0.01);
        assert!(tapered.ratio(4.0) > 0.15);
    }

    #[test]
    fn position_stepping_is_even_across_a_tapered_range() {
        let scale = DialScale::tapered(0.0, 512.0, Taper::Exp(3.0));
        // Sweeping from the floor takes exactly the promised press count.
        let mut value = 0.0;
        for _ in 0..48 {
            value = scale
                .step_in_position(value, 1.0, 48.0)
                .expect("a tapered scale steps in position space");
        }
        assert!((value - 512.0).abs() < 0.5, "swept to {value}");
    }

    #[test]
    fn scales_without_an_inverse_decline_position_stepping() {
        assert!(
            DialScale::BeatGrid {
                min: 0.0,
                max: 16.0
            }
            .step_in_position(1.0, 1.0, 48.0)
            .is_none()
        );
    }
}
