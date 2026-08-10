//! Macro routes: a control's independent bipolar amount for each of the four
//! macro sliders.

use crate::fluid::widget::DialScale;
use crate::fluid::{MACRO_COUNT, Taper};

// ============================================================
// Macro routes
// ============================================================

const MACRO_AMOUNT_STEP: f32 = 0.01;

/// One of the four macro sliders' independent amount fields on a route.
/// There is no "target" selection any more: every macro assignment (a
/// regular control's `v` route, or a field macro stacked on an LFO field)
/// holds a bipolar amount for all four macro sliders at once, so a single
/// control can ride several macros simultaneously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MacroField(usize);

impl MacroField {
    /// Every macro slot is a bipolar depth, so they all share one scale.
    pub(crate) const SCALE: DialScale = DialScale::Tapered {
        min: -1.0,
        max: 1.0,
        taper: Taper::Linear,
    };

    pub(crate) const ALL: [MacroField; MACRO_COUNT] = {
        let mut all = [MacroField(0); MACRO_COUNT];
        let mut i = 0;
        while i < MACRO_COUNT {
            all[i] = MacroField(i);
            i += 1;
        }
        all
    };

    pub(crate) fn label(self) -> String {
        format!("macro {}", self.0 + 1)
    }

    fn index(self) -> usize {
        self.0
    }
}

/// Assignment of a control (or a single stacked LFO field) to the macro
/// sliders. Each of the four macro sliders has its own independent bipolar
/// amount in -1..1, applied to the control's full range and summed — a
/// control can ride several macros at once, each set directly, none of them
/// requiring the others to be neutral.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MacroRoute {
    pub(crate) amounts: [f32; MACRO_COUNT],
}

impl Default for MacroRoute {
    fn default() -> Self {
        Self {
            amounts: [0.0; MACRO_COUNT],
        }
    }
}

impl MacroRoute {
    pub(crate) fn is_neutral(self) -> bool {
        self.amounts.iter().all(|a| a.abs() <= f32::EPSILON)
    }

    pub(crate) fn adjust_field(&mut self, field: MacroField, dir: f32) {
        let a = &mut self.amounts[field.index()];
        *a = (*a + dir * MACRO_AMOUNT_STEP).clamp(-1.0, 1.0);
    }

    pub(crate) fn set_field(&mut self, field: MacroField, value: f32) {
        self.amounts[field.index()] = (value / 100.0).clamp(-1.0, 1.0);
    }

    pub(crate) fn reset_field(&mut self, field: MacroField) {
        self.amounts[field.index()] = 0.0;
    }

    pub(crate) fn field_value(self, field: MacroField) -> f32 {
        self.amounts[field.index()]
    }

    pub(crate) fn field_display(self, field: MacroField) -> String {
        format!("{:+.0}%", self.amounts[field.index()] * 100.0)
    }

    /// Compact summary of every non-neutral slot, e.g. "m1 +30%  m3 -50%",
    /// for the closed chip line. "none" when every slot is at zero.
    pub(crate) fn summary(self) -> String {
        let parts: Vec<String> = self
            .amounts
            .iter()
            .enumerate()
            .filter(|(_, a)| a.abs() > f32::EPSILON)
            .map(|(i, a)| format!("m{} {:+.0}%", i + 1, a * 100.0))
            .collect();
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("  ")
        }
    }

    /// Combined bipolar contribution ratio: sum over every macro slider of
    /// this route's amount times that slider's live value, each individually
    /// clamped before summing. Multiplied by the control's range by the
    /// caller. 0.0 when neutral, matching the "no effect" case.
    pub(super) fn combined(self, macro_values: &[f32; MACRO_COUNT]) -> f32 {
        self.amounts
            .iter()
            .zip(macro_values)
            .map(|(a, v)| a.clamp(-1.0, 1.0) * v.clamp(0.0, 1.0))
            .sum()
    }

    /// Morph an optional macro route across a leg transition. Every slot is
    /// a plain bipolar amount, so there's no snap-field split — the whole
    /// route just glides by `tt`, fading in/out toward 0 on the side it's
    /// missing from.
    pub(super) fn morph(
        from: Option<&MacroRoute>,
        to: Option<&MacroRoute>,
        tt: f32,
    ) -> Option<MacroRoute> {
        match (from, to) {
            (Some(f), Some(t)) => Some(MacroRoute {
                amounts: std::array::from_fn(|i| f.amounts[i] + (t.amounts[i] - f.amounts[i]) * tt),
            }),
            (Some(f), None) => Some(MacroRoute {
                amounts: f.amounts.map(|a| a * (1.0 - tt)),
            }),
            (None, Some(t)) => Some(MacroRoute {
                amounts: t.amounts.map(|a| a * tt),
            }),
            (None, None) => None,
        }
    }

    /// Best-case full reach: how far the combined contribution could swing
    /// the control below (negative) and above (positive) base if every macro
    /// slider it rides independently reached its own extreme (1.0). Used by
    /// the reach-shadow marker, not the live value.
    pub(crate) fn swing(self, range: f32) -> (f32, f32) {
        let mut lo = 0.0;
        let mut hi = 0.0;
        for a in self.amounts {
            let a = a.clamp(-1.0, 1.0);
            if a < 0.0 {
                lo += a * range;
            } else {
                hi += a * range;
            }
        }
        (lo, hi)
    }
}
