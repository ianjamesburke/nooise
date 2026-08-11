//! The live control values themselves: `FluidControls` and the per-voice
//! structs it aggregates, each a plain field bag with a default. Meaning,
//! range, and presentation all live in the registry, never here.

use super::module::LayerModules;

pub(crate) const MASTER_BPM_MIN: f32 = 30.0;
pub(crate) const MASTER_BPM_MAX: f32 = 200.0;
// bass.cutoff range for BassEngine's one-pole lowpass. The max is treated as
// a literal bypass in BassEngine::next (not just a high filter coefficient)
// so the default render stays byte-identical — see BassLowPass in bass.rs.
pub(crate) const BASS_CUTOFF_MIN_HZ: f32 = 80.0;
pub(crate) const BASS_CUTOFF_MAX_HZ: f32 = 8000.0;
// Short enough to feel instant under a moving slider, long enough to stay
// click-free on gain changes.
pub(crate) const LEVEL_RAMP_MS: f32 = 30.0;

#[derive(Clone)]
pub(crate) struct MasterControls {
    pub(crate) bpm: f32,
    pub(crate) level: f32,
    pub(crate) tone: f32, // -1 (bass) to +1 (treble)
    pub(crate) tune: f32, // semitones, -12 (1 octave down) to +12 (1 octave up)
}

impl Default for MasterControls {
    fn default() -> Self {
        Self {
            bpm: 82.0,
            level: 0.8,
            tone: 0.0,
            tune: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PercControls {
    pub(crate) level: f32,
    pub(crate) decay_ms: f32,
    pub(crate) filter: f32,
    pub(crate) interval_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
}

impl Default for PercControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            decay_ms: 200.0,
            filter: 0.7,
            interval_beats: 0.25,
            offset_beats: 0.0,
            swing: 0.0,
        }
    }
}

/// One slot of a custom chord progression: `degree` is the chord root as a
/// tonic-relative scale degree (diatonic steps, -7..7, spanning one octave in
/// each direction); `accidental` nudges that root a semitone flat/sharp;
/// `quality` overrides the chord's third for modal interchange (-1=force
/// minor, 0=whatever the scale gives at this degree, +1=force major);
/// `extension` picks how high the chord's top voice reaches above the triad
/// (0=triad, 1/2/3=progressively richer diatonic extensions); `inversion`
/// moves the lowest voice(s) up an octave. Only read when `PadControls`'s
/// `progression` selects the custom slot (`voice::CUSTOM_PROGRESSION_INDEX`);
/// otherwise inert.
#[derive(Clone, Default)]
pub(crate) struct ChordSlotControls {
    pub(crate) degree: f32,
    pub(crate) accidental: f32,
    pub(crate) quality: f32,
    pub(crate) extension: f32,
    pub(crate) inversion: f32,
}

/// Default per-slot root degrees for a fresh custom progression: a stepwise
/// shape around the tonic so switching into Custom mode is immediately
/// musical rather than eight identical tonic chords.
pub(crate) const DEFAULT_CHORD_SLOT_DEGREES: [f32; 8] = [0.0, -1.0, 0.0, 1.0, 0.0, -1.0, 2.0, 4.0];

/// Number of custom chord slots (`PadControls::chord_slots`), and the max of
/// `pad.chord_count`. Matches the built-ins' fixed 8-step length.
pub(crate) const CHORD_SLOT_COUNT: usize = 8;

#[derive(Clone)]
pub(crate) struct PadControls {
    pub(crate) level: f32,
    pub(crate) voice_type: f32, // 0=Warm (legacy), 1=Dark, 2=Glass character selector
    pub(crate) chord_bars: f32, // 1,2,4,8,16,32,64
    pub(crate) chord_count: f32,
    pub(crate) progression: f32,
    pub(crate) chord_slots: [ChordSlotControls; CHORD_SLOT_COUNT],
    pub(crate) stereo_width: f32,
    pub(crate) detune: f32,
    pub(crate) octave_mix: f32,
    pub(crate) attack_time: f32,
    pub(crate) release_time: f32,
}

impl Default for PadControls {
    fn default() -> Self {
        Self {
            level: 0.7,
            voice_type: 0.0,
            chord_bars: 4.0,
            chord_count: 8.0,
            progression: 0.0,
            chord_slots: std::array::from_fn(|slot| ChordSlotControls {
                degree: DEFAULT_CHORD_SLOT_DEGREES[slot],
                ..ChordSlotControls::default()
            }),
            stereo_width: 0.8,
            detune: 0.5,
            octave_mix: 0.5,
            attack_time: 6.0,
            release_time: 8.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct KickControls {
    pub(crate) level: f32,
    pub(crate) voice_type: f32, // 0=Sub (legacy), 1=Punch, 2=Membrane, 3=Driven character selector
    pub(crate) start_freq: f32,
    pub(crate) pitch_decay_ms: f32,
    pub(crate) amp_decay_ms: f32,
    pub(crate) click: f32, // 0–0.2 UI range
    pub(crate) filter: f32,
    pub(crate) interval_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
}

impl Default for KickControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            voice_type: 0.0,
            start_freq: 160.0,
            pitch_decay_ms: 55.0,
            amp_decay_ms: 250.0,
            click: 0.0,
            filter: 0.7,
            interval_beats: 1.0,
            offset_beats: 0.0,
            swing: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct TonalControls {
    pub(crate) level: f32,
    pub(crate) synth_type: f32,
    pub(crate) octave: f32,
    pub(crate) phrase: f32,
    pub(crate) randomness: f32,
    pub(crate) evolve_rate: f32,
    pub(crate) rate_beats: f32,
    pub(crate) step_interval_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) attack: f32,
    pub(crate) decay: f32,
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
}

impl Default for TonalControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            synth_type: 0.0,
            octave: 0.0,
            phrase: 0.0,
            randomness: 0.5,
            evolve_rate: 0.0,
            rate_beats: 0.5,
            step_interval_beats: 16.0,
            offset_beats: 0.0,
            // Attack + decay are the note's whole envelope: ramp in over
            // `attack`, then fall from the peak to silence over `decay`. The
            // note's sounding length is exactly `attack + decay`, decoupled
            // from the step grid, so `decay` alone sets how long each note
            // rings.
            attack: 0.03,
            decay: 1.2,
            swing: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ClapControls {
    pub(crate) level: f32,
    pub(crate) interval_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
    pub(crate) slap_count: f32, // 1-8
    pub(crate) slap_spread_ms: f32, // 0-100 ms
    pub(crate) decay_ms: f32, // 10-200 ms
    pub(crate) filter: f32, // 0=dark 1=bright
    pub(crate) body: f32,  // 0-1 low-freq flesh mix
}

impl Default for ClapControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            interval_beats: 2.0,
            offset_beats: 1.0,
            swing: 0.0,
            slap_count: 3.0,
            slap_spread_ms: 8.0,
            decay_ms: 40.0,
            filter: 0.75,
            body: 0.2,
        }
    }
}

#[derive(Clone)]
pub(crate) struct BassControls {
    pub(crate) level: f32,
    pub(crate) voice_type: f32, // 0=Sub (legacy), 1=Saw, 2=Pluck character selector
    pub(crate) interval_beats: f32, // crops the 16-step rhythm phrase to this many beats (step length is fixed)
    pub(crate) offset_beats: f32,
    pub(crate) rhythm: f32, // 0..=3, A/B/C/D pattern selector
    pub(crate) octave: f32, // octaves relative to the chord root, e.g. -1.0 = one octave down
    pub(crate) attack_time: f32,
    pub(crate) decay_time: f32, // also used as the cutoff curve when a hit retriggers mid-decay
    pub(crate) cutoff: f32, // one-pole lowpass cutoff, Hz; BASS_CUTOFF_MAX_HZ = fully open (bypass)
    pub(crate) swing: f32,  // derived from the optional Swing module
}

impl Default for BassControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            voice_type: 0.0,
            interval_beats: 4.0,
            offset_beats: 0.0,
            rhythm: 0.0,
            octave: -1.0,
            decay_time: 0.3,
            attack_time: 0.01,
            cutoff: BASS_CUTOFF_MAX_HZ,
            swing: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ArpControls {
    pub(crate) gain: f32,
    pub(crate) voice_type: f32, // same Sine/piano-profile set as tonal.synth_type
    pub(crate) rate_beats: f32,
    pub(crate) offset_beats: f32,
    pub(crate) pattern: f32, // 0=Up, 1=Down, 2=Up-Down, 3=Random
    pub(crate) octaves: f32, // 1-3, octave span of the cycled chord tones
    pub(crate) attack: f32,
    pub(crate) decay: f32,
    pub(crate) swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
}

impl Default for ArpControls {
    fn default() -> Self {
        Self {
            // Silent by default: a new voice must never change the sound of
            // existing songs or a fresh startup.
            gain: 0.0,
            // 6 => the "Pluck" piano profile, matching the arp's former
            // fixed synth character byte-for-byte.
            voice_type: 6.0,
            rate_beats: 0.5,
            offset_beats: 0.0,
            pattern: 0.0,
            octaves: 1.0,
            attack: 0.005,
            decay: 0.4,
            swing: 0.0,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct FluidControls {
    pub(crate) master: MasterControls,
    pub(crate) perc: PercControls,
    pub(crate) pad: PadControls,
    pub(crate) kick: KickControls,
    pub(crate) tonal: TonalControls,
    pub(crate) clap: ClapControls,
    pub(crate) bass: BassControls,
    pub(crate) arp: ArpControls,
    /// Per-layer module chain. Every slot defaults to empty, so this adds no
    /// audible state and prunes entirely out of a song code.
    pub(crate) modules: LayerModules,
}
