use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::{FluidControls, all_specs, spec_by_id};

const MAGIC: &[u8; 4] = b"NOOI";
const CONTAINER_VERSION: u8 = 1;
const CODE_PREFIX: &str = "n1_";
pub(crate) const SNAPSHOT_RECORD: u8 = 0;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SongCodeError {
    MissingPrefix,
    InvalidBase64,
    InvalidMagic,
    UnsupportedVersion(u8),
    Truncated,
    InvalidUtf8,
    TooLarge,
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
        }
    }
}

impl Error for SongCodeError {}

pub(crate) fn launch_line(controls: &FluidControls) -> Result<String, SongCodeError> {
    Ok(format!("nooise {}", encode_song_code(controls)?))
}

pub(crate) fn encode_song_code(controls: &FluidControls) -> Result<String, SongCodeError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.push(CONTAINER_VERSION);
    write_str(env!("CARGO_PKG_VERSION"), &mut bytes)?;

    let mut snapshot = Vec::new();
    write_snapshot(controls, &mut snapshot)?;
    write_record(SNAPSHOT_RECORD, &snapshot, &mut bytes)?;

    Ok(format!("{CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub(crate) fn decode_song_code(code: &str) -> Result<FluidControls, SongCodeError> {
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
    let version = reader.u8()?;
    if version != CONTAINER_VERSION {
        return Err(SongCodeError::UnsupportedVersion(version));
    }

    let _app_version = reader.string()?;
    let mut controls = FluidControls::default();

    while !reader.is_empty() {
        let record_type = reader.u8()?;
        let len = reader.u32()? as usize;
        let payload = reader.bytes(len)?;
        if record_type == SNAPSHOT_RECORD {
            read_snapshot(payload, &mut controls)?;
        }
    }

    Ok(controls)
}

fn write_snapshot(controls: &FluidControls, out: &mut Vec<u8>) -> Result<(), SongCodeError> {
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
        entries.push((spec.id, value));
    }

    write_u16(entries.len(), out)?;
    for (id, value) in entries {
        write_str(id, out)?;
        out.extend_from_slice(&value.to_le_bytes());
    }
    Ok(())
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

    fn u16(&mut self) -> Result<u16, SongCodeError> {
        let mut bytes = [0u8; 2];
        bytes.copy_from_slice(self.bytes(2)?);
        Ok(u16::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, SongCodeError> {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(self.bytes(4)?);
        Ok(u32::from_le_bytes(bytes))
    }

    fn f32(&mut self) -> Result<f32, SongCodeError> {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(self.bytes(4)?);
        Ok(f32::from_le_bytes(bytes))
    }

    fn string(&mut self) -> Result<&'a str, SongCodeError> {
        let len = self.u8()? as usize;
        let bytes = self.bytes(len)?;
        std::str::from_utf8(bytes).map_err(|_| SongCodeError::InvalidUtf8)
    }
}
