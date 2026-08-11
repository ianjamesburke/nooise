//! Song codes: the binary snapshot format behind `Ctrl+S` and
//! `nooise <code>`.
//!
//! This module owns the container version and every value encoding. Control
//! values are generic over `all_specs()`; anything that is not a flat control
//! value (automation routes, runtime session records) gets its own record
//! type. Unknown record types skip; an unknown container version is fatal.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::song_ids::{song_id_at, song_id_index};
use super::voice::{TONAL_MAX_LOOP_STEPS, TONAL_PHRASES, TonalSequenceState};
use super::{
    AutomationState, ControlAddress, ControlKind, ControlSpec, DEFAULT_LFO_DEPTH_RATIO, EnvTrigger,
    EnvelopeRoute, FluidControls, LfoRoute, LfoShape, MAX_ENV_ATTACK_BEATS, MAX_ENV_DECAY_BEATS,
    MAX_LFO_CYCLE_BEATS, MAX_LFO_OFFSET_BEATS, MAX_LFO_STEPS, MIN_LFO_CYCLE_BEATS, Step, all_specs,
    spec_by_id,
};

const MAGIC: &[u8; 4] = b"NOOI";
/// Control ids are interned as `SONG_ID_TABLE` indexes and continuous values
/// are quantized to a u16 taper position. This is the only version axis —
/// record payloads carry no version byte of their own. Version 1 (length-
/// prefixed ids, f32 values, its own nested automation payload versions) is
/// gone; a v1 code is rejected with a message telling the user why.
const CONTAINER_VERSION: u8 = 2;
/// Unchanged across container versions: the CLI, `just add-morph`, and both
/// Python helpers all match song codes on this prefix.
const CODE_PREFIX: &str = "n1_";
pub(crate) const SNAPSHOT_RECORD: u8 = 0;
pub(crate) const AUTOMATION_RECORD: u8 = 1;
const TONAL_SEQUENCE_RECORD: u8 = 2;
const LFO_SHAPE_SINE: u8 = 0;
const LFO_SHAPE_TRIANGLE: u8 = 1;
const LFO_SHAPE_RAMP_UP: u8 = 2;
const LFO_SHAPE_RAMP_DOWN: u8 = 3;
const LFO_SHAPE_SQUARE: u8 = 4;
const LFO_SHAPE_RANDOM_DRIFT: u8 = 5;
const LFO_SHAPE_SAMPLE_HOLD: u8 = 6;
const LFO_SHAPE_STEPS: u8 = 7;
const ENV_TRIGGER_EVERY_BEATS: u8 = 0;
const ENV_TRIGGER_ON_KICK: u8 = 1;
const ENV_TRIGGER_ONCE: u8 = 2;
/// Fallback `EveryBeats` interval for an envelope whose stored trigger param
/// is non-finite; matches `EnvTrigger`'s own "every 4 beats" default.
const DEFAULT_ENV_TRIGGER_BEATS: f32 = 4.0;
/// A macro or envelope route with no audible effect is dead weight; skip it
/// on encode exactly like the LFO editor already prunes zero-depth routes.
const NEUTRAL_ENVELOPE_AMOUNT_EPSILON: f32 = f32::EPSILON;

#[derive(Clone, Default)]
pub(crate) struct SongState {
    pub(crate) controls: FluidControls,
    pub(crate) automation: AutomationState,
    pub(crate) tonal_sequence: Option<TonalSequenceState>,
}

impl SongState {
    pub(crate) fn from_controls(controls: FluidControls) -> Self {
        Self {
            controls,
            automation: AutomationState::default(),
            tonal_sequence: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SongCodeError {
    MissingPrefix,
    InvalidBase64,
    InvalidMagic,
    UnsupportedVersion(u8),
    Truncated,
    TooLarge,
    /// A stored value carried a tag this build has no encoding for.
    InvalidValueTag(u8),
    /// The code sets a control this build retired. Its value has nowhere to
    /// go, so the code is refused rather than loaded with that value missing.
    RetiredControl(&'static str),
    /// A live control has no `SONG_ID_TABLE` slot, so it cannot be saved.
    /// `song_ids_cover_every_registry_control` exists to stop this reaching a
    /// user; append the id to the table.
    UnregisteredControl(&'static str),
}

impl fmt::Display for SongCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrefix => write!(f, "song code must start with {CODE_PREFIX}"),
            Self::InvalidBase64 => write!(f, "song code is not valid base64url"),
            Self::InvalidMagic => write!(f, "song code is not a nooise snapshot"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "song code version {version} is from an older nooise and can no longer be read \
                 (this build writes and reads version {CONTAINER_VERSION})"
            ),
            Self::Truncated => write!(f, "song code is truncated"),
            Self::TooLarge => write!(f, "song code payload is too large"),
            Self::InvalidValueTag(tag) => write!(f, "song code has unknown value tag {tag}"),
            Self::RetiredControl(id) => write!(
                f,
                "song code sets {id}, a control this build no longer has; the code predates the \
                 change that retired it and can no longer be loaded"
            ),
            Self::UnregisteredControl(id) => {
                write!(f, "control {id} is missing from the song id table")
            }
        }
    }
}

impl Error for SongCodeError {}

pub(crate) fn encode_song_code(song: &SongState) -> Result<String, SongCodeError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.push(CONTAINER_VERSION);

    let mut snapshot = Vec::new();
    write_snapshot(&song.controls, &mut snapshot)?;
    write_record(SNAPSHOT_RECORD, &snapshot, &mut bytes)?;

    if automation_has_content(&song.automation) {
        let mut automation = Vec::new();
        write_automation(&song.automation, &mut automation)?;
        write_record(AUTOMATION_RECORD, &automation, &mut bytes)?;
    }

    if let Some(sequence) = &song.tonal_sequence {
        let mut tonal_sequence = Vec::new();
        write_tonal_sequence(sequence, &mut tonal_sequence)?;
        write_record(TONAL_SEQUENCE_RECORD, &tonal_sequence, &mut bytes)?;
    }
    Ok(format!("{CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub(crate) fn decode_song_code(code: &str) -> Result<SongState, SongCodeError> {
    let encoded = code
        .strip_prefix(CODE_PREFIX)
        .ok_or(SongCodeError::MissingPrefix)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| SongCodeError::InvalidBase64)?;
    let mut reader = Reader::new(&bytes);

    if reader.bytes(MAGIC.len())? != MAGIC {
        return Err(SongCodeError::InvalidMagic);
    }
    match reader.u8()? {
        CONTAINER_VERSION => decode_container(&mut reader),
        version => Err(SongCodeError::UnsupportedVersion(version)),
    }
}

fn decode_container(reader: &mut Reader) -> Result<SongState, SongCodeError> {
    let mut song = SongState::default();

    while !reader.is_empty() {
        let record_type = reader.u8()?;
        let len = reader.u32()? as usize;
        let payload = reader.bytes(len)?;
        match record_type {
            SNAPSHOT_RECORD => read_snapshot(payload, &mut song.controls)?,
            AUTOMATION_RECORD => read_automation(payload, &mut song.automation)?,
            TONAL_SEQUENCE_RECORD => song.tonal_sequence = Some(read_tonal_sequence(payload)?),
            // Records are length-prefixed, so an unknown one is skipped
            // without losing alignment. This stays permissive on purpose: it
            // is how a code from a newer nooise carrying a record this build
            // has never heard of still loads everything else. Version
            // mismatches are caught at the container version, which is a hard
            // error — this is forward compatibility, not a silent fallback.
            _ => {}
        }
    }

    Ok(song)
}

fn write_tonal_sequence(
    sequence: &TonalSequenceState,
    out: &mut Vec<u8>,
) -> Result<(), SongCodeError> {
    let note_count = u8::try_from(sequence.notes.len()).map_err(|_| SongCodeError::TooLarge)?;
    out.push(sequence.phrase as u8);
    out.push(note_count);
    for note in &sequence.notes {
        out.extend_from_slice(&note.to_le_bytes());
    }
    out.extend_from_slice(&sequence.evolution_seed.to_le_bytes());
    out.extend_from_slice(&sequence.evolution_count.to_le_bytes());
    Ok(())
}

fn read_tonal_sequence(bytes: &[u8]) -> Result<TonalSequenceState, SongCodeError> {
    let mut reader = Reader::new(bytes);
    let phrase = reader.u8()? as usize;
    let note_count = reader.u8()? as usize;
    if phrase >= TONAL_PHRASES.len() || note_count == 0 || note_count > TONAL_MAX_LOOP_STEPS {
        return Err(SongCodeError::Truncated);
    }
    let mut notes = Vec::with_capacity(note_count);
    for _ in 0..note_count {
        notes.push(reader.i32()?);
    }
    let evolution_seed = reader.u64()?;
    let evolution_count = reader.u64()?;
    if !reader.is_empty() {
        return Err(SongCodeError::Truncated);
    }
    Ok(TonalSequenceState {
        phrase,
        notes,
        evolution_seed,
        evolution_count,
    })
}

// ============================================================
// Codec
// ============================================================

/// How a snapshot entry spells its value. The tag travels with the value
/// so a reader never re-derives the choice from the live `ControlSpec` — a
/// control's kind or step ladder may change in a later build without
/// invalidating codes written today — and so an entry naming an unknown
/// control can still be skipped without losing byte alignment.
const VALUE_TAG_POSITION: u8 = 0;
const VALUE_TAG_INT: u8 = 1;
const VALUE_TAG_FLOAT: u8 = 2;
const VALUE_TAG_SMALL_INT: u8 = 3;

/// Even span for bipolar `-1..=1` amounts, so exactly 0 — the neutral value
/// every automation amount rests at — round-trips to exactly 0.
const BIPOLAR_SPAN: f32 = 65_534.0;

#[derive(Clone, Copy, PartialEq)]
enum EncodedValue {
    /// Position along the spec's taper: 0 is `min`, `u16::MAX` is `max`.
    Position(u16),
    SmallInt(i8),
    Int(i16),
    Float(f32),
}

impl EncodedValue {
    /// Continuous rows ride the taper in position space. Discrete rows and
    /// musical step ladders (`Step::PowerOfTwo`, `Step::BeatGrid`) do not:
    /// `ControlSpec::ratio` overrides the taper for both ladders and neither
    /// `beat_grid_ratio` nor the `Log2` override has an inverse in the crate,
    /// so they store their value exactly instead. Discrete rows are already
    /// whole numbers and would gain nothing but error from a round trip
    /// through 0..1.
    fn encode(spec: &ControlSpec, value: f32, c: &FluidControls) -> Self {
        if !spec.exact_in_song
            && spec.kind != ControlKind::Discrete
            && matches!(spec.step, Step::Linear(_))
        {
            Self::Position(unit_to_u16(spec.ratio(value, c)))
        } else {
            Self::exact(value)
        }
    }

    /// Smallest exactly-lossless spelling of a whole number: one byte through
    /// `i8` (which covers every discrete row — the widest is `master.tune` at
    /// -12..12 — and the low power-of-two/beat-grid rungs), two through `i16`,
    /// four as a raw `f32` for anything fractional. The value is absolute, not
    /// relative to `spec.min`, so a later build that retunes a control's range
    /// cannot silently reinterpret a code written today.
    fn exact(value: f32) -> Self {
        let rounded = value.round();
        if value != rounded {
            return Self::Float(value);
        }
        if (i8::MIN as f32..=i8::MAX as f32).contains(&rounded) {
            Self::SmallInt(rounded as i8)
        } else if (i16::MIN as f32..=i16::MAX as f32).contains(&rounded) {
            Self::Int(rounded as i16)
        } else {
            Self::Float(value)
        }
    }

    fn write(self, out: &mut Vec<u8>) {
        match self {
            Self::Position(position) => {
                out.push(VALUE_TAG_POSITION);
                out.extend_from_slice(&position.to_le_bytes());
            }
            Self::SmallInt(value) => {
                out.push(VALUE_TAG_SMALL_INT);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::Int(value) => {
                out.push(VALUE_TAG_INT);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::Float(value) => {
                out.push(VALUE_TAG_FLOAT);
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    fn read(reader: &mut Reader) -> Result<Self, SongCodeError> {
        match reader.u8()? {
            VALUE_TAG_POSITION => Ok(Self::Position(reader.u16()?)),
            VALUE_TAG_SMALL_INT => Ok(Self::SmallInt(reader.i8()?)),
            VALUE_TAG_INT => Ok(Self::Int(reader.i16()?)),
            VALUE_TAG_FLOAT => Ok(Self::Float(reader.f32()?)),
            tag => Err(SongCodeError::InvalidValueTag(tag)),
        }
    }

    /// `Taper::value_at` clamps its ratio but not its output, so the inverse
    /// can land a float hair outside the range; clamp here rather than relying
    /// on every caller to route through `apply_quantized_value`.
    fn resolve(self, spec: &ControlSpec) -> f32 {
        let value = match self {
            Self::Position(position) => {
                spec.taper
                    .value_at(u16_to_unit(position), spec.min, spec.max)
            }
            Self::SmallInt(value) => value as f32,
            Self::Int(value) => value as f32,
            Self::Float(value) => value,
        };
        value.clamp(spec.min, spec.max)
    }
}

/// A `0..=1` ratio as a u16; both endpoints land exactly.
fn unit_to_u16(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

fn u16_to_unit(value: u16) -> f32 {
    value as f32 / u16::MAX as f32
}

fn bipolar_to_u16(value: f32) -> u16 {
    ((value.clamp(-1.0, 1.0) + 1.0) * 0.5 * BIPOLAR_SPAN).round() as u16
}

fn u16_to_bipolar(value: u16) -> f32 {
    (value as f32 / BIPOLAR_SPAN).min(1.0) * 2.0 - 1.0
}

/// The `SONG_ID_TABLE` slot for a live control id. A registry control with no
/// slot is a table that was not appended to, not a user error.
fn write_control_index(id: &'static str, out: &mut Vec<u8>) -> Result<(), SongCodeError> {
    let index = song_id_index(id).ok_or(SongCodeError::UnregisteredControl(id))?;
    out.extend_from_slice(&index.to_le_bytes());
    Ok(())
}

/// The live spec an interned index names, or `None` when this build has no
/// such slot or no longer registers that control.
fn control_at(index: u16) -> Option<&'static ControlSpec> {
    song_id_at(index).and_then(spec_by_id)
}

/// Snapshot record:
/// `u16 entry_count`, then per entry `u16 id_index` + one tagged value.
/// Entries are still deduped first-wins over `all_specs()` and still pruned
/// against `FluidControls::default`.
///
/// The prune compares the *encoded* value against the encoded default rather
/// than the two raw floats. A plain absolute `f32::EPSILON` comparison is
/// only safe while `quantize` is a no-op for tapered dials: a position round
/// trip carries relative error, so on a large-magnitude control (`bass.cutoff`
/// at 8 kHz, `perc.decay_ms` at 2 s) a value that decoded to its own default
/// would re-encode as a spurious entry, growing the code on every save/load
/// cycle. Equal encodings are provably redundant — the reader would
/// reconstruct the default from either — so this both fixes that and prunes
/// slightly harder.
fn write_snapshot(controls: &FluidControls, out: &mut Vec<u8>) -> Result<(), SongCodeError> {
    let defaults = FluidControls::default();
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in all_specs() {
        if !seen.insert(spec.id) {
            continue;
        }
        // Slot fields borrow their units from the loaded module. Encode the
        // value and prune baseline through that same semantic view.
        let spec = spec.contextual(controls);
        // Both encodes map through the live session's contextual view (spec is
        // already contextual to `controls`, and `contextual` is idempotent) so
        // the baseline prune compares positions on one mapping.
        let value = EncodedValue::encode(&spec, spec.quantized_value(controls), controls);
        let default = EncodedValue::encode(&spec, spec.quantized_value(&defaults), controls);
        if value == default {
            continue;
        }
        let index = song_id_index(spec.id).ok_or(SongCodeError::UnregisteredControl(spec.id))?;
        entries.push((index, value));
    }

    write_u16(entries.len(), out)?;
    for (index, value) in entries {
        out.extend_from_slice(&index.to_le_bytes());
        value.write(out);
    }
    Ok(())
}

fn read_snapshot(bytes: &[u8], controls: &mut FluidControls) -> Result<(), SongCodeError> {
    let mut reader = Reader::new(bytes);
    let count = reader.u16()?;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let index = reader.u16()?;
        let value = EncodedValue::read(&mut reader)?;
        entries.push((index, value));
    }

    // A slot's kind and Delay clock modes define the units of its remaining
    // fields. Resolve those structural values first regardless of registry or
    // song-id order, then decode the parameters through the established unit.
    for structural in [true, false] {
        for &(index, value) in &entries {
            let id = song_id_at(index).unwrap_or_default();
            let is_structural = id.ends_with(".kind") || id.ends_with("clock");
            if is_structural != structural {
                continue;
            }
            if let Some(spec) = control_at(index) {
                let spec = spec.contextual(controls);
                spec.apply_quantized_value(value.resolve(&spec), controls);
            } else if !structural {
                reject_retired_control(index)?;
            }
        }
    }
    Ok(())
}

/// An index inside `SONG_ID_TABLE` whose id no registry control claims names
/// a control this build retired. There is nowhere to put its value, so the
/// code is rejected instead of decoded with that value silently missing.
/// An index past the table's end is a control from a *newer* build and stays
/// skipped — that is forward compatibility, not a retirement.
fn reject_retired_control(index: u16) -> Result<(), SongCodeError> {
    match song_id_at(index) {
        Some(id) if spec_by_id(id).is_none() => Err(SongCodeError::RetiredControl(id)),
        _ => Ok(()),
    }
}

/// Automation record. Control ids are interned `u16` indexes and the bounded
/// ratios (`depth_ratio`, `step_glide`, step values, and envelope amount) are
/// u16-quantized. Repeated control indexes represent stacked lanes.
/// Beat-valued fields and
/// `LfoRoute::seed` stay bit-exact — a quantized rate or seed would move
/// where a modulator sits on the transport grid.
fn write_automation(automation: &AutomationState, out: &mut Vec<u8>) -> Result<(), SongCodeError> {
    write_u16(automation.routes().count(), out)?;
    for (address, route) in automation.routes() {
        write_control_index(address.id(), out)?;
        out.extend_from_slice(&route.cycle_beats.to_le_bytes());
        out.extend_from_slice(&unit_to_u16(route.depth_ratio).to_le_bytes());
        out.push(shape_tag(route.shape));
        out.extend_from_slice(&route.phase_offset_beats.to_le_bytes());
        out.extend_from_slice(&route.seed.to_le_bytes());
        if route.shape == LfoShape::Steps {
            out.push(route.step_count);
            out.extend_from_slice(&unit_to_u16(route.step_glide).to_le_bytes());
            for value in &route.steps[..route.active_step_count()] {
                out.extend_from_slice(&bipolar_to_u16(*value).to_le_bytes());
            }
        }
    }

    // Reserved legacy macro-route section. Always empty in current codes.
    write_u16(0, out)?;

    let envelopes: Vec<_> = automation
        .envelopes()
        .filter(|(_, route)| route.amount.abs() > NEUTRAL_ENVELOPE_AMOUNT_EPSILON)
        .collect();
    write_u16(envelopes.len(), out)?;
    for (address, route) in envelopes {
        write_control_index(address.id(), out)?;
        out.extend_from_slice(&bipolar_to_u16(route.amount).to_le_bytes());
        out.extend_from_slice(&route.attack_beats.to_le_bytes());
        out.extend_from_slice(&route.decay_beats.to_le_bytes());
        let (tag, param) = env_trigger_tag(route.trigger);
        out.push(tag);
        out.extend_from_slice(&param.to_le_bytes());
    }

    // Reserved legacy field-macro section. Always empty in current codes.
    write_u16(0, out)?;

    Ok(())
}

fn read_automation(bytes: &[u8], automation: &mut AutomationState) -> Result<(), SongCodeError> {
    let mut reader = Reader::new(bytes);

    let lfo_count = reader.u16()?;
    for _ in 0..lfo_count {
        let index = reader.u16()?;
        let cycle_beats = reader.f32()?;
        let depth_ratio = u16_to_unit(reader.u16()?);
        let shape_byte = reader.u8()?;
        let phase_offset_beats = reader.f32()?;
        let seed = reader.u32()?;

        // Read the staircase before resolving the id and shape, so a route
        // this build cannot place is still skipped in byte-aligned whole.
        let steps = if shape_byte == LFO_SHAPE_STEPS {
            let step_count = reader.u8()?;
            let step_glide = u16_to_unit(reader.u16()?);
            let live = (step_count as usize).clamp(1, MAX_LFO_STEPS);
            let mut values = [0.0f32; MAX_LFO_STEPS];
            for value in values.iter_mut().take(live) {
                *value = u16_to_bipolar(reader.u16()?);
            }
            Some((step_count, step_glide, values))
        } else {
            None
        };

        reject_retired_control(index)?;
        let (Some(spec), Some(shape)) = (control_at(index), shape_from_tag(shape_byte)) else {
            continue;
        };
        let mut route = build_lfo_route(cycle_beats, depth_ratio, shape, phase_offset_beats, seed);
        if let Some((step_count, step_glide, values)) = steps {
            route.step_count = step_count.clamp(1, MAX_LFO_STEPS as u8);
            route.step_glide = step_glide;
            route.steps = values;
        }
        automation.add_route(ControlAddress::new(spec.id), route);
    }

    let macro_count = reader.u16()?;
    if macro_count > 0 {
        return Err(SongCodeError::RetiredControl("macro route"));
    }

    let envelope_count = reader.u16()?;
    for _ in 0..envelope_count {
        let index = reader.u16()?;
        let amount = u16_to_bipolar(reader.u16()?);
        let attack_beats = reader.f32()?;
        let decay_beats = reader.f32()?;
        let trigger_tag = reader.u8()?;
        let trigger_param = reader.f32()?;

        reject_retired_control(index)?;
        let (Some(spec), Some(trigger)) = (
            control_at(index),
            env_trigger_from_tag(
                trigger_tag,
                finite_or(trigger_param, DEFAULT_ENV_TRIGGER_BEATS),
            ),
        ) else {
            continue;
        };
        automation.add_envelope(
            ControlAddress::new(spec.id),
            EnvelopeRoute {
                amount,
                attack_beats: finite_or(attack_beats, 0.0).clamp(0.0, MAX_ENV_ATTACK_BEATS),
                decay_beats: finite_or(decay_beats, 0.0).clamp(0.0, MAX_ENV_DECAY_BEATS),
                trigger,
            },
        );
    }

    let field_macro_count = reader.u16()?;
    if field_macro_count > 0 {
        return Err(SongCodeError::RetiredControl("macro route"));
    }

    Ok(())
}

/// A route or envelope worth persisting. Mirrors the
/// pruning `AutomationState::close_editor` already applies in the UI, so a
/// route the editor would delete on close never round-trips through a song
/// code either.
fn automation_has_content(automation: &AutomationState) -> bool {
    automation.routes().next().is_some()
        || automation
            .envelopes()
            .any(|(_, route)| route.amount.abs() > NEUTRAL_ENVELOPE_AMOUNT_EPSILON)
}

/// Shared `LfoRoute` construction for the reader: clamps every field to its
/// valid range and substitutes a default for anything non-finite, so a
/// corrupt or hand-edited code cannot install an out-of-range modulator.
fn build_lfo_route(
    cycle_beats: f32,
    depth_ratio: f32,
    shape: LfoShape,
    phase_offset_beats: f32,
    seed: u32,
) -> LfoRoute {
    LfoRoute {
        cycle_beats: finite_or(cycle_beats, 2.0).clamp(MIN_LFO_CYCLE_BEATS, MAX_LFO_CYCLE_BEATS),
        depth_ratio: finite_or(depth_ratio, DEFAULT_LFO_DEPTH_RATIO).clamp(0.0, 1.0),
        shape,
        phase_offset_beats: finite_or(phase_offset_beats, 0.0).clamp(0.0, MAX_LFO_OFFSET_BEATS),
        seed,
        // A non-Steps route ignores these; a Steps route overwrites them
        // from its inline staircase.
        ..LfoRoute::default()
    }
}

fn env_trigger_tag(trigger: EnvTrigger) -> (u8, f32) {
    match trigger {
        EnvTrigger::EveryBeats(beats) => (ENV_TRIGGER_EVERY_BEATS, beats),
        EnvTrigger::OnKick => (ENV_TRIGGER_ON_KICK, 0.0),
        EnvTrigger::Once => (ENV_TRIGGER_ONCE, 0.0),
    }
}

fn env_trigger_from_tag(tag: u8, param: f32) -> Option<EnvTrigger> {
    match tag {
        ENV_TRIGGER_EVERY_BEATS => Some(EnvTrigger::EveryBeats(param)),
        ENV_TRIGGER_ON_KICK => Some(EnvTrigger::OnKick),
        ENV_TRIGGER_ONCE => Some(EnvTrigger::Once),
        _ => None,
    }
}

fn shape_tag(shape: LfoShape) -> u8 {
    match shape {
        LfoShape::Sine => LFO_SHAPE_SINE,
        LfoShape::Triangle => LFO_SHAPE_TRIANGLE,
        LfoShape::RampUp => LFO_SHAPE_RAMP_UP,
        LfoShape::RampDown => LFO_SHAPE_RAMP_DOWN,
        LfoShape::Square => LFO_SHAPE_SQUARE,
        LfoShape::RandomDrift => LFO_SHAPE_RANDOM_DRIFT,
        LfoShape::SampleHold => LFO_SHAPE_SAMPLE_HOLD,
        LfoShape::Steps => LFO_SHAPE_STEPS,
    }
}

fn shape_from_tag(tag: u8) -> Option<LfoShape> {
    match tag {
        LFO_SHAPE_SINE => Some(LfoShape::Sine),
        LFO_SHAPE_TRIANGLE => Some(LfoShape::Triangle),
        LFO_SHAPE_RAMP_UP => Some(LfoShape::RampUp),
        LFO_SHAPE_RAMP_DOWN => Some(LfoShape::RampDown),
        LFO_SHAPE_SQUARE => Some(LfoShape::Square),
        LFO_SHAPE_RANDOM_DRIFT => Some(LfoShape::RandomDrift),
        LFO_SHAPE_SAMPLE_HOLD => Some(LfoShape::SampleHold),
        LFO_SHAPE_STEPS => Some(LfoShape::Steps),
        _ => None,
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

pub(crate) fn write_record(
    record_type: u8,
    payload: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), SongCodeError> {
    let len = u32::try_from(payload.len()).map_err(|_| SongCodeError::TooLarge)?;
    out.push(record_type);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

fn write_u16(value: usize, out: &mut Vec<u8>) -> Result<(), SongCodeError> {
    let value = u16::try_from(value).map_err(|_| SongCodeError::TooLarge)?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.bytes.len()
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], SongCodeError> {
        let end = self.pos.checked_add(len).ok_or(SongCodeError::TooLarge)?;
        let Some(bytes) = self.bytes.get(self.pos..end) else {
            return Err(SongCodeError::Truncated);
        };
        self.pos = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, SongCodeError> {
        Ok(self.bytes(1)?[0])
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], SongCodeError> {
        let mut bytes = [0u8; N];
        bytes.copy_from_slice(self.bytes(N)?);
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16, SongCodeError> {
        Ok(u16::from_le_bytes(self.read_array()?))
    }

    fn u32(&mut self) -> Result<u32, SongCodeError> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    fn u64(&mut self) -> Result<u64, SongCodeError> {
        Ok(u64::from_le_bytes(self.read_array()?))
    }

    fn i8(&mut self) -> Result<i8, SongCodeError> {
        Ok(self.u8()? as i8)
    }

    fn i16(&mut self) -> Result<i16, SongCodeError> {
        Ok(i16::from_le_bytes(self.read_array()?))
    }

    fn i32(&mut self) -> Result<i32, SongCodeError> {
        Ok(i32::from_le_bytes(self.read_array()?))
    }

    fn f32(&mut self) -> Result<f32, SongCodeError> {
        Ok(f32::from_le_bytes(self.read_array()?))
    }
}

#[cfg(test)]
mod retired_control_tests {
    use super::*;

    /// One snapshot entry naming `id`, wrapped in a valid container.
    fn code_setting(id: &str) -> String {
        let index = song_id_index(id).expect("id is in the table");
        let mut snapshot = Vec::new();
        write_u16(1usize, &mut snapshot).unwrap();
        snapshot.extend_from_slice(&index.to_le_bytes());
        EncodedValue::Position(unit_to_u16(0.35)).write(&mut snapshot);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.push(CONTAINER_VERSION);
        write_record(SNAPSHOT_RECORD, &snapshot, &mut bytes).unwrap();
        format!("{CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
    }

    /// A code from before the per-voice effect sliders folded into module
    /// slots names controls that no longer exist. Loading it and quietly
    /// leaving those values at their defaults would hand back a song that
    /// sounds wrong with no explanation, so the code is refused instead.
    #[test]
    fn a_code_setting_a_retired_control_is_refused() {
        assert_eq!(
            decode_song_code(&code_setting("pad.reverb_mix")).err(),
            Some(SongCodeError::RetiredControl("pad.reverb_mix"))
        );
    }

    /// The same skip that makes a retired id fatal must not catch an id from
    /// a newer build: that index is past the table's end, not inside it.
    #[test]
    fn a_code_setting_an_id_this_build_has_never_heard_of_still_loads() {
        let mut snapshot = Vec::new();
        write_u16(1usize, &mut snapshot).unwrap();
        snapshot.extend_from_slice(&u16::MAX.to_le_bytes());
        EncodedValue::Position(unit_to_u16(0.35)).write(&mut snapshot);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.push(CONTAINER_VERSION);
        write_record(SNAPSHOT_RECORD, &snapshot, &mut bytes).unwrap();
        let code = format!("{CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));

        assert!(decode_song_code(&code).is_ok());
    }
}
