use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{AutomationState, ControlKind, FluidControls, SongState, all_specs, decode_song_code};
#[cfg(test)]
use super::automation::{ControlAddress, LfoRoute, LfoShape};

/// Bars per morph leg, matching the throttled-writer granularity of one leg
/// spanning `bars * 4` beats (4/4).
pub(crate) const DEFAULT_AUTO_BARS: u32 = 64;

/// Ordered share codes for the built-in auto-morph states. The morph loops
/// through them in order, one per leg, so the list length is the number of legs
/// in a cycle.
///
/// # Adding a morph target
/// 1. In a live session, dial in the full sound you want as a destination.
/// 2. Press `Ctrl+S` to copy its launch line (`nooise n1_…`) to the clipboard.
/// 3. Add a new entry below with just the `n1_…` code (drop the `nooise `
///    prefix), in the position you want it to appear in the cycle. Order is
///    musical, not structural — reorder freely.
///
/// No other code changes are needed: `decode_auto_states` picks up every entry
/// and the morph scheduler scales to any count. Aim for up to 8 targets; the
/// future TOML mixtape loader will replace this array with the same
/// `Vec<SongState>` it decodes into, so keep entries as plain share codes.
const AUTO_STATES: &[&str] = &[
    // 1. Baseline seed — near-default state the cycle opens from.
    "n1_Tk9PSQEFMS41LjIAAgAAAAAA",
    // 2. Driving perc / full progression.
    "n1_Tk9PSQEFMS41LjIAlwIAACMACXBhZC5sZXZlbAAAAAAKcGVyYy5sZXZlbArXIz4Ka2ljay5sZXZlbClcDz4KY2xhcC5sZXZlbClcDz4IYXJwLmdhaW6ZmZk-DG1hc3Rlci5kcml2ZZmZmT4VbWFzdGVyLmNvbXBfdGhyZXNob2xkAAAwwRFtYXN0ZXIuY29tcF9yYXRpbwAAkEANcGVyYy5kZWNheV9tcwAAyEILcGVyYy5maWx0ZXKZmRk_CHBhZC50eXBlAACAPw5wYWQuY2hvcmRfYmFycwAAAEAPcGFkLmNob3JkX2NvdW50AAAAQA9wYWQucHJvZ3Jlc3Npb24AAABBEXBhZC5jaG9yZDJfZGVncmVlAACgQBRwYWQuY2hvcmQyX2V4dGVuc2lvbgAAgD8UcGFkLmNob3JkMl9pbnZlcnNpb24AAABAE2tpY2sucGl0Y2hfZGVjYXlfbXMAAIxCEWtpY2suYW1wX2RlY2F5X21zAABwQgpraWNrLmNsaWNrCtejPApraWNrLmRyaXZlKVwPPwtraWNrLmZpbHRlcilcDz4Ua2ljay5lY2hvX3RpbWVfYmVhdHMAAJA_EHRvbmFsLnN5bnRoX3R5cGUAAMBADHRvbmFsLnBocmFzZQAAgD8MdG9uYWwuYXR0YWNrAAAAAA10b25hbC5yZWxlYXNlmpkZPhB0b25hbC5yYW5kb21uZXNzrkdhPhd0b25hbC5ub3RlX2xlbmd0aF9iZWF0cwAAQD8QdG9uYWwucmV2ZXJiX21peAAAAAALY2xhcC5maWx0ZXKZmRk_DmFycC5yYXRlX2JlYXRzAAAAPgthcnAub2N0YXZlcwAAAEALYXJwLnJlbGVhc2UAAKA_DmFycC5yZXZlcmJfbWl4MzMzPwGMAAAABQQADXBlcmMuZGVjYXlfbXMAAABACtejPAAAAAAAwhjLfgtwZXJjLmZpbHRlcgAAAEEK1yM9AAAAgECDv16xE3BlcmMuaW50ZXJ2YWxfYmVhdHMAAABAj8L1PAAAAAAAviMbWRB0b25hbC5yYXRlX2JlYXRzAAAAQClcjz0GAAAAAGdyy2MAAAAAAAA",
    // 3. Sparse pad-led breakdown.
    "n1_Tk9PSQEFMS41LjIAzAEAABcACXBhZC5sZXZlbFyPQj8KcGVyYy5sZXZlbI_CdT0KbWFzdGVyLmJwbQAAmEITcGVyYy5pbnRlcnZhbF9iZWF0cwAAiEANcGVyYy5kZWNheV9tcwAA8EILcGVyYy5maWx0ZXLhehQ_CHBhZC50eXBlAAAAQA5wYWQuY2hvcmRfYmFycwAAAEAPcGFkLmNob3JkX2NvdW50AACAQA9wYWQucHJvZ3Jlc3Npb24AAABBDnBhZC5yZXZlcmJfbWl44XoUPxBwYWQuc3RlcmVvX3dpZHRoUrgePwpwYWQuZGV0dW5lAAAAAA5wYWQub2N0YXZlX21peJmZGT8PcGFkLmF0dGFja190aW1lAAAAQBBwYWQucmVsZWFzZV90aW1lAAAAQBRwYWQuY2hvcmQxX2V4dGVuc2lvbgAAQEARcGFkLmNob3JkMl9kZWdyZWUAAKBAFHBhZC5jaG9yZDJfZXh0ZW5zaW9uAAAAQBRwYWQuY2hvcmQzX2V4dGVuc2lvbgAAQEARcGFkLmNob3JkNF9kZWdyZWUAAKBAFHBhZC5jaG9yZDRfZXh0ZW5zaW9uAABAQBRwYWQuY2hvcmQ0X2ludmVyc2lvbgAAAEABYgAAAAUDAA5wYWQub2N0YXZlX21peAAAwD9cj0I-AAAAQD_VzSl-C3BlcmMuZmlsdGVyAACAPwrXozwAAACAPoO_XrEKcGVyYy5sZXZlbAAAeEEK16M8AAAAAACZoH_EAAAAAAAA",
    // 4. Full-band build with driving kick/clap and busy arp.
    "n1_Tk9PSQEFMS41LjIAuQIAACcACXBhZC5sZXZlbAAAAAAKcGVyYy5sZXZlbOtROD4Ka2ljay5sZXZlbOtROD4KY2xhcC5sZXZlbClcDz4KYmFzcy5sZXZlbI_C9T0IYXJwLmdhaW4pXI8-C3BlcmMuZmlsdGVyUrgePw1wZXJjLmRlY2F5X21zhEHEQhNwZXJjLmludGVydmFsX2JlYXRzAAAAPwpwZXJjLnN3aW5nj8J1PQ9wYWQuYXR0YWNrX3RpbWUAAABAEHBhZC5yZWxlYXNlX3RpbWUAAABADnBhZC5jaG9yZF9iYXJzAACAPw9wYWQuY2hvcmRfY291bnQAAIBAD3BhZC5wcm9ncmVzc2lvbgAAAEEQcGFkLnN0ZXJlb193aWR0aD0KVz8RcGFkLmNob3JkMl9kZWdyZWUAAEBAEXBhZC5jaG9yZDRfZGVncmVlAACAwBRwYWQuY2hvcmQ0X2ludmVyc2lvbgAAQEALYmFzcy5jdXRvZmZblvdDEGJhc3MuYXR0YWNrX3RpbWUK16M7D2Jhc3MuZGVjYXlfdGltZfLx7z4JYmFzcy50eXBlAACAPwtiYXNzLm9jdGF2ZQAAAAAKYmFzcy5kcml2ZY_CdT4La2ljay5maWx0ZXK4HgU_E2tpY2sucGl0Y2hfZGVjYXlfbXMpthNCEWtpY2suYW1wX2RlY2F5X21zlv-EQw9raWNrLnN0YXJ0X2ZyZXEAADlDCmtpY2suY2xpY2sK16M8CmtpY2suZHJpdmUK16M-EHRvbmFsLnJhbmRvbW5lc3MAAAAAC2NsYXAuZmlsdGVyMzMzPwphcnAuYXR0YWNr5P_5PAlhcnAuZGVjYXkmJ4Q-CGFycC50eXBlAAAAAA5hcnAucmF0ZV9iZWF0cwAAgD4JYXJwLnN3aW5nexSuPg5hcnAucmV2ZXJiX21peAAAAAABlwAAAAUFAAlhcnAuZGVjYXkAAABBCtcjPAQAAAAAlikraQlhcnAuc3dpbmcAAABBzMxMPQQAAAAAdpeKaQ1wZXJjLmRlY2F5X21zAABAQQrXozwAAAAAAMIYy34LcGVyYy5maWx0ZXIAAIBACtcjPQAAAIA_g79esQpwZXJjLmxldmVsAACAPwrXoz0DAABAP5mgf8QAAAAAAAA",
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

// ============================================================
// Morph model
//
// Every leg is HOLD then TRANSITION: the `from` state is held steady for the
// first `HOLD_FRACTION` of the leg, then the states cross over in the final
// stretch. Sections actually sit still instead of fading the whole time.
//
// During the transition each control moves by its own behavior, derived from
// its `ControlKind`:
//
//   Glide (Gain/Continuous) — lerp `from`→`to` across the transition window.
//       Levels glide too, so total audible energy always stays between the two
//       endpoints: a morph can never introduce silence the endpoints don't have.
//
//   Snap (Discrete/Timing)  — never interpolated; hold `from`, then hard-jump.
//       Structural params (progression + chord count/length + arp pattern) all
//       jump together on the transition downbeat ("one") as one atomic event.
//       Every other grid param staggers in at 8-bar offsets after it, in
//       registry order, so similar sections hard-switch rather than crossfade.
// ============================================================

/// Fraction of a leg spent holding the `from` state before the transition. The
/// transition gets the remaining third, so transition length ≈ half the hold.
const HOLD_FRACTION: f64 = 2.0 / 3.0;

/// Spacing between successive non-structural grid hard-switches, in bars.
const STAGGER_STEP_BARS: f64 = 8.0;

/// Control ids that hard-jump together on the transition downbeat, never
/// interpolated and never staggered against each other. A progression change
/// and the chord shape it implies must arrive as one musical event.
const STRUCTURAL_SNAP_IDS: &[&str] = &[
    "pad.progression",
    "pad.chord_count",
    "pad.chord_bars",
    "arp.pattern",
];

fn is_structural(spec_id: &str) -> bool {
    STRUCTURAL_SNAP_IDS.contains(&spec_id)
}

/// Crude "how different do two states sound" metric: the summed absolute
/// difference of every performing element's level/gain. Deliberately simple —
/// used only to pick which built-in state a live auto-toggle heads toward first.
fn level_distance(a: &FluidControls, b: &FluidControls) -> f32 {
    (a.pad.level - b.pad.level).abs()
        + (a.perc.level - b.perc.level).abs()
        + (a.kick.level - b.kick.level).abs()
        + (a.tonal.level - b.tonal.level).abs()
        + (a.clap.level - b.clap.level).abs()
        + (a.bass.level - b.bass.level).abs()
        + (a.arp.gain - b.arp.gain).abs()
}

/// (spec index into `all_specs()` order, jump offset in bars from the
/// transition downbeat) for every changed non-structural grid param on a leg,
/// staggered in registry order. Structural and glide params aren't listed.
fn stepped_offsets(from: &FluidControls, to: &FluidControls) -> Vec<(usize, f64)> {
    all_specs()
        .enumerate()
        .filter(|(_, spec)| matches!(spec.kind, ControlKind::Discrete | ControlKind::Timing))
        .filter(|(_, spec)| !is_structural(spec.id))
        .filter(|(_, spec)| (spec.get)(from) != (spec.get)(to))
        .enumerate()
        .map(|(order, (index, _))| (index, (order + 1) as f64 * STAGGER_STEP_BARS))
        .collect()
}

/// Config for the slow-evolution morph between song states, published to the
/// audio thread via `ArcSwap<Option<MorphState>>` alongside controls and
/// automation. Live progress is derived from the beat clock, not stored, so
/// it stays deterministic for any future offline render.
pub(crate) struct MorphState {
    endpoints: Vec<SongState>,
    bars: u32,
    /// Staggered hard-switch offsets for leg i -> i+1 (mod n), precomputed once.
    stepped: Vec<Vec<(usize, f64)>>,
    /// Engine beat the morph timeline is anchored to. Zero for the baked-in
    /// loop (which starts at beat 0); set to the toggle beat for a live start
    /// so the first leg begins from the current state, not mid-loop.
    origin_beat: f64,
}

impl MorphState {
    pub(crate) fn new(endpoints: Vec<SongState>, bars: u32) -> Self {
        assert!(
            !endpoints.is_empty(),
            "auto-morph requires at least one state"
        );
        let n = endpoints.len();
        let stepped = (0..n)
            .map(|i| stepped_offsets(&endpoints[i].controls, &endpoints[(i + 1) % n].controls))
            .collect();
        Self {
            endpoints,
            bars: bars.max(1),
            stepped,
            origin_beat: 0.0,
        }
    }

    /// Build a morph for a live toggle at `start_beat`: endpoint 0 is the
    /// caller's current controls and automation (LFO/envelope/macro routes),
    /// so nothing changes instantly — the morph just starts moving from where
    /// it already is, taking whatever modulators are live along for the ride
    /// instead of leaving them running unmodified forever. It heads to the
    /// *nearest* built-in state first (by `level_distance`), then loops the
    /// rest.
    pub(crate) fn from_live(
        current: FluidControls,
        current_automation: AutomationState,
        states: Vec<SongState>,
        bars: u32,
        start_beat: f64,
    ) -> Self {
        let nearest = states
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                level_distance(&current, &a.controls).total_cmp(&level_distance(&current, &b.controls))
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        let mut endpoints = Vec::with_capacity(states.len() + 1);
        endpoints.push(SongState {
            controls: current,
            automation: current_automation,
        });
        endpoints.extend(states[nearest..].iter().cloned());
        endpoints.extend(states[..nearest].iter().cloned());
        let mut morph = Self::new(endpoints, bars);
        morph.origin_beat = start_beat.max(0.0);
        morph
    }

    fn beats_per_leg(&self) -> f64 {
        f64::from(self.bars) * 4.0
    }

    /// Beat within a leg at which the hold ends and the transition begins,
    /// snapped down to a bar downbeat so structural changes land on "one".
    fn transition_start_beat(&self) -> f64 {
        (f64::from(self.bars) * HOLD_FRACTION).floor() * 4.0
    }

    /// (from index, to index, t in [0,1)) for the leg containing `beat`.
    /// Looping A→B→…→A forever falls out of the modulo, correct for any N.
    fn leg_at(&self, beat: f64) -> (usize, usize, f64) {
        let beats_per_leg = self.beats_per_leg();
        let beat = (beat - self.origin_beat).max(0.0);
        let leg_index = (beat / beats_per_leg).floor() as i64;
        let t = (beat - leg_index as f64 * beats_per_leg) / beats_per_leg;
        let n = self.endpoints.len() as i64;
        let from = leg_index.rem_euclid(n) as usize;
        let to = (leg_index + 1).rem_euclid(n) as usize;
        (from, to, t.clamp(0.0, 1.0))
    }

    /// The morphed `FluidControls` at `beat`: hold `from`, then glide or
    /// hard-switch each control across the transition window (see the module
    /// comment). At the leg boundary `leg_at` wraps to the next leg's `from`,
    /// which equals this leg's `to`, so the real target lands exactly on "one".
    pub(crate) fn controls_at(&self, beat: f64) -> FluidControls {
        let (from_idx, to_idx, t) = self.leg_at(beat);
        let from = &self.endpoints[from_idx].controls;
        let to = &self.endpoints[to_idx].controls;
        let offsets = &self.stepped[from_idx];
        let beats_per_leg = self.beats_per_leg();
        let t_beat = t * beats_per_leg;
        let transition_start = self.transition_start_beat();
        let transition_beats = (beats_per_leg - transition_start).max(1e-6);

        let mut next = from.clone();
        for (index, spec) in all_specs().enumerate() {
            let from_v = (spec.get)(from);
            let to_v = (spec.get)(to);

            let value = match spec.kind {
                ControlKind::Gain | ControlKind::Continuous => {
                    let tt = ((t_beat - transition_start) / transition_beats).clamp(0.0, 1.0) as f32;
                    from_v + (to_v - from_v) * tt
                }
                ControlKind::Timing | ControlKind::Discrete => {
                    let jump_beat = if is_structural(spec.id) {
                        transition_start
                    } else {
                        match offsets.iter().find(|(i, _)| *i == index) {
                            Some(&(_, offset_bars)) => {
                                (transition_start + offset_bars * 4.0).min(beats_per_leg)
                            }
                            None => transition_start, // unchanged: `from` and `to` are identical
                        }
                    };
                    spec.quantize(if t_beat < jump_beat { from_v } else { to_v })
                }
            };

            (spec.set)(&mut next, value);
        }
        next
    }

    /// The morphed `AutomationState` at `beat`: the `AutomationState`
    /// counterpart to `controls_at`. Every LFO/envelope/macro route snaps its
    /// non-level fields together at the single transition downbeat (no
    /// per-field staggering, unlike `controls_at`'s grid params) while its
    /// level field glides continuously — see `AutomationState::morph` for the
    /// full rationale.
    pub(crate) fn automation_at(&self, beat: f64) -> AutomationState {
        let (from_idx, to_idx, t) = self.leg_at(beat);
        let from = &self.endpoints[from_idx].automation;
        let to = &self.endpoints[to_idx].automation;
        let beats_per_leg = self.beats_per_leg();
        let t_beat = t * beats_per_leg;
        let transition_start = self.transition_start_beat();
        let transition_beats = (beats_per_leg - transition_start).max(1e-6);
        let tt = ((t_beat - transition_start) / transition_beats).clamp(0.0, 1.0) as f32;
        AutomationState::morph(from, to, tt, t_beat >= transition_start)
    }
}

/// A morph-less `ArcSwap`, shared by every entry point that doesn't run
/// `nooise auto`.
pub(crate) fn no_morph() -> Arc<ArcSwap<Option<MorphState>>> {
    Arc::new(ArcSwap::from_pointee(None))
}

/// Everything the UI thread needs to drive auto mode: the shared morph handle
/// (also held by the audio engine) plus the built-in states and leg length to
/// spin up a fresh morph on demand. Owns the on/off mechanics so the UI never
/// touches the `ArcSwap` directly.
pub(crate) struct AutoControls {
    morph: Arc<ArcSwap<Option<MorphState>>>,
    states: Vec<SongState>,
    bars: u32,
}

impl AutoControls {
    pub(crate) fn new(morph: Arc<ArcSwap<Option<MorphState>>>, states: Vec<SongState>, bars: u32) -> Self {
        Self { morph, states, bars }
    }

    /// True while a morph is running (auto mode is on).
    pub(crate) fn is_running(&self) -> bool {
        self.morph.load().is_some()
    }

    /// Leave auto mode. The engine stops rewriting controls and automation,
    /// so the current morphed values stay live and editable. A no-op when
    /// already off.
    pub(crate) fn exit(&self) {
        self.morph.store(Arc::new(None));
    }

    /// Flip auto mode. Turning on builds a morph anchored at `beat` starting
    /// from `current`/`current_automation`, so nothing jumps — any live
    /// LFO/envelope/macro routes ride along with the morph instead of being
    /// left behind; turning off just calls `exit`.
    pub(crate) fn toggle(&self, current: FluidControls, current_automation: AutomationState, beat: f64) {
        if self.is_running() {
            self.exit();
        } else {
            let state = MorphState::from_live(
                current,
                current_automation,
                self.states.clone(),
                self.bars,
                beat,
            );
            self.morph.store(Arc::new(Some(state)));
        }
    }
}

/// Throttled writer driving the morph from the engine's control-reload tick.
/// Recomputes and returns the morphed controls and automation only once per
/// 1/8 note, tracking the last beat it fired on so the audio thread never
/// rewrites the shared Arcs more often than that.
#[derive(Default)]
pub(crate) struct MorphWriter {
    last_tick_beat: Option<f64>,
}

impl MorphWriter {
    /// `Some((controls, automation))` when a new morph tick is due at `beat`;
    /// `None` otherwise (call site should skip the write).
    pub(crate) fn tick(&mut self, morph: &MorphState, beat: f64) -> Option<(FluidControls, AutomationState)> {
        let due = match self.last_tick_beat {
            None => true,
            Some(last) => beat - last >= MORPH_TICK_BEATS,
        };
        if !due {
            return None;
        }
        self.last_tick_beat = Some(beat);
        Some((morph.controls_at(beat), morph.automation_at(beat)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(bpm: f32) -> FluidControls {
        let mut c = FluidControls::default();
        c.master.bpm = bpm;
        c
    }

    /// Wrap bare controls into a `SongState` with no automation, for tests
    /// that only care about the controls side of a morph.
    fn song(controls: FluidControls) -> SongState {
        SongState {
            controls,
            automation: AutomationState::default(),
        }
    }

    /// Sum of every performing element's level/gain: the audible-energy proxy
    /// the never-silent invariant is checked against.
    fn total_level(c: &FluidControls) -> f32 {
        c.pad.level + c.perc.level + c.kick.level + c.tonal.level + c.clap.level + c.bass.level + c.arp.gain
    }

    #[test]
    fn auto_states_all_decode_without_error() {
        let states = decode_auto_states();
        assert_eq!(states.len(), AUTO_STATES.len());
    }

    #[test]
    fn leg_math_two_states_wraps_forever() {
        let morph = MorphState::new(vec![song(state(80.0)), song(state(120.0))], 2);
        // 2 bars/leg * 4 beats = 8 beats per leg.
        assert_eq!(morph.leg_at(0.0), (0, 1, 0.0));
        assert_eq!(morph.leg_at(4.0), (0, 1, 0.5));
        assert_eq!(morph.leg_at(8.0), (1, 0, 0.0));
        assert_eq!(morph.leg_at(12.0), (1, 0, 0.5));
        assert_eq!(morph.leg_at(16.0), (0, 1, 0.0));
    }

    #[test]
    fn leg_math_eight_states_wraps_forever() {
        let endpoints: Vec<SongState> = (0..8).map(|i| song(state(80.0 + i as f32))).collect();
        let morph = MorphState::new(endpoints, 1);
        // 1 bar/leg * 4 beats = 4 beats per leg.
        assert_eq!(morph.leg_at(0.0), (0, 1, 0.0));
        assert_eq!(morph.leg_at(28.0), (7, 0, 0.0));
        assert_eq!(morph.leg_at(30.0), (7, 0, 0.5));
        assert_eq!(morph.leg_at(32.0), (0, 1, 0.0));
    }

    #[test]
    fn leg_math_boundaries_at_t_zero_and_towards_one() {
        let morph = MorphState::new(vec![song(state(80.0)), song(state(120.0))], 1);
        let (_, _, t_start) = morph.leg_at(0.0);
        assert_eq!(t_start, 0.0);
        let (from, to, t_end) = morph.leg_at(3.999_999);
        assert_eq!((from, to), (0, 1));
        assert!(t_end > 0.99);
    }

    #[test]
    fn gain_param_lerps_linearly() {
        // 1 bar/leg: no hold window, so the whole leg is a linear glide.
        let mut from = FluidControls::default();
        from.pad.level = 0.0;
        let mut to = FluidControls::default();
        to.pad.level = 1.0;
        let morph = MorphState::new(vec![song(from), song(to)], 1);
        let controls = morph.controls_at(4.0 * 0.25);
        assert!((controls.pad.level - 0.25).abs() < 1e-4);
    }

    #[test]
    fn glide_holds_through_hold_window_then_lerps() {
        let mut from = FluidControls::default();
        from.master.drive = 0.0;
        let mut to = FluidControls::default();
        to.master.drive = 1.0;
        // 6 bars/leg: hold 4 bars (transition_start=16 beats), transition 8 beats.
        let morph = MorphState::new(vec![song(from), song(to)], 6);
        // Deep in the hold window: still `from`.
        assert!((morph.controls_at(8.0).master.drive - 0.0).abs() < 1e-4);
        // Halfway through the transition (beat 20 of 24): ~0.5.
        assert!((morph.controls_at(20.0).master.drive - 0.5).abs() < 1e-3);
        // Near the end of the transition: essentially `to`.
        assert!(morph.controls_at(23.9).master.drive > 0.98);
    }

    #[test]
    fn structural_params_snap_together_on_transition_downbeat() {
        let from = FluidControls::default();
        let mut to = FluidControls::default();
        to.pad.progression = 3.0;
        to.pad.chord_count = 2.0;
        to.arp.pattern = 2.0;
        // 6 bars/leg -> transition downbeat at beat 16.
        let morph = MorphState::new(vec![song(from.clone()), song(to.clone())], 6);

        // Just before the downbeat: all three still hold `from`.
        let before = morph.controls_at(15.9);
        assert_eq!(before.pad.progression, from.pad.progression);
        assert_eq!(before.pad.chord_count, from.pad.chord_count);
        assert_eq!(before.arp.pattern, from.arp.pattern);

        // On the downbeat: all three jump together, no interpolation.
        let after = morph.controls_at(16.0);
        assert_eq!(after.pad.progression, 3.0);
        assert_eq!(after.pad.chord_count, 2.0);
        assert_eq!(after.arp.pattern, 2.0);
    }

    #[test]
    fn nonstructural_grid_param_staggers_after_the_structural_downbeat() {
        let from = FluidControls::default();
        let mut to = FluidControls::default();
        to.tonal.synth_type = 1.0; // one changed non-structural grid param -> 8-bar offset
        // 30 bars/leg: hold 20 bars (transition_start=80), its jump at 80+8*4=112.
        let morph = MorphState::new(vec![song(from.clone()), song(to.clone())], 30);

        // Still holds through the structural downbeat and up to its own offset.
        assert_eq!(morph.controls_at(80.0).tonal.synth_type, from.tonal.synth_type);
        assert_eq!(morph.controls_at(111.0).tonal.synth_type, from.tonal.synth_type);
        // At its 8-bar offset it hard-switches.
        assert_eq!(morph.controls_at(112.0).tonal.synth_type, 1.0);
    }

    #[test]
    fn morph_never_dips_below_the_quieter_endpoint() {
        let mut loud = FluidControls::default();
        loud.pad.level = 0.9;
        loud.kick.level = 0.8;
        loud.bass.level = 0.7;
        let mut quiet = FluidControls::default();
        quiet.pad.level = 0.2;
        quiet.perc.level = 0.0;
        quiet.kick.level = 0.0;
        quiet.tonal.level = 0.0;
        quiet.clap.level = 0.0;
        quiet.bass.level = 0.1;
        quiet.arp.gain = 0.0;
        let floor = total_level(&loud).min(total_level(&quiet));
        let morph = MorphState::new(vec![song(loud), song(quiet)], 8);
        let beats_per_leg = 32.0;

        for i in 0..=64 {
            let beat = beats_per_leg * i as f64 / 64.0;
            assert!(
                total_level(&morph.controls_at(beat)) >= floor - 1e-4,
                "morph dipped below the quieter endpoint at beat {beat}"
            );
        }
    }

    #[test]
    fn from_live_starts_at_current_and_heads_to_nearest_state() {
        let mut current = FluidControls::default();
        current.pad.level = 0.5;
        let mut far = FluidControls::default();
        far.pad.level = 1.0;
        far.kick.level = 1.0;
        far.bass.level = 1.0;
        let mut near = FluidControls::default();
        near.pad.level = 0.5; // matches current, everything else default -> closest

        let morph = MorphState::from_live(
            current.clone(),
            AutomationState::default(),
            vec![song(far), song(near.clone())],
            4,
            0.0,
        );
        // Endpoint 0 is exactly the current state: toggling on changes nothing.
        assert_eq!(morph.endpoints[0].controls.pad.level, current.pad.level);
        // First target is the nearest built-in state, not the far one.
        assert_eq!(morph.endpoints[1].controls.pad.level, near.pad.level);
        assert_eq!(morph.endpoints[1].controls.kick.level, near.kick.level);
    }

    #[test]
    fn from_live_timeline_starts_at_the_toggle_beat() {
        let mut current = FluidControls::default();
        current.pad.level = 0.2;
        let mut target = FluidControls::default();
        target.pad.level = 0.9;
        // Toggle on at beat 1000: the leg must start there, not mid-loop.
        let morph = MorphState::from_live(
            current.clone(),
            AutomationState::default(),
            vec![song(target)],
            4,
            1000.0,
        );
        // At the toggle beat, output is exactly `current` (leg 0, t=0).
        assert!((morph.controls_at(1000.0).pad.level - 0.2).abs() < 1e-4);
    }

    #[test]
    fn writer_throttles_to_one_tick_per_eighth_note() {
        let morph = MorphState::new(vec![song(state(80.0)), song(state(120.0))], 64);
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

    fn lfo_route(depth_ratio: f32) -> LfoRoute {
        LfoRoute {
            depth_ratio,
            ..LfoRoute::default()
        }
    }

    #[test]
    fn automation_route_present_on_both_sides_glides_depth_and_snaps_shape() {
        let mut from_state = FluidControls::default();
        from_state.master.drive = 0.0;
        let mut to_state = FluidControls::default();
        to_state.master.drive = 0.0;

        let address = ControlAddress::new("pad.level");
        let mut from_auto = AutomationState::default();
        from_auto.set_route(
            address,
            LfoRoute { depth_ratio: 0.2, shape: LfoShape::Sine, ..LfoRoute::default() },
        );
        let mut to_auto = AutomationState::default();
        to_auto.set_route(
            address,
            LfoRoute { depth_ratio: 0.8, shape: LfoShape::Square, ..LfoRoute::default() },
        );

        let morph = MorphState::new(
            vec![
                SongState { controls: from_state, automation: from_auto },
                SongState { controls: to_state, automation: to_auto },
            ],
            6,
        );
        // 6 bars/leg -> transition downbeat at beat 16, transition ends at beat 24.

        // Deep in the hold window: depth and shape both still `from`.
        let held = morph.automation_at(8.0);
        assert!((held.route(address).unwrap().depth_ratio - 0.2).abs() < 1e-4);
        assert_eq!(held.route(address).unwrap().shape, LfoShape::Sine);

        // Halfway through the transition: depth has glided, shape has already
        // snapped to `to` at the downbeat (not interpolated).
        let mid = morph.automation_at(20.0);
        assert!((mid.route(address).unwrap().depth_ratio - 0.5).abs() < 1e-3);
        assert_eq!(mid.route(address).unwrap().shape, LfoShape::Square);

        // End of the transition: essentially `to`.
        let done = morph.automation_at(23.9);
        assert!(done.route(address).unwrap().depth_ratio > 0.78);
    }

    #[test]
    fn automation_route_added_by_target_fades_in_from_silence() {
        let from_auto = AutomationState::default();
        let address = ControlAddress::new("pad.level");
        let mut to_auto = AutomationState::default();
        to_auto.set_route(address, lfo_route(0.6));

        let morph = MorphState::new(
            vec![
                SongState { controls: FluidControls::default(), automation: from_auto },
                SongState { controls: FluidControls::default(), automation: to_auto },
            ],
            6,
        );

        // Present but silent (depth 0) during the hold window — functionally
        // identical to absent, since a zero-depth route has no audible effect.
        assert!((morph.automation_at(0.0).route(address).unwrap().depth_ratio).abs() < 1e-4);
        // Fading in during the transition, silent at its start.
        let mid = morph.automation_at(20.0);
        assert!((mid.route(address).unwrap().depth_ratio - 0.3).abs() < 1e-3);
    }

    #[test]
    fn automation_route_removed_by_target_fades_out_and_does_not_reappear() {
        // Three states so leg 1 (endpoint 1 -> endpoint 2) doesn't loop
        // straight back to the routed endpoint 0.
        let address = ControlAddress::new("pad.level");
        let mut routed = AutomationState::default();
        routed.set_route(address, lfo_route(0.6));
        let unrouted = AutomationState::default();

        let morph = MorphState::new(
            vec![
                SongState { controls: FluidControls::default(), automation: routed },
                SongState { controls: FluidControls::default(), automation: unrouted.clone() },
                SongState { controls: FluidControls::default(), automation: unrouted },
            ],
            6,
        );

        // Still full depth during the hold window of leg 0 (endpoint 0 -> 1).
        assert!((morph.automation_at(0.0).route(address).unwrap().depth_ratio - 0.6).abs() < 1e-4);
        // Fading out during the transition.
        let mid = morph.automation_at(20.0);
        assert!((mid.route(address).unwrap().depth_ratio - 0.3).abs() < 1e-3);
        // Gone once this leg's `to` (endpoint 1, unrouted) becomes leg 1's
        // `from`: beat 24 is the start of leg 1 (endpoint 1 -> 2, neither
        // routed).
        assert!(morph.automation_at(24.0).route(address).is_none());
    }

    #[test]
    fn from_live_carries_current_automation_as_endpoint_zero() {
        let address = ControlAddress::new("pad.level");
        let mut current_auto = AutomationState::default();
        current_auto.set_route(address, lfo_route(0.5));

        let morph = MorphState::from_live(
            FluidControls::default(),
            current_auto.clone(),
            vec![song(FluidControls::default())],
            4,
            0.0,
        );
        assert_eq!(
            morph.endpoints[0].automation.route(address).unwrap().depth_ratio,
            0.5
        );
    }
}
