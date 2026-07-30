use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::song_ids::{song_id_at, song_id_index};
use super::voice::{TONAL_MAX_LOOP_STEPS, TONAL_PHRASES, TonalSequenceState};
use super::{
    AutomationState, ControlAddress, ControlKind, ControlSpec, DEFAULT_LFO_DEPTH_RATIO, EnvTrigger,
    EnvelopeRoute, FluidControls, LfoRoute, LfoShape, MACRO_COUNT, MAX_ENV_ATTACK_BEATS,
    MAX_ENV_DECAY_BEATS, MAX_LFO_CYCLE_BEATS, MAX_LFO_OFFSET_BEATS, MAX_LFO_STEPS,
    MIN_LFO_CYCLE_BEATS, MacroRoute, Step, all_specs, spec_by_id,
};

const MAGIC: &[u8; 4] = b"NOOI";
/// Container v1: an app-version string followed by records that spell every
/// control id out in full and store every value as an f32.
const CONTAINER_VERSION_V1: u8 = 1;
/// Container v2: no app-version string, control ids interned as
/// `SONG_ID_TABLE` indexes, and continuous values quantized to a u16 taper
/// position. The container version is the only version axis — record payloads
/// carry no version byte of their own.
const CONTAINER_VERSION: u8 = 2;
/// Unchanged across container versions: the CLI, `just add-morph`, and both
/// Python helpers all match song codes on this prefix.
const CODE_PREFIX: &str = "n1_";
pub(crate) const SNAPSHOT_RECORD: u8 = 0;
pub(crate) const AUTOMATION_RECORD: u8 = 1;
const TONAL_SEQUENCE_RECORD: u8 = 2;
const AUTOMATION_PAYLOAD_VERSION_V2: u8 = 2;
const AUTOMATION_PAYLOAD_VERSION_V3: u8 = 3;
const AUTOMATION_PAYLOAD_VERSION_V4: u8 = 4;
const AUTOMATION_PAYLOAD_VERSION_V5: u8 = 5;
const AUTOMATION_PAYLOAD_VERSION: u8 = 6;
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
/// Default `EveryBeats` interval used when a v3 payload's trigger param is
/// missing or non-finite; matches `EnvTrigger`'s own "every 4 beats" default.
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SongCodeError {
    MissingPrefix,
    InvalidBase64,
    InvalidMagic,
    UnsupportedVersion(u8),
    Truncated,
    InvalidUtf8,
    TooLarge,
    /// A container-v2 value carried a tag this build has no encoding for.
    InvalidValueTag(u8),
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
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported song code version {version}")
            }
            Self::Truncated => write!(f, "song code is truncated"),
            Self::InvalidUtf8 => write!(f, "song code contains invalid text"),
            Self::TooLarge => write!(f, "song code payload is too large"),
            Self::InvalidValueTag(tag) => write!(f, "song code has unknown value tag {tag}"),
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
    write_snapshot_c2(&song.controls, &mut snapshot)?;
    write_record(SNAPSHOT_RECORD, &snapshot, &mut bytes)?;

    if automation_has_content(&song.automation) {
        let mut automation = Vec::new();
        write_automation_c2(&song.automation, &mut automation)?;
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
        CONTAINER_VERSION_V1 => decode_container_v1(&mut reader),
        CONTAINER_VERSION => decode_container_v2(&mut reader),
        version => Err(SongCodeError::UnsupportedVersion(version)),
    }
}

fn decode_container_v1(reader: &mut Reader) -> Result<SongState, SongCodeError> {
    let _app_version = reader.string()?;
    let mut song = SongState::default();

    while !reader.is_empty() {
        let record_type = reader.u8()?;
        let len = reader.u32()? as usize;
        let payload = reader.bytes(len)?;
        match record_type {
            SNAPSHOT_RECORD => read_snapshot(payload, &mut song.controls)?,
            AUTOMATION_RECORD => read_automation(payload, &mut song.automation)?,
            TONAL_SEQUENCE_RECORD => song.tonal_sequence = Some(read_tonal_sequence(payload)?),
            _ => {}
        }
    }

    Ok(song)
}

fn decode_container_v2(reader: &mut Reader) -> Result<SongState, SongCodeError> {
    let mut song = SongState::default();

    while !reader.is_empty() {
        let record_type = reader.u8()?;
        let len = reader.u32()? as usize;
        let payload = reader.bytes(len)?;
        match record_type {
            SNAPSHOT_RECORD => read_snapshot_c2(payload, &mut song.controls)?,
            AUTOMATION_RECORD => read_automation_c2(payload, &mut song.automation)?,
            TONAL_SEQUENCE_RECORD => song.tonal_sequence = Some(read_tonal_sequence(payload)?),
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

fn read_snapshot(bytes: &[u8], controls: &mut FluidControls) -> Result<(), SongCodeError> {
    let mut reader = Reader::new(bytes);
    let count = reader.u16()?;
    for _ in 0..count {
        let id = reader.string()?;
        let value = reader.f32()?;
        if let Some(spec) = spec_by_id(id) {
            spec.apply_quantized_value(value, controls);
        }
    }
    Ok(())
}

// ============================================================
// Container v2 codec
//
// `_c2` marks a container-v2 reader/writer, distinct from the `_v<n>`
// suffixes on the v1 container's per-payload automation versions.
// ============================================================

/// How a container-v2 entry spells its value. The tag travels with the value
/// so a reader never re-derives the choice from the live `ControlSpec` — a
/// control's kind or step ladder may change in a later build without
/// invalidating codes written today — and so an entry naming an unknown
/// control can still be skipped without losing byte alignment.
const VALUE_TAG_POSITION: u8 = 0;
const VALUE_TAG_INT: u8 = 1;
const VALUE_TAG_FLOAT: u8 = 2;

/// Even span for bipolar `-1..=1` amounts, so exactly 0 — the neutral value
/// every automation amount rests at — round-trips to exactly 0.
const BIPOLAR_SPAN: f32 = 65_534.0;

#[derive(Clone, Copy)]
enum EncodedValue {
    /// Position along the spec's taper: 0 is `min`, `u16::MAX` is `max`.
    Position(u16),
    Int(i16),
    Float(f32),
}

impl EncodedValue {
    /// Continuous rows ride the taper in position space. Discrete rows and
    /// musical step ladders (`Step::PowerOfTwo`, `Step::BeatGrid`) do not:
    /// `ControlSpec::ratio` overrides the taper for those and has no inverse,
    /// so they store their value exactly instead.
    fn encode(spec: &ControlSpec, value: f32) -> Self {
        if spec.kind != ControlKind::Discrete && matches!(spec.step, Step::Linear(_)) {
            Self::Position(unit_to_u16(spec.ratio(value)))
        } else {
            Self::exact(value)
        }
    }

    /// Exact in two bytes when the value is a whole number in `i16` range —
    /// which every discrete row and every power-of-two rung above 1 is — and
    /// in four otherwise.
    fn exact(value: f32) -> Self {
        let rounded = value.round();
        if value == rounded && (i16::MIN as f32..=i16::MAX as f32).contains(&rounded) {
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
            VALUE_TAG_INT => Ok(Self::Int(reader.i16()?)),
            VALUE_TAG_FLOAT => Ok(Self::Float(reader.f32()?)),
            tag => Err(SongCodeError::InvalidValueTag(tag)),
        }
    }

    fn resolve(self, spec: &ControlSpec) -> f32 {
        match self {
            Self::Position(position) => {
                spec.taper
                    .value_at(u16_to_unit(position), spec.min, spec.max)
            }
            Self::Int(value) => value as f32,
            Self::Float(value) => value,
        }
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

/// Snapshot record, container v2:
/// `u16 entry_count`, then per entry `u16 id_index` + one tagged value.
/// Entries are still pruned against `FluidControls::default`, still deduped
/// first-wins over `all_specs()`.
fn write_snapshot_c2(controls: &FluidControls, out: &mut Vec<u8>) -> Result<(), SongCodeError> {
    let defaults = FluidControls::default();
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in all_specs() {
        if !seen.insert(spec.id) {
            continue;
        }
        let value = spec.quantized_value(controls);
        let default = spec.quantized_value(&defaults);
        if (value - default).abs() <= f32::EPSILON {
            continue;
        }
        let index = song_id_index(spec.id).ok_or(SongCodeError::UnregisteredControl(spec.id))?;
        entries.push((index, EncodedValue::encode(spec, value)));
    }

    write_u16(entries.len(), out)?;
    for (index, value) in entries {
        out.extend_from_slice(&index.to_le_bytes());
        value.write(out);
    }
    Ok(())
}

fn read_snapshot_c2(bytes: &[u8], controls: &mut FluidControls) -> Result<(), SongCodeError> {
    let mut reader = Reader::new(bytes);
    let count = reader.u16()?;
    for _ in 0..count {
        let index = reader.u16()?;
        let value = EncodedValue::read(&mut reader)?;
        let Some(spec) = control_at(index) else {
            continue;
        };
        spec.apply_quantized_value(value.resolve(spec), controls);
    }
    Ok(())
}

/// Automation record, container v2. Section order, pruning, and every
/// read-side clamp match the v1 container's v6 payload; the changes are
/// interned `u16` control ids in place of length-prefixed strings and u16
/// quantization for the bounded ratios (`depth_ratio`, `step_glide`, macro
/// amounts, step values, envelope amount). Beat-valued fields and
/// `LfoRoute::seed` stay bit-exact — a quantized rate or seed would move
/// where a modulator sits on the transport grid.
///
/// Field-macro keys stay length-prefixed strings: they are composite
/// `control.id#lfo.field` keys owned by `automation.rs`, not registry control
/// ids, so `SONG_ID_TABLE` does not cover them.
fn write_automation_c2(
    automation: &AutomationState,
    out: &mut Vec<u8>,
) -> Result<(), SongCodeError> {
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

    let macros: Vec<_> = automation
        .macro_routes()
        .filter(|(_, route)| !route.is_neutral())
        .collect();
    write_u16(macros.len(), out)?;
    for (address, route) in macros {
        write_control_index(address.id(), out)?;
        write_macro_amounts_c2(route, out);
    }

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

    let field_macros: Vec<_> = automation
        .field_macros()
        .filter(|(_, route)| !route.is_neutral())
        .collect();
    write_u16(field_macros.len(), out)?;
    for (key, route) in field_macros {
        write_str(key, out)?;
        write_macro_amounts_c2(route, out);
    }

    Ok(())
}

fn write_macro_amounts_c2(route: &MacroRoute, out: &mut Vec<u8>) {
    for amount in route.amounts {
        out.extend_from_slice(&bipolar_to_u16(amount).to_le_bytes());
    }
}

fn read_macro_amounts_c2(reader: &mut Reader) -> Result<MacroRoute, SongCodeError> {
    let mut amounts = [0.0; MACRO_COUNT];
    for amount in &mut amounts {
        *amount = u16_to_bipolar(reader.u16()?);
    }
    Ok(MacroRoute { amounts })
}

fn read_automation_c2(bytes: &[u8], automation: &mut AutomationState) -> Result<(), SongCodeError> {
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

        let (Some(spec), Some(shape)) = (control_at(index), shape_from_tag(shape_byte)) else {
            continue;
        };
        let mut route = build_lfo_route(cycle_beats, depth_ratio, shape, phase_offset_beats, seed);
        if let Some((step_count, step_glide, values)) = steps {
            route.step_count = step_count.clamp(1, MAX_LFO_STEPS as u8);
            route.step_glide = step_glide;
            route.steps = values;
        }
        automation.set_route(ControlAddress::new(spec.id), route);
    }

    let macro_count = reader.u16()?;
    for _ in 0..macro_count {
        let index = reader.u16()?;
        let route = read_macro_amounts_c2(&mut reader)?;
        if let Some(spec) = control_at(index) {
            automation.set_macro_route(ControlAddress::new(spec.id), route);
        }
    }

    let envelope_count = reader.u16()?;
    for _ in 0..envelope_count {
        let index = reader.u16()?;
        let amount = u16_to_bipolar(reader.u16()?);
        let attack_beats = reader.f32()?;
        let decay_beats = reader.f32()?;
        let trigger_tag = reader.u8()?;
        let trigger_param = reader.f32()?;

        let (Some(spec), Some(trigger)) = (
            control_at(index),
            env_trigger_from_tag(
                trigger_tag,
                finite_or(trigger_param, DEFAULT_ENV_TRIGGER_BEATS),
            ),
        ) else {
            continue;
        };
        automation.set_envelope(
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
    for _ in 0..field_macro_count {
        let key = reader.string()?;
        let route = read_macro_amounts_c2(&mut reader)?;
        automation.set_field_macro(key.to_string(), route);
    }

    Ok(())
}

/// A route, macro assignment, or envelope worth persisting. Mirrors the
/// pruning `AutomationState::close_editor` already applies in the UI, so a
/// route the editor would delete on close never round-trips through a song
/// code either.
fn automation_has_content(automation: &AutomationState) -> bool {
    automation.routes().next().is_some()
        || automation
            .macro_routes()
            .any(|(_, route)| !route.is_neutral())
        || automation
            .envelopes()
            .any(|(_, route)| route.amount.abs() > NEUTRAL_ENVELOPE_AMOUNT_EPSILON)
}

fn read_automation(bytes: &[u8], automation: &mut AutomationState) -> Result<(), SongCodeError> {
    let mut reader = Reader::new(bytes);
    let version = reader.u8()?;
    match version {
        AUTOMATION_PAYLOAD_VERSION_V2 => read_automation_v2(&mut reader, automation),
        AUTOMATION_PAYLOAD_VERSION_V3 => read_automation_v3(&mut reader, automation),
        AUTOMATION_PAYLOAD_VERSION_V4 => read_automation_v4(&mut reader, automation),
        AUTOMATION_PAYLOAD_VERSION_V5 => read_automation_v5(&mut reader, automation),
        AUTOMATION_PAYLOAD_VERSION => read_automation_v6(&mut reader, automation),
        _ => Ok(()),
    }
}

/// Legacy v2 layout: LFO routes only, no seed, no macros, no envelopes.
/// Kept so song codes authored before this change keep decoding.
fn read_automation_v2(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    let count = reader.u16()?;
    for _ in 0..count {
        let id = reader.string()?;
        let cycle_beats = reader.f32()?;
        let depth_ratio = reader.f32()?;
        let shape = reader.u8()?;
        let phase_offset_beats = reader.f32()?;

        let (Some(spec), Some(shape)) = (spec_by_id(id), shape_from_tag(shape)) else {
            continue;
        };
        automation.set_route(
            ControlAddress::new(spec.id),
            build_lfo_route(cycle_beats, depth_ratio, shape, phase_offset_beats, 0),
        );
    }
    Ok(())
}

/// v3 layout: LFO section (with seed), macro section (single target+amount
/// per address), envelope section.
fn read_automation_v3(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    read_lfo_section(reader, automation)?;
    read_legacy_macro_section(reader, automation)?;
    read_envelope_section(reader, automation)
}

/// v4 layout: identical LFO/macro/envelope sections to v3, plus a trailing
/// field-macro section (a macro stacked onto one numeric LFO field), both
/// still in the single target+amount shape superseded by v5's per-slider
/// amounts.
fn read_automation_v4(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    read_lfo_section(reader, automation)?;
    read_legacy_macro_section(reader, automation)?;
    read_envelope_section(reader, automation)?;

    let field_macro_count = reader.u16()?;
    for _ in 0..field_macro_count {
        let key = reader.string()?;
        let target = reader.u8()? as usize;
        let amount = reader.f32()?;

        if target >= MACRO_COUNT {
            continue;
        }
        automation.set_field_macro(key.to_string(), single_slot_macro_route(target, amount));
    }

    Ok(())
}

/// v5 layout: identical LFO/envelope sections, but macro and field-macro
/// sections now carry one bipolar amount per macro slider per address/key,
/// so a control (or stacked field) can ride several macros at once.
fn read_automation_v5(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    read_lfo_section(reader, automation)?;
    read_macro_env_fieldmacro_v5(reader, automation)
}

/// v6 layout: identical to v5 except each Steps LFO route carries an inline
/// staircase; the macro/envelope/field-macro tail is byte-identical to v5.
fn read_automation_v6(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    read_lfo_section_v6(reader, automation)?;
    read_macro_env_fieldmacro_v5(reader, automation)
}

/// The per-slot macro, envelope, and field-macro sections shared unchanged by
/// the v5 and v6 layouts.
fn read_macro_env_fieldmacro_v5(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    let macro_count = reader.u16()?;
    for _ in 0..macro_count {
        let id = reader.string()?;
        let route = read_macro_amounts(reader)?;
        if let Some(spec) = spec_by_id(id) {
            automation.set_macro_route(ControlAddress::new(spec.id), route);
        }
    }

    read_envelope_section(reader, automation)?;

    let field_macro_count = reader.u16()?;
    for _ in 0..field_macro_count {
        let key = reader.string()?;
        let route = read_macro_amounts(reader)?;
        automation.set_field_macro(key.to_string(), route);
    }

    Ok(())
}

/// v6 LFO section: same base fields as `read_lfo_section`, plus an inline
/// staircase (count, glide, per-step values) after any route tagged `Steps`.
/// The step-value count read always equals what the writer emitted because
/// both clamp `step_count` to `1..=MAX_LFO_STEPS`.
fn read_lfo_section_v6(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    let lfo_count = reader.u16()?;
    for _ in 0..lfo_count {
        let id = reader.string()?;
        let cycle_beats = reader.f32()?;
        let depth_ratio = reader.f32()?;
        let shape_byte = reader.u8()?;
        let phase_offset_beats = reader.f32()?;
        let seed = reader.u32()?;

        let steps = if shape_byte == LFO_SHAPE_STEPS {
            let step_count = reader.u8()?;
            let step_glide = reader.f32()?;
            let live = (step_count as usize).clamp(1, MAX_LFO_STEPS);
            let mut values = [0.0f32; MAX_LFO_STEPS];
            for value in values.iter_mut().take(live) {
                *value = finite_or(reader.f32()?, 0.0).clamp(-1.0, 1.0);
            }
            Some((
                step_count,
                finite_or(step_glide, 0.0).clamp(0.0, 1.0),
                values,
            ))
        } else {
            None
        };

        let (Some(spec), Some(shape)) = (spec_by_id(id), shape_from_tag(shape_byte)) else {
            continue;
        };
        let mut route = build_lfo_route(cycle_beats, depth_ratio, shape, phase_offset_beats, seed);
        if let Some((step_count, step_glide, values)) = steps {
            route.step_count = step_count.clamp(1, MAX_LFO_STEPS as u8);
            route.step_glide = step_glide;
            route.steps = values;
        }
        automation.set_route(ControlAddress::new(spec.id), route);
    }
    Ok(())
}

fn read_macro_amounts(reader: &mut Reader) -> Result<MacroRoute, SongCodeError> {
    let mut amounts = [0.0; MACRO_COUNT];
    for amount in &mut amounts {
        *amount = finite_or(reader.f32()?, 0.0).clamp(-1.0, 1.0);
    }
    Ok(MacroRoute { amounts })
}

/// A v3/v4-era macro assignment named one target macro slider; fold it into
/// the current per-slot representation with only that slot set.
fn single_slot_macro_route(target: usize, amount: f32) -> MacroRoute {
    let mut amounts = [0.0; MACRO_COUNT];
    if target < MACRO_COUNT {
        amounts[target] = finite_or(amount, 0.0).clamp(-1.0, 1.0);
    }
    MacroRoute { amounts }
}

/// LFO section shared by the v3 and v4 layouts (identical byte shape).
fn read_lfo_section(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    let lfo_count = reader.u16()?;
    for _ in 0..lfo_count {
        let id = reader.string()?;
        let cycle_beats = reader.f32()?;
        let depth_ratio = reader.f32()?;
        let shape = reader.u8()?;
        let phase_offset_beats = reader.f32()?;
        let seed = reader.u32()?;

        let (Some(spec), Some(shape)) = (spec_by_id(id), shape_from_tag(shape)) else {
            continue;
        };
        automation.set_route(
            ControlAddress::new(spec.id),
            build_lfo_route(cycle_beats, depth_ratio, shape, phase_offset_beats, seed),
        );
    }
    Ok(())
}

/// Shared LfoRoute construction for the song-code readers: clamps each field
/// to its valid range the same way regardless of which payload version
/// supplied it (v2 has no seed byte and always passes 0).
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
        // v2..v5 codes carry no step block; a non-Steps route ignores these,
        // and a v6 Steps route overwrites them from its inline staircase.
        ..LfoRoute::default()
    }
}

/// Macro section shared by the v3 and v4 layouts: one target macro slider
/// plus one amount per address, superseded by v5's per-slot amounts.
fn read_legacy_macro_section(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    let macro_count = reader.u16()?;
    for _ in 0..macro_count {
        let id = reader.string()?;
        let target = reader.u8()? as usize;
        let amount = reader.f32()?;

        let Some(spec) = spec_by_id(id) else {
            continue;
        };
        if target >= MACRO_COUNT {
            continue;
        }
        automation.set_macro_route(
            ControlAddress::new(spec.id),
            single_slot_macro_route(target, amount),
        );
    }
    Ok(())
}

/// Envelope section shared by the v3, v4, and v5 layouts (identical shape).
fn read_envelope_section(
    reader: &mut Reader,
    automation: &mut AutomationState,
) -> Result<(), SongCodeError> {
    let envelope_count = reader.u16()?;
    for _ in 0..envelope_count {
        let id = reader.string()?;
        let amount = reader.f32()?;
        let attack_beats = reader.f32()?;
        let decay_beats = reader.f32()?;
        let trigger_tag = reader.u8()?;
        let trigger_param = reader.f32()?;

        let (Some(spec), Some(trigger)) = (
            spec_by_id(id),
            env_trigger_from_tag(
                trigger_tag,
                finite_or(trigger_param, DEFAULT_ENV_TRIGGER_BEATS),
            ),
        ) else {
            continue;
        };
        automation.set_envelope(
            ControlAddress::new(spec.id),
            EnvelopeRoute {
                amount: finite_or(amount, 0.0).clamp(-1.0, 1.0),
                attack_beats: finite_or(attack_beats, 0.0).clamp(0.0, MAX_ENV_ATTACK_BEATS),
                decay_beats: finite_or(decay_beats, 0.0).clamp(0.0, MAX_ENV_DECAY_BEATS),
                trigger,
            },
        );
    }

    Ok(())
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

fn write_str(value: &str, out: &mut Vec<u8>) -> Result<(), SongCodeError> {
    let bytes = value.as_bytes();
    let len = u8::try_from(bytes.len()).map_err(|_| SongCodeError::TooLarge)?;
    out.push(len);
    out.extend_from_slice(bytes);
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

    fn i16(&mut self) -> Result<i16, SongCodeError> {
        Ok(i16::from_le_bytes(self.read_array()?))
    }

    fn i32(&mut self) -> Result<i32, SongCodeError> {
        Ok(i32::from_le_bytes(self.read_array()?))
    }

    fn f32(&mut self) -> Result<f32, SongCodeError> {
        Ok(f32::from_le_bytes(self.read_array()?))
    }

    fn string(&mut self) -> Result<&'a str, SongCodeError> {
        let len = self.u8()? as usize;
        let bytes = self.bytes(len)?;
        std::str::from_utf8(bytes).map_err(|_| SongCodeError::InvalidUtf8)
    }
}
