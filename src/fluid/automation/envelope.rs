//! One-shot envelope routes: what retriggers them, their fields, and the
//! attack/decay curve both the engine and the animated lane read.

use crate::fluid::widget::DialScale;
use crate::fluid::{Entry, TIME_TAPER, Taper};

use super::{FieldSpec, ModContext, Stepping, clamped_index, morph_scalar_route, stepped_index};

// Envelope route field ranges. Attack/decay reach into the minutes at slow
// tempos (512 beats is ~6 min at 82 BPM, ~12 min at 40 BPM) so the same
// one-shot serves both fast swells and set-and-forget macro blooms.
pub(crate) const MAX_ENV_ATTACK_BEATS: f32 = 512.0;
pub(crate) const MAX_ENV_DECAY_BEATS: f32 = 512.0;
const ENV_BEATS_STEP: f32 = 0.5;
const ENV_AMOUNT_STEP: f32 = 0.01;
const DEFAULT_ENV_ATTACK_BEATS: f32 = 1.0;
const DEFAULT_ENV_DECAY_BEATS: f32 = 4.0;
// ============================================================
// Envelope routes
// ============================================================

/// What re-triggers a one-shot envelope. `EveryBeats` cycles on a musical grid,
/// `OnKick` fires with the kick, and `Once` is the set-and-forget macro that
/// sweeps a single time from song start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EnvTrigger {
    EveryBeats(f32),
    OnKick,
    Once,
}

impl EnvTrigger {
    /// Ordered presets the Trigger field cycles through, folding the every-N
    /// interval choices and the macro one-shot into one discrete field.
    const CYCLE: [EnvTrigger; 8] = [
        Self::EveryBeats(1.0),
        Self::EveryBeats(2.0),
        Self::EveryBeats(4.0),
        Self::EveryBeats(8.0),
        Self::EveryBeats(16.0),
        Self::EveryBeats(32.0),
        Self::OnKick,
        Self::Once,
    ];

    fn index(self) -> usize {
        Self::CYCLE.iter().position(|&t| t == self).unwrap_or(2) // default: every 4 beats
    }

    fn cycled(self, dir: f32) -> Self {
        Self::CYCLE[stepped_index(self.index(), dir, Self::CYCLE.len())]
    }

    fn from_index(index: f32) -> Self {
        Self::CYCLE[clamped_index(index, Self::CYCLE.len())]
    }

    fn label(self) -> String {
        match self {
            Self::EveryBeats(n) => format!("every {n:.0} beats"),
            Self::OnKick => "on kick".to_string(),
            Self::Once => "once (macro)".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnvField {
    Amount,
    Attack,
    Decay,
    Trigger,
}

impl EnvField {
    pub(crate) const ALL: [EnvField; 4] = [Self::Amount, Self::Attack, Self::Decay, Self::Trigger];

    /// How this field maps onto bar position. Trigger is a discrete enum and
    /// carries no numeric spec, so it spans the bar by variant index.
    pub(crate) fn scale(self) -> DialScale {
        match self {
            Self::Trigger => DialScale::enumerated(EnvTrigger::CYCLE.len()),
            _ => self.spec().scale,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            _ => self.spec().label,
        }
    }

    /// Only continuous slider fields carry a numeric spec; Trigger is discrete.
    fn spec(self) -> &'static FieldSpec<EnvField> {
        ENV_FIELD_SPECS
            .iter()
            .find(|spec| spec.field == self)
            .expect("every continuous envelope field has a spec")
    }
}

/// Attack and decay span 512 beats but every musically ordinary setting lives
/// in the first few, so they take the same exp taper the registry gives its
/// time controls and step in position space: a linear sweep buried 4 beats
/// inside the first 1% of the bar and cost 1024 presses end to end.
const ENV_FIELD_SPECS: &[FieldSpec<EnvField>] = &[
    FieldSpec {
        field: EnvField::Amount,
        label: "amount",
        min: -1.0,
        max: 1.0,
        step: ENV_AMOUNT_STEP,
        scale: DialScale::bipolar(),
        stepping: Stepping::Linear,
        entry: Entry::Percent,
        reset: 0.0,
    },
    FieldSpec {
        field: EnvField::Attack,
        label: "attack",
        min: 0.0,
        max: MAX_ENV_ATTACK_BEATS,
        step: ENV_BEATS_STEP,
        scale: DialScale::tapered(0.0, MAX_ENV_ATTACK_BEATS, Taper::Exp(TIME_TAPER)),
        stepping: Stepping::Position,
        entry: Entry::Snap,
        reset: DEFAULT_ENV_ATTACK_BEATS,
    },
    FieldSpec {
        field: EnvField::Decay,
        label: "decay",
        min: 0.0,
        max: MAX_ENV_DECAY_BEATS,
        step: ENV_BEATS_STEP,
        scale: DialScale::tapered(0.0, MAX_ENV_DECAY_BEATS, Taper::Exp(TIME_TAPER)),
        stepping: Stepping::Position,
        entry: Entry::Snap,
        reset: DEFAULT_ENV_DECAY_BEATS,
    },
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EnvelopeRoute {
    /// Bipolar sweep depth in -1..1; positive blooms up, negative dips down.
    pub(crate) amount: f32,
    pub(crate) attack_beats: f32,
    /// Fall time back to base; 0 holds at the peak indefinitely (macro hold).
    pub(crate) decay_beats: f32,
    pub(crate) trigger: EnvTrigger,
}

impl Default for EnvelopeRoute {
    fn default() -> Self {
        Self {
            amount: 0.0,
            attack_beats: DEFAULT_ENV_ATTACK_BEATS,
            decay_beats: DEFAULT_ENV_DECAY_BEATS,
            trigger: EnvTrigger::EveryBeats(4.0),
        }
    }
}

impl EnvelopeRoute {
    /// Beats elapsed since the most recent trigger, or None before the first
    /// trigger has fired. Pure function of the context so UI and engine agree.
    fn beats_since_trigger(&self, ctx: ModContext) -> Option<f32> {
        match self.trigger {
            EnvTrigger::EveryBeats(n) => {
                let n = f64::from(n.max(ENV_BEATS_STEP));
                if ctx.beat < 0.0 {
                    return None;
                }
                Some(ctx.beat.rem_euclid(n) as f32)
            }
            EnvTrigger::Once => {
                if ctx.beat < 0.0 {
                    None
                } else {
                    Some(ctx.beat as f32)
                }
            }
            EnvTrigger::OnKick => {
                let interval = f64::from(ctx.kick_interval_beats.max(1.0 / 64.0));
                let offset = f64::from(ctx.kick_offset_beats).rem_euclid(interval);
                let slot = ((ctx.beat - offset) / interval).floor();
                let last = offset + slot * interval;
                if last < -1e-9 {
                    None
                } else {
                    Some((ctx.beat - last) as f32)
                }
            }
        }
    }

    /// One-shot AD level in 0..1 at the given beat. Zero attack fires instantly;
    /// zero decay holds at the peak (set-and-forget macro).
    pub(crate) fn level_at(&self, ctx: ModContext) -> f32 {
        let Some(since) = self.beats_since_trigger(ctx) else {
            return 0.0;
        };
        self.level_for_elapsed(since)
    }

    fn level_for_elapsed(&self, since: f32) -> f32 {
        if since < 0.0 {
            0.0
        } else if self.attack_beats > 0.0 && since < self.attack_beats {
            since / self.attack_beats
        } else if self.decay_beats <= 0.0 {
            1.0
        } else if since < self.attack_beats + self.decay_beats {
            1.0 - (since - self.attack_beats) / self.decay_beats
        } else {
            0.0
        }
    }

    /// Beats spanned by one trigger period, used to scope the animated lane.
    pub(crate) fn window_beats(&self) -> f32 {
        match self.trigger {
            EnvTrigger::EveryBeats(n) => n.max(ENV_BEATS_STEP),
            EnvTrigger::OnKick => self.attack_beats + self.decay_beats.max(ENV_BEATS_STEP),
            EnvTrigger::Once => (self.attack_beats + self.decay_beats).max(ENV_BEATS_STEP),
        }
    }

    /// Envelope level at a given elapsed beat, for drawing the lane curve.
    pub(crate) fn level_for_lane(&self, since: f32) -> f32 {
        self.level_for_elapsed(since)
    }

    /// Where the live phase head sits along the lane window, 0..1.
    pub(crate) fn lane_head_phase(&self, ctx: ModContext) -> f32 {
        match self.beats_since_trigger(ctx) {
            Some(since) => (since / self.window_beats().max(ENV_BEATS_STEP)).clamp(0.0, 1.0),
            None => 0.0,
        }
    }

    pub(crate) fn adjust_field(&mut self, field: EnvField, dir: f32) {
        match field {
            EnvField::Trigger => self.trigger = self.trigger.cycled(dir),
            _ => {
                let next = field.spec().adjust(self.field_value(field), dir);
                self.write_field(field, next);
            }
        }
    }

    pub(crate) fn set_field(&mut self, field: EnvField, value: f32) {
        match field {
            EnvField::Trigger => self.trigger = EnvTrigger::from_index(value),
            _ => self.write_field(field, field.spec().parse_value(value)),
        }
    }

    /// Set a time field to an exact value, clamped to range but not snapped
    /// to the beat grid — used while the field is being driven in ms.
    pub(crate) fn set_field_raw(&mut self, field: EnvField, value: f32) {
        match field {
            EnvField::Attack | EnvField::Decay => {
                let spec = field.spec();
                self.write_field(field, value.clamp(spec.min, spec.max));
            }
            EnvField::Amount | EnvField::Trigger => self.set_field(field, value),
        }
    }

    pub(crate) fn reset_field(&mut self, field: EnvField) {
        match field {
            EnvField::Trigger => self.trigger = EnvelopeRoute::default().trigger,
            _ => self.write_field(field, field.spec().reset),
        }
    }

    /// Store an already ranged value on the field its spec came from. Trigger
    /// is discrete and never routed here.
    fn write_field(&mut self, field: EnvField, value: f32) {
        match field {
            EnvField::Amount => self.amount = value,
            EnvField::Attack => self.attack_beats = value,
            EnvField::Decay => self.decay_beats = value,
            EnvField::Trigger => {}
        }
    }

    /// The field's value in the units its own scale is expressed in.
    pub(crate) fn field_value(&self, field: EnvField) -> f32 {
        match field {
            EnvField::Amount => self.amount,
            EnvField::Attack => self.attack_beats,
            EnvField::Decay => self.decay_beats,
            EnvField::Trigger => self.trigger.index() as f32,
        }
    }

    pub(crate) fn field_display(&self, field: EnvField) -> String {
        match field {
            EnvField::Amount => format!("{:+.0}%", self.amount * 100.0),
            EnvField::Attack => format!("{:.2} beats", self.attack_beats),
            EnvField::Decay => {
                if self.decay_beats <= 0.0 {
                    "hold".to_string()
                } else {
                    format!("{:.2} beats", self.decay_beats)
                }
            }
            EnvField::Trigger => self.trigger.label(),
        }
    }

    /// Morph an optional envelope route across a leg transition; same
    /// glide/snap split as `LfoRoute::morph` with `amount` as the level
    /// field. See `morph_scalar_route` for the full rationale.
    pub(super) fn morph(
        from: Option<&EnvelopeRoute>,
        to: Option<&EnvelopeRoute>,
        tt: f32,
        use_to: bool,
    ) -> Option<EnvelopeRoute> {
        morph_scalar_route(from, to, tt, use_to, |r| r.amount, |r, v| r.amount = v)
    }
}
