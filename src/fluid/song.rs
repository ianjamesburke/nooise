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
    EnvelopeRoute, FluidControls, GestureEnvelope, GestureKind, GestureState, LfoRoute, LfoShape,
    MAX_ENV_ATTACK_BEATS, MAX_ENV_DECAY_BEATS, MAX_LFO_CYCLE_BEATS, MAX_LFO_OFFSET_BEATS,
    MAX_LFO_STEPS, MIN_LFO_CYCLE_BEATS, MUTE_BYTES, ModuleSlotField, MuteState, Step, TAB_COUNT,
    Tab, all_specs, parse_module_slot_id, spec_by_id,
};

const MAGIC: &[u8; 4] = b"NOOI";
/// Control ids are interned as `SONG_ID_TABLE` indexes and continuous values
/// are quantized to a u16 taper position. This is the only version axis —
/// record payloads carry no version byte of their own. Version 1 (length-
/// prefixed ids, f32 values, its own nested automation payload versions) is
/// gone; a v1 code is rejected with a message telling the user why.
pub(crate) const CONTAINER_VERSION: u8 = 2;
/// Unchanged across container versions: the CLI, `just add-morph`, and both
/// Python helpers all match song codes on this prefix.
pub(crate) const CODE_PREFIX: &str = "n1_";
pub(crate) const SNAPSHOT_RECORD: u8 = 0;
pub(crate) const AUTOMATION_RECORD: u8 = 1;
const TONAL_SEQUENCE_RECORD: u8 = 2;
const MUTE_RECORD: u8 = 3;
const GESTURE_RECORD: u8 = 4;
const GESTURE_HELD_FLAG: u8 = 1 << 0;
/// Wire tag for each LFO shape. Append-only: a tag is part of every saved
/// code that carries the shape. `shape_tag`/`shape_from_tag` are the two
/// directions of this one table.
const LFO_SHAPE_TAGS: [(LfoShape, u8); 8] = [
    (LfoShape::Sine, 0),
    (LfoShape::Triangle, 1),
    (LfoShape::RampUp, 2),
    (LfoShape::RampDown, 3),
    (LfoShape::Square, 4),
    (LfoShape::RandomDrift, 5),
    (LfoShape::SampleHold, 6),
    (LfoShape::Steps, 7),
];
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
    pub(crate) muted: MuteState,
    pub(crate) gestures: GestureState,
}

impl SongState {
    pub(crate) fn from_controls(controls: FluidControls) -> Self {
        Self {
            controls,
            automation: AutomationState::default(),
            tonal_sequence: None,
            muted: [false; TAB_COUNT],
            gestures: GestureState::default(),
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
    InvalidGestureTarget(u8),
    InvalidGestureKind(u8),
    InvalidGestureFlags(u8),
    InvalidGestureCount(u8),
    InvalidGestureAmount,
    InvalidGestureTimeAnchor {
        target: u8,
        kind: u8,
    },
    DuplicateGesture {
        target: u8,
        kind: u8,
    },
    DuplicateHeldGesture(u8),
    DuplicateGestureRecord,
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
            Self::InvalidGestureTarget(target) => {
                write!(f, "song code has unknown gesture target {target}")
            }
            Self::InvalidGestureKind(kind) => {
                write!(f, "song code has unknown gesture kind {kind}")
            }
            Self::InvalidGestureFlags(flags) => {
                write!(f, "song code has unknown gesture flags {flags:#04x}")
            }
            Self::InvalidGestureCount(count) => {
                write!(f, "song code has too many gesture entries ({count})")
            }
            Self::InvalidGestureAmount => {
                write!(f, "song code has a gesture amount outside 0..=1")
            }
            Self::InvalidGestureTimeAnchor { target, kind } => write!(
                f,
                "gesture kind {kind} on target {target} was not rebased to song time zero"
            ),
            Self::DuplicateGesture { target, kind } => write!(
                f,
                "song code repeats gesture kind {kind} on target {target}"
            ),
            Self::DuplicateHeldGesture(kind) => {
                write!(f, "song code holds gesture kind {kind} on multiple targets")
            }
            Self::DuplicateGestureRecord => write!(f, "song code repeats the gesture record"),
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
    if song.muted.iter().any(|muted| *muted) {
        write_record(MUTE_RECORD, &mute_bytes(&song.muted), &mut bytes)?;
    }
    let mut gestures = Vec::new();
    if write_gestures(&song.gestures, &mut gestures)? {
        write_record(GESTURE_RECORD, &gestures, &mut bytes)?;
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
    let mut gesture_record_seen = false;

    while !reader.is_empty() {
        let record_type = reader.u8()?;
        let len = reader.u32()? as usize;
        let payload = reader.bytes(len)?;
        match record_type {
            SNAPSHOT_RECORD => read_snapshot(payload, &mut song.controls)?,
            AUTOMATION_RECORD => read_automation(payload, &mut song.automation)?,
            TONAL_SEQUENCE_RECORD => song.tonal_sequence = Some(read_tonal_sequence(payload)?),
            MUTE_RECORD => read_mute(payload, &mut song.muted)?,
            GESTURE_RECORD => {
                if gesture_record_seen {
                    return Err(SongCodeError::DuplicateGestureRecord);
                }
                gesture_record_seen = true;
                read_gestures(payload, &mut song.gestures)?;
            }
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

/// Gesture record: `u8 entry_count`, then fixed-width entries of stable
/// `u8 target_mute_bit`, `u8 kind`, `u8 flags`, and a `u16` amount in 0..=1.
/// One quantization step is 1/65,535 of the gesture throw, well below an
/// audible envelope difference, and keeps all 36 possible lanes shareable.
fn write_gestures(gestures: &GestureState, out: &mut Vec<u8>) -> Result<bool, SongCodeError> {
    let mut entries = Vec::new();
    let mut held_kinds = BTreeSet::new();
    for tab in Tab::all() {
        for kind in GestureKind::ALL {
            let envelope = gestures.envelope(tab, kind);
            if !envelope.amount.is_finite() || !(0.0..=1.0).contains(&envelope.amount) {
                return Err(SongCodeError::InvalidGestureAmount);
            }
            if !envelope.held && envelope.amount == 0.0 {
                continue;
            }
            let target = u8::try_from(tab.mute_bit()).map_err(|_| SongCodeError::TooLarge)?;
            if envelope.at_seconds != 0.0 {
                return Err(SongCodeError::InvalidGestureTimeAnchor {
                    target,
                    kind: kind as u8,
                });
            }
            if envelope.held && !held_kinds.insert(kind as u8) {
                return Err(SongCodeError::DuplicateHeldGesture(kind as u8));
            }
            entries.push((
                target,
                kind as u8,
                u8::from(envelope.held) * GESTURE_HELD_FLAG,
                unit_to_u16(envelope.amount),
            ));
        }
    }
    if entries.is_empty() {
        return Ok(false);
    }
    let count = u8::try_from(entries.len()).map_err(|_| SongCodeError::TooLarge)?;
    out.push(count);
    for (target, kind, flags, amount) in entries {
        out.extend_from_slice(&[target, kind, flags]);
        out.extend_from_slice(&amount.to_le_bytes());
    }
    Ok(true)
}

fn read_gestures(bytes: &[u8], gestures: &mut GestureState) -> Result<(), SongCodeError> {
    let mut reader = Reader::new(bytes);
    let count = reader.u8()?;
    let max_count = TAB_COUNT * GestureKind::ALL.len();
    if count as usize > max_count {
        return Err(SongCodeError::InvalidGestureCount(count));
    }
    let expected_len = 1 + count as usize * 5;
    if bytes.len() != expected_len {
        return Err(SongCodeError::Truncated);
    }
    let mut seen = BTreeSet::new();
    let mut held_kinds = BTreeSet::new();
    for tab in Tab::all() {
        for kind in GestureKind::ALL {
            let envelope = gestures.envelope(tab, kind);
            if envelope.held || envelope.amount > 0.0 {
                seen.insert((tab.mute_bit() as u8, kind as u8));
            }
            if envelope.held {
                held_kinds.insert(kind as u8);
            }
        }
    }
    for _ in 0..count {
        let target = reader.u8()?;
        let tab = Tab::all()
            .into_iter()
            .find(|tab| tab.mute_bit() == target as usize)
            .ok_or(SongCodeError::InvalidGestureTarget(target))?;
        let kind_tag = reader.u8()?;
        let kind = GestureKind::ALL
            .into_iter()
            .find(|kind| *kind as u8 == kind_tag)
            .ok_or(SongCodeError::InvalidGestureKind(kind_tag))?;
        let flags = reader.u8()?;
        if flags & !GESTURE_HELD_FLAG != 0 {
            return Err(SongCodeError::InvalidGestureFlags(flags));
        }
        if !seen.insert((target, kind_tag)) {
            return Err(SongCodeError::DuplicateGesture {
                target,
                kind: kind_tag,
            });
        }
        let held = flags & GESTURE_HELD_FLAG != 0;
        if held && !held_kinds.insert(kind_tag) {
            return Err(SongCodeError::DuplicateHeldGesture(kind_tag));
        }
        let amount = u16_to_unit(reader.u16()?);
        gestures.lanes[tab as usize][kind as usize] = GestureEnvelope {
            amount,
            at_seconds: 0.0,
            held,
            restored: held,
        };
    }
    if !reader.is_empty() {
        return Err(SongCodeError::Truncated);
    }
    Ok(())
}

/// The mute record is a little-endian bit set indexed by `Tab::mute_bit`,
/// not by tab discriminant: a tab's bit is assigned once and never moves, so
/// a new tab in the middle of the strip cannot silently steal a saved mute
/// from the tab after it. One byte per eight bits; a code written before a
/// tab existed simply carries no bit for it, which reads as unmuted.
fn mute_bytes(muted: &MuteState) -> Vec<u8> {
    let mut bytes = vec![0u8; MUTE_BYTES];
    for tab in Tab::all() {
        if muted[tab as usize] {
            let bit = tab.mute_bit();
            bytes[bit / 8] |= 1 << (bit % 8);
        }
    }
    bytes
}

fn read_mute(bytes: &[u8], muted: &mut MuteState) -> Result<(), SongCodeError> {
    if bytes.is_empty() || bytes.len() > MUTE_BYTES {
        return Err(SongCodeError::Truncated);
    }
    for tab in Tab::all() {
        let bit = tab.mute_bit();
        muted[tab as usize] = bytes
            .get(bit / 8)
            .is_some_and(|byte| byte & (1 << (bit % 8)) != 0);
    }
    Ok(())
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
pub(crate) const VALUE_TAG_POSITION: u8 = 0;
pub(crate) const VALUE_TAG_INT: u8 = 1;
pub(crate) const VALUE_TAG_FLOAT: u8 = 2;
pub(crate) const VALUE_TAG_SMALL_INT: u8 = 3;

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
    /// `DialScale::from_step` overrides the taper for both ladders and neither
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
/// trip carries relative error, so on a large-magnitude control (a Filter
/// cutoff at 8 kHz, `perc.decay_ms` at 2 s) a value that decoded to its own default
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
            let is_structural = parse_module_slot_id(id).is_some_and(|(_, _, field)| {
                matches!(
                    field,
                    ModuleSlotField::Kind | ModuleSlotField::Clock | ModuleSlotField::RightClock
                )
            });
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
        let steps = if shape_from_tag(shape_byte) == Some(LfoShape::Steps) {
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

/// Every shape has a row (`lfo_shape_tags_cover_every_shape`), so the
/// `expect` can only fire on a table edit.
fn shape_tag(shape: LfoShape) -> u8 {
    LFO_SHAPE_TAGS
        .iter()
        .find(|(candidate, _)| *candidate == shape)
        .map(|(_, tag)| *tag)
        .expect("LFO_SHAPE_TAGS has a row for every shape")
}

fn shape_from_tag(tag: u8) -> Option<LfoShape> {
    LFO_SHAPE_TAGS
        .iter()
        .find(|(_, candidate)| *candidate == tag)
        .map(|(shape, _)| *shape)
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn write_record(record_type: u8, payload: &[u8], out: &mut Vec<u8>) -> Result<(), SongCodeError> {
    let len = u32::try_from(payload.len()).map_err(|_| SongCodeError::TooLarge)?;
    out.push(record_type);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

/// A song code whose container carries `version` and exactly `records`, so a
/// test can hand the decoder any byte sequence without re-spelling the
/// magic, prefix, or base64 layer.
#[cfg(test)]
pub(crate) fn code_from_records(version: u8, records: &[(u8, &[u8])]) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.push(version);
    for (record_type, payload) in records {
        write_record(*record_type, payload, &mut bytes).unwrap();
    }
    format!("{CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// The snapshot record payload `encode_song_code` writes for `controls`.
#[cfg(test)]
pub(crate) fn snapshot_payload(controls: &FluidControls) -> Vec<u8> {
    let mut snapshot = Vec::new();
    write_snapshot(controls, &mut snapshot).unwrap();
    snapshot
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
mod gesture_record_tests {
    use super::*;

    const AMOUNT_STEP: f32 = 1.0 / u16::MAX as f32;

    fn code_with_gesture_payload(payload: &[u8]) -> String {
        code_from_records(CONTAINER_VERSION, &[(GESTURE_RECORD, payload)])
    }

    fn entry(target: u8, kind: u8, flags: u8, amount: u16) -> [u8; 5] {
        let amount = amount.to_le_bytes();
        [target, kind, flags, amount[0], amount[1]]
    }

    fn payload(entries: &[[u8; 5]]) -> Vec<u8> {
        let mut payload = vec![entries.len() as u8];
        payload.extend(entries.iter().flatten());
        payload
    }

    #[test]
    fn inactive_gestures_add_no_song_code_payload() {
        let song = SongState::default();
        let snapshot = snapshot_payload(&song.controls);
        let expected = code_from_records(CONTAINER_VERSION, &[(SNAPSHOT_RECORD, &snapshot)]);

        assert_eq!(encode_song_code(&song).unwrap(), expected);
    }

    #[test]
    fn gesture_kind_wire_tags_are_stable() {
        assert_eq!(GestureKind::ALL.map(|kind| kind as u8), [0, 1, 2, 3]);
    }

    #[test]
    fn held_gesture_round_trip_resumes_rising_from_time_zero() {
        let mut song = SongState::default();
        song.gestures.lanes[Tab::Tonal as usize][GestureKind::Bloom as usize] = GestureEnvelope {
            amount: 0.375,
            at_seconds: 0.0,
            held: true,
            restored: false,
        };

        let decoded = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();
        let envelope = decoded.gestures.envelope(Tab::Tonal, GestureKind::Bloom);

        assert!((envelope.amount - 0.375).abs() <= AMOUNT_STEP);
        assert_eq!(envelope.at_seconds, 0.0);
        assert!(envelope.held);
        assert!(envelope.restored);
        assert!(envelope.amount_at(GestureKind::Bloom, 0.1) > envelope.amount);
    }

    #[test]
    fn zero_amount_held_gesture_is_not_omitted_as_inactive() {
        let mut song = SongState::default();
        song.gestures.lanes[Tab::Perc as usize][GestureKind::Thin as usize].held = true;

        let decoded = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();
        let envelope = decoded.gestures.envelope(Tab::Perc, GestureKind::Thin);

        assert!(envelope.held);
        assert!(envelope.restored);
    }

    #[test]
    fn returning_gesture_round_trip_resumes_returning_from_time_zero() {
        let mut song = SongState::default();
        song.gestures.lanes[Tab::Kick as usize][GestureKind::Submerge as usize] = GestureEnvelope {
            amount: 0.625,
            at_seconds: 0.0,
            held: false,
            restored: false,
        };

        let decoded = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();
        let envelope = decoded.gestures.envelope(Tab::Kick, GestureKind::Submerge);

        assert!((envelope.amount - 0.625).abs() <= AMOUNT_STEP);
        assert_eq!(envelope.at_seconds, 0.0);
        assert!(!envelope.held);
        assert!(!envelope.restored);
        assert!(envelope.amount_at(GestureKind::Submerge, 0.1) < envelope.amount);
    }

    #[test]
    fn returning_and_held_instances_of_one_kind_round_trip_on_different_targets() {
        let mut song = SongState::default();
        song.gestures.lanes[Tab::Chords as usize][GestureKind::Echo as usize] = GestureEnvelope {
            amount: 0.7,
            at_seconds: 0.0,
            held: false,
            restored: false,
        };
        song.gestures.lanes[Tab::Master as usize][GestureKind::Echo as usize] = GestureEnvelope {
            amount: 0.2,
            at_seconds: 0.0,
            held: true,
            restored: false,
        };

        let decoded = decode_song_code(&encode_song_code(&song).unwrap()).unwrap();

        assert!(
            decoded
                .gestures
                .envelope(Tab::Chords, GestureKind::Echo)
                .amount
                > 0.0
        );
        assert_eq!(
            decoded.gestures.held_target(GestureKind::Echo),
            Some(Tab::Master)
        );
    }

    #[test]
    fn gesture_target_wire_id_uses_the_stable_mute_bit() {
        let payload = payload(&[entry(
            Tab::Master.mute_bit() as u8,
            GestureKind::Thin as u8,
            0,
            unit_to_u16(0.5),
        )]);

        let decoded = decode_song_code(&code_with_gesture_payload(&payload)).unwrap();

        assert!(
            decoded
                .gestures
                .envelope(Tab::Master, GestureKind::Thin)
                .amount
                > 0.0
        );
        assert_eq!(
            decoded
                .gestures
                .envelope(Tab::Lead, GestureKind::Thin)
                .amount,
            0.0
        );
    }

    #[test]
    fn gesture_record_rejects_unknown_target() {
        let payload = payload(&[entry(255, GestureKind::Bloom as u8, 0, 1)]);
        assert_eq!(
            decode_song_code(&code_with_gesture_payload(&payload)).err(),
            Some(SongCodeError::InvalidGestureTarget(255))
        );
    }

    #[test]
    fn gesture_record_rejects_unknown_kind() {
        let payload = payload(&[entry(Tab::Chords.mute_bit() as u8, 255, 0, 1)]);
        assert_eq!(
            decode_song_code(&code_with_gesture_payload(&payload)).err(),
            Some(SongCodeError::InvalidGestureKind(255))
        );
    }

    #[test]
    fn gesture_record_rejects_unknown_flags() {
        let payload = payload(&[entry(
            Tab::Chords.mute_bit() as u8,
            GestureKind::Bloom as u8,
            0b10,
            1,
        )]);
        assert_eq!(
            decode_song_code(&code_with_gesture_payload(&payload)).err(),
            Some(SongCodeError::InvalidGestureFlags(0b10))
        );
    }

    #[test]
    fn gesture_record_rejects_more_entries_than_fixed_storage() {
        let payload = [37];
        assert_eq!(
            decode_song_code(&code_with_gesture_payload(&payload)).err(),
            Some(SongCodeError::InvalidGestureCount(37))
        );
    }

    #[test]
    fn gesture_record_rejects_duplicate_target_and_kind() {
        let duplicate = entry(Tab::Chords.mute_bit() as u8, GestureKind::Bloom as u8, 0, 1);
        let payload = payload(&[duplicate, duplicate]);
        assert_eq!(
            decode_song_code(&code_with_gesture_payload(&payload)).err(),
            Some(SongCodeError::DuplicateGesture {
                target: Tab::Chords.mute_bit() as u8,
                kind: GestureKind::Bloom as u8,
            })
        );
    }

    #[test]
    fn gesture_record_rejects_multiple_held_targets_for_one_kind() {
        let payload = payload(&[
            entry(
                Tab::Chords.mute_bit() as u8,
                GestureKind::Bloom as u8,
                GESTURE_HELD_FLAG,
                1,
            ),
            entry(
                Tab::Perc.mute_bit() as u8,
                GestureKind::Bloom as u8,
                GESTURE_HELD_FLAG,
                1,
            ),
        ]);
        assert_eq!(
            decode_song_code(&code_with_gesture_payload(&payload)).err(),
            Some(SongCodeError::DuplicateHeldGesture(
                GestureKind::Bloom as u8
            ))
        );
    }

    #[test]
    fn song_code_rejects_duplicate_gesture_records() {
        let payload = payload(&[]);
        let code = code_from_records(
            CONTAINER_VERSION,
            &[(GESTURE_RECORD, &payload), (GESTURE_RECORD, &payload)],
        );

        assert_eq!(
            decode_song_code(&code).err(),
            Some(SongCodeError::DuplicateGestureRecord)
        );
    }

    #[test]
    fn gesture_record_rejects_a_truncated_entry() {
        let truncated = [1, Tab::Chords.mute_bit() as u8, GestureKind::Bloom as u8];
        assert_eq!(
            decode_song_code(&code_with_gesture_payload(&truncated)).err(),
            Some(SongCodeError::Truncated)
        );
    }

    #[test]
    fn gesture_record_rejects_trailing_bytes() {
        let trailing = [0, 0];
        assert_eq!(
            decode_song_code(&code_with_gesture_payload(&trailing)).err(),
            Some(SongCodeError::Truncated)
        );
    }

    #[test]
    fn gesture_writer_rejects_non_finite_or_out_of_range_amounts() {
        for amount in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            let mut song = SongState::default();
            song.gestures.lanes[Tab::Chords as usize][GestureKind::Thin as usize].amount = amount;
            assert_eq!(
                encode_song_code(&song).err(),
                Some(SongCodeError::InvalidGestureAmount)
            );
        }
    }

    #[test]
    fn gesture_writer_rejects_an_active_envelope_not_rebased_to_time_zero() {
        let mut song = SongState::default();
        song.gestures.lanes[Tab::Kick as usize][GestureKind::Echo as usize] = GestureEnvelope {
            amount: 0.5,
            at_seconds: 42.0,
            held: false,
            restored: false,
        };

        assert_eq!(
            encode_song_code(&song).err(),
            Some(SongCodeError::InvalidGestureTimeAnchor {
                target: Tab::Kick.mute_bit() as u8,
                kind: GestureKind::Echo as u8,
            })
        );
    }

    #[test]
    fn maximum_gesture_record_stays_below_280_characters() {
        let mut song = SongState::default();
        for tab in Tab::all() {
            for kind in GestureKind::ALL {
                song.gestures.lanes[tab as usize][kind as usize].amount = 1.0;
            }
        }

        let code = encode_song_code(&song).unwrap();

        assert!(code.len() < 280, "max gesture code is {} chars", code.len());
    }
}

#[cfg(test)]
mod retired_control_tests {
    use super::*;

    /// A one-entry snapshot payload setting id-table slot `index`.
    fn snapshot_entry(index: u16) -> Vec<u8> {
        let mut snapshot = Vec::new();
        write_u16(1usize, &mut snapshot).unwrap();
        snapshot.extend_from_slice(&index.to_le_bytes());
        EncodedValue::Position(unit_to_u16(0.35)).write(&mut snapshot);
        snapshot
    }

    /// One snapshot entry naming `id`, wrapped in a valid container.
    fn code_setting(id: &str) -> String {
        let index = song_id_index(id).expect("id is in the table");
        code_from_records(
            CONTAINER_VERSION,
            &[(SNAPSHOT_RECORD, &snapshot_entry(index))],
        )
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

    #[test]
    fn a_code_setting_a_retired_filter_control_is_refused() {
        assert_eq!(
            decode_song_code(&code_setting("bass.cutoff")).err(),
            Some(SongCodeError::RetiredControl("bass.cutoff"))
        );
        assert_eq!(
            decode_song_code(&code_setting("kick.filter")).err(),
            Some(SongCodeError::RetiredControl("kick.filter"))
        );
    }

    /// The same skip that makes a retired id fatal must not catch an id from
    /// a newer build: that index is past the table's end, not inside it.
    #[test]
    fn a_code_setting_an_id_this_build_has_never_heard_of_still_loads() {
        let code = code_from_records(
            CONTAINER_VERSION,
            &[(SNAPSHOT_RECORD, &snapshot_entry(u16::MAX))],
        );

        assert!(decode_song_code(&code).is_ok());
    }
}

#[cfg(test)]
mod tag_tests {
    use super::*;

    #[test]
    fn lfo_shape_tags_cover_every_shape() {
        let mut tags = BTreeSet::new();
        for shape in LfoShape::ALL {
            let tag = shape_tag(shape);
            assert_eq!(shape_from_tag(tag), Some(shape));
            assert!(tags.insert(tag), "{shape:?}: tag {tag} reused");
        }
        assert_eq!(tags.len(), LFO_SHAPE_TAGS.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A code written when the strip ended at Master carried one mute byte
    /// with Master on bit 7. Lead's bit is 8, in the second byte, so that
    /// old payload still lands on Master and Lead reads as unmuted.
    #[test]
    fn a_one_byte_mute_payload_from_before_lead_still_names_master() {
        let mut muted = [false; TAB_COUNT];
        read_mute(&[1 << 7 | 1 << 4], &mut muted).unwrap();
        assert!(muted[Tab::Master as usize]);
        assert!(muted[Tab::Tonal as usize]);
        assert!(!muted[Tab::Lead as usize]);
        assert_eq!(read_mute(&[], &mut muted), Err(SongCodeError::Truncated));
        assert_eq!(
            read_mute(&[0, 0, 0], &mut muted),
            Err(SongCodeError::Truncated)
        );
    }
}
