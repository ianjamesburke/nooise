use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{ControlKind, FluidControls, SongState, all_specs, decode_song_code};

/// Bars per morph leg, matching the throttled-writer granularity of one leg
/// spanning `bars * 4` beats (4/4).
pub(crate) const DEFAULT_AUTO_BARS: u32 = 64;

/// Ordered share codes for the built-in auto-morph states. Appending a code
/// here scales the loop to more legs with no structural change; the future
/// TOML mixtape loader produces the same `Vec<SongState>` this decodes into.
const AUTO_STATES: &[&str] = &[
    "n1_Tk9PSQEFMS41LjIAAgAAAAAA",
    "n1_Tk9PSQEFMS41LjIAlwIAACMACXBhZC5sZXZlbAAAAAAKcGVyYy5sZXZlbArXIz4Ka2ljay5sZXZlbClcDz4KY2xhcC5sZXZlbClcDz4IYXJwLmdhaW6ZmZk-DG1hc3Rlci5kcml2ZZmZmT4VbWFzdGVyLmNvbXBfdGhyZXNob2xkAAAwwRFtYXN0ZXIuY29tcF9yYXRpbwAAkEANcGVyYy5kZWNheV9tcwAAyEILcGVyYy5maWx0ZXKZmRk_CHBhZC50eXBlAACAPw5wYWQuY2hvcmRfYmFycwAAAEAPcGFkLmNob3JkX2NvdW50AAAAQA9wYWQucHJvZ3Jlc3Npb24AAABBEXBhZC5jaG9yZDJfZGVncmVlAACgQBRwYWQuY2hvcmQyX2V4dGVuc2lvbgAAgD8UcGFkLmNob3JkMl9pbnZlcnNpb24AAABAE2tpY2sucGl0Y2hfZGVjYXlfbXMAAIxCEWtpY2suYW1wX2RlY2F5X21zAABwQgpraWNrLmNsaWNrCtejPApraWNrLmRyaXZlKVwPPwtraWNrLmZpbHRlcilcDz4Ua2ljay5lY2hvX3RpbWVfYmVhdHMAAJA_EHRvbmFsLnN5bnRoX3R5cGUAAMBADHRvbmFsLnBocmFzZQAAgD8MdG9uYWwuYXR0YWNrAAAAAA10b25hbC5yZWxlYXNlmpkZPhB0b25hbC5yYW5kb21uZXNzrkdhPhd0b25hbC5ub3RlX2xlbmd0aF9iZWF0cwAAQD8QdG9uYWwucmV2ZXJiX21peAAAAAALY2xhcC5maWx0ZXKZmRk_DmFycC5yYXRlX2JlYXRzAAAAPgthcnAub2N0YXZlcwAAAEALYXJwLnJlbGVhc2UAAKA_DmFycC5yZXZlcmJfbWl4MzMzPwGMAAAABQQADXBlcmMuZGVjYXlfbXMAAABACtejPAAAAAAAwhjLfgtwZXJjLmZpbHRlcgAAAEEK1yM9AAAAgECDv16xE3BlcmMuaW50ZXJ2YWxfYmVhdHMAAABAj8L1PAAAAAAAviMbWRB0b25hbC5yYXRlX2JlYXRzAAAAQClcjz0GAAAAAGdyy2MAAAAAAAA",
];

/// Decode every `AUTO_STATES` code into a `SongState`. Fatal on the first bad
/// code: a malformed baked-in constant is a bug, not a user-facing error.
pub(crate) fn decode_auto_states() -> Vec<SongState> {
    AUTO_STATES
        .iter()
        .map(|code| {
            decode_song_code(code)
                .unwrap_or_else(|err| panic!("built-in auto-morph state {code:?} failed to decode: {err:?}"))
        })
        .collect()
}

/// Throttle granularity for the morph writer: one 1/8 note, i.e. half a beat.
const MORPH_TICK_BEATS: f64 = 0.5;

/// Config for the slow-evolution morph between song states, published to the
/// audio thread via `ArcSwap<Option<MorphState>>` alongside controls and
/// automation. Live progress is derived from the beat clock, not stored, so
/// it stays deterministic for any future offline render.
pub(crate) struct MorphState {
    endpoints: Vec<FluidControls>,
    bars: u32,
}

impl MorphState {
    pub(crate) fn new(endpoints: Vec<FluidControls>, bars: u32) -> Self {
        assert!(
            !endpoints.is_empty(),
            "auto-morph requires at least one state"
        );
        Self {
            endpoints,
            bars: bars.max(1),
        }
    }

    fn beats_per_leg(&self) -> f64 {
        f64::from(self.bars) * 4.0
    }

    /// (from index, to index, t in [0,1)) for the leg containing `beat`.
    /// Looping A→B→…→A forever falls out of the modulo, correct for any N.
    fn leg_at(&self, beat: f64) -> (usize, usize, f64) {
        let beats_per_leg = self.beats_per_leg();
        let beat = beat.max(0.0);
        let leg_index = (beat / beats_per_leg).floor() as i64;
        let t = (beat - leg_index as f64 * beats_per_leg) / beats_per_leg;
        let n = self.endpoints.len() as i64;
        let from = leg_index.rem_euclid(n) as usize;
        let to = (leg_index + 1).rem_euclid(n) as usize;
        (from, to, t.clamp(0.0, 1.0))
    }

    /// The morphed `FluidControls` at `beat`: continuous params (`Gain`,
    /// `Continuous`) lerp; grid params (`Timing`, `Discrete`) hold `from`
    /// until t=0.5 then jump to `to`, passed through `spec.quantize` so the
    /// result lands on a valid step/grid value.
    pub(crate) fn controls_at(&self, beat: f64) -> FluidControls {
        let (from_idx, to_idx, t) = self.leg_at(beat);
        let from = &self.endpoints[from_idx];
        let to = &self.endpoints[to_idx];
        let mut next = from.clone();
        for spec in all_specs() {
            let from_v = (spec.get)(from);
            let to_v = (spec.get)(to);
            let value = match spec.kind {
                ControlKind::Gain | ControlKind::Continuous => {
                    from_v + (to_v - from_v) * t as f32
                }
                ControlKind::Timing | ControlKind::Discrete => {
                    spec.quantize(if t < 0.5 { from_v } else { to_v })
                }
            };
            (spec.set)(&mut next, value);
        }
        next
    }
}

/// A morph-less `ArcSwap`, shared by every entry point that doesn't run
/// `nooise auto`.
pub(crate) fn no_morph() -> Arc<ArcSwap<Option<MorphState>>> {
    Arc::new(ArcSwap::from_pointee(None))
}

/// Throttled writer driving the morph from the engine's control-reload tick.
/// Recomputes and returns the morphed controls only once per 1/8 note,
/// tracking the last beat it fired on so the audio thread never rewrites the
/// shared controls Arc more often than that.
#[derive(Default)]
pub(crate) struct MorphWriter {
    last_tick_beat: Option<f64>,
}

impl MorphWriter {
    /// `Some(controls)` when a new morph tick is due at `beat`; `None`
    /// otherwise (call site should skip the write).
    pub(crate) fn tick(&mut self, morph: &MorphState, beat: f64) -> Option<FluidControls> {
        let due = match self.last_tick_beat {
            None => true,
            Some(last) => beat - last >= MORPH_TICK_BEATS,
        };
        if !due {
            return None;
        }
        self.last_tick_beat = Some(beat);
        Some(morph.controls_at(beat))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::spec_by_id;

    fn state(bpm: f32) -> FluidControls {
        let mut c = FluidControls::default();
        c.master.bpm = bpm;
        c
    }

    #[test]
    fn auto_states_all_decode_without_error() {
        let states = decode_auto_states();
        assert_eq!(states.len(), AUTO_STATES.len());
    }

    #[test]
    fn leg_math_two_states_wraps_forever() {
        let morph = MorphState::new(vec![state(80.0), state(120.0)], 2);
        // 2 bars/leg * 4 beats = 8 beats per leg.
        assert_eq!(morph.leg_at(0.0), (0, 1, 0.0));
        assert_eq!(morph.leg_at(4.0), (0, 1, 0.5));
        assert_eq!(morph.leg_at(8.0), (1, 0, 0.0));
        assert_eq!(morph.leg_at(12.0), (1, 0, 0.5));
        assert_eq!(morph.leg_at(16.0), (0, 1, 0.0));
    }

    #[test]
    fn leg_math_eight_states_wraps_forever() {
        let endpoints: Vec<FluidControls> = (0..8).map(|i| state(80.0 + i as f32)).collect();
        let morph = MorphState::new(endpoints, 1);
        // 1 bar/leg * 4 beats = 4 beats per leg.
        assert_eq!(morph.leg_at(0.0), (0, 1, 0.0));
        assert_eq!(morph.leg_at(28.0), (7, 0, 0.0));
        assert_eq!(morph.leg_at(30.0), (7, 0, 0.5));
        assert_eq!(morph.leg_at(32.0), (0, 1, 0.0));
    }

    #[test]
    fn leg_math_boundaries_at_t_zero_and_towards_one() {
        let morph = MorphState::new(vec![state(80.0), state(120.0)], 1);
        let (_, _, t_start) = morph.leg_at(0.0);
        assert_eq!(t_start, 0.0);
        let (from, to, t_end) = morph.leg_at(3.999_999);
        assert_eq!((from, to), (0, 1));
        assert!(t_end > 0.99);
    }

    #[test]
    fn gain_param_lerps_linearly() {
        let mut from = FluidControls::default();
        from.pad.level = 0.0;
        let mut to = FluidControls::default();
        to.pad.level = 1.0;
        let morph = MorphState::new(vec![from, to], 1);
        let beats_per_leg = 4.0;
        let controls = morph.controls_at(beats_per_leg * 0.25);
        assert!((controls.pad.level - 0.25).abs() < 1e-4);
    }

    #[test]
    fn discrete_param_holds_then_flips_at_midpoint() {
        let mut from = FluidControls::default();
        from.pad.progression = 0.0;
        let mut to = FluidControls::default();
        to.pad.progression = 3.0;
        let morph = MorphState::new(vec![from, to], 1);
        let beats_per_leg = 4.0;

        let before = morph.controls_at(beats_per_leg * 0.49);
        assert_eq!(before.pad.progression, 0.0);

        let after = morph.controls_at(beats_per_leg * 0.51);
        assert_eq!(after.pad.progression, 3.0);
    }

    #[test]
    fn timing_param_snaps_to_valid_grid_after_jump() {
        let spec = spec_by_id("kick.interval_beats").expect("kick.interval_beats is registered");
        let mut from = FluidControls::default();
        (spec.set)(&mut from, 1.0);
        let mut to = FluidControls::default();
        (spec.set)(&mut to, 0.33); // not on the beat grid
        let morph = MorphState::new(vec![from, to], 1);
        let beats_per_leg = 4.0;

        let after = morph.controls_at(beats_per_leg * 0.75);
        let value = (spec.get)(&after);
        assert_eq!(value, spec.quantize(value), "jumped value must already be grid-snapped");
        assert_ne!(value, 0.33, "raw unsnapped endpoint must not pass through untouched");
    }

    #[test]
    fn writer_throttles_to_one_tick_per_eighth_note() {
        let morph = MorphState::new(vec![state(80.0), state(120.0)], 64);
        let mut writer = MorphWriter::default();

        assert!(writer.tick(&morph, 0.0).is_some(), "first tick always fires");
        assert!(
            writer.tick(&morph, 0.1).is_none(),
            "within the same 1/8 note, no new write"
        );
        assert!(
            writer.tick(&morph, 0.5).is_some(),
            "a full 1/8 note later, a new write is due"
        );
    }
}
