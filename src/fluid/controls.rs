// ============================================================
// Controls
// ============================================================

pub(crate) const MASTER_BPM_MIN: f32 = 60.0;
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
    pub bpm: f32,
    pub level: f32,
    pub drive: f32,
    pub comp_threshold: f32,  // dB, -40 to 0
    pub comp_ratio: f32,      // 1-8
    pub comp_release_ms: f32, // 10-500
    pub tone: f32,            // -1 (bass) to +1 (treble)
    pub tune: f32,            // semitones, -12 (1 octave down) to +12 (1 octave up)
}

impl Default for MasterControls {
    fn default() -> Self {
        Self {
            bpm: 82.0,
            level: 0.8,
            drive: 0.1,
            comp_threshold: -8.0,
            comp_ratio: 2.0,
            comp_release_ms: 100.0,
            tone: 0.0,
            tune: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PercControls {
    pub level: f32,
    pub decay_ms: f32,
    pub filter: f32,
    pub interval_beats: f32,
    pub offset_beats: f32,
    pub swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
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
/// `extension` picks how high the chord's top voice reaches above the triad
/// (0=triad, 1/2/3=progressively richer diatonic extensions); `inversion`
/// moves the lowest voice(s) up an octave. Only read when `PadControls`'s
/// `progression` selects the custom slot (`voice::CUSTOM_PROGRESSION_INDEX`);
/// otherwise inert.
#[derive(Clone, Default)]
pub(crate) struct ChordSlotControls {
    pub degree: f32,
    pub accidental: f32,
    pub extension: f32,
    pub inversion: f32,
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
    pub level: f32,
    pub voice_type: f32, // 0=Warm (legacy), 1=Dark, 2=Glass character selector
    pub chord_bars: f32, // 1,2,4,8,16,32,64
    pub chord_count: f32,
    pub progression: f32,
    pub chord_slots: [ChordSlotControls; CHORD_SLOT_COUNT],
    pub reverb_mix: f32,
    pub stereo_width: f32,
    pub detune: f32,
    pub octave_mix: f32,
    pub attack_time: f32,
    pub release_time: f32,
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
            reverb_mix: 0.8,
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
    pub level: f32,
    pub start_freq: f32,
    pub pitch_decay_ms: f32,
    pub amp_decay_ms: f32,
    pub click: f32, // 0–0.2 UI range
    pub drive: f32,
    pub filter: f32,
    pub interval_beats: f32,
    pub offset_beats: f32,
}

impl Default for KickControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            start_freq: 160.0,
            pitch_decay_ms: 55.0,
            amp_decay_ms: 250.0,
            click: 0.0,
            drive: 0.2,
            filter: 0.7,
            interval_beats: 1.0,
            offset_beats: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct TonalControls {
    pub level: f32,
    pub synth_type: f32,
    pub octave: f32,
    pub phrase: f32,
    pub randomness: f32,
    pub evolve_rate: f32,
    pub rate_beats: f32,
    pub step_interval_beats: f32,
    pub offset_beats: f32,
    pub attack: f32,
    pub decay: f32,
    pub reverb_mix: f32,
    pub swing: f32, // 0 (straight) to 1 (max shuffle) on this voice's grid
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
            reverb_mix: 0.6,
            swing: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ClapControls {
    pub level: f32,
    pub interval_beats: f32,
    pub offset_beats: f32,
    pub slap_count: f32,     // 1-8
    pub slap_spread_ms: f32, // 0-100 ms
    pub decay_ms: f32,       // 10-200 ms
    pub filter: f32,         // 0=dark 1=bright
    pub room: f32,           // 0-1 reverb send
    pub body: f32,           // 0-1 low-freq flesh mix
}

impl Default for ClapControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            interval_beats: 2.0,
            offset_beats: 1.0,
            slap_count: 3.0,
            slap_spread_ms: 8.0,
            decay_ms: 40.0,
            filter: 0.75,
            room: 0.0,
            body: 0.2,
        }
    }
}

#[derive(Clone)]
pub(crate) struct BassControls {
    pub level: f32,
    pub voice_type: f32, // 0=Sub (legacy), 1=Saw, 2=Pluck character selector
    pub interval_beats: f32, // crops the 16-step rhythm phrase to this many beats (step length is fixed)
    pub offset_beats: f32,
    pub rhythm: f32, // 0..=3, A/B/C/D pattern selector
    pub octave: f32, // octaves relative to the chord root, e.g. -1.0 = one octave down
    pub attack_time: f32,
    pub decay_time: f32, // also used as the cutoff curve when a hit retriggers mid-decay
    pub drive: f32,
    pub cutoff: f32, // one-pole lowpass cutoff, Hz; BASS_CUTOFF_MAX_HZ = fully open (bypass)
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
            drive: 0.15,
            cutoff: BASS_CUTOFF_MAX_HZ,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ArpControls {
    pub gain: f32,
    pub voice_type: f32, // same Sine/piano-profile set as tonal.synth_type
    pub rate_beats: f32,
    pub offset_beats: f32,
    pub pattern: f32,   // 0=Up, 1=Down, 2=Up-Down, 3=Random
    pub octaves: f32,   // 1-3, octave span of the cycled chord tones
    pub attack: f32,
    pub decay: f32,
    pub reverb_mix: f32,
    pub swing: f32,     // 0 (straight) to 1 (max shuffle) on this voice's grid
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
            // Matches the former AMBIENT_REVERB_ARP_MIX_FIXED constant so the
            // default sound is unchanged.
            reverb_mix: 0.5,
            swing: 0.0,
        }
    }
}

pub(crate) const MACRO_COUNT: usize = 4;

/// The macro sliders: bare 0..1 values with no direct audio path. They only
/// matter through macro routes, which scale a route's amount into its target
/// control's range.
#[derive(Clone, Default)]
pub(crate) struct MacroControls {
    pub values: [f32; MACRO_COUNT],
}

#[derive(Clone, Default)]
pub(crate) struct FluidControls {
    pub master: MasterControls,
    pub perc: PercControls,
    pub pad: PadControls,
    pub kick: KickControls,
    pub tonal: TonalControls,
    pub clap: ClapControls,
    pub bass: BassControls,
    pub arp: ArpControls,
    pub macros: MacroControls,
}
