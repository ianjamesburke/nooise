// ============================================================
// Controls
// ============================================================

pub(crate) const MASTER_BPM_MIN: f32 = 60.0;
pub(crate) const MASTER_BPM_MAX: f32 = 200.0;
pub(crate) const KICK_ECHO_TIME_BEATS_MIN: f32 = 0.125;
pub(crate) const KICK_ECHO_TIME_BEATS_MAX: f32 = 2.0;
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
}

impl Default for PercControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            decay_ms: 200.0,
            filter: 0.7,
            interval_beats: 0.25,
            offset_beats: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct PadControls {
    pub level: f32,
    pub chord_bars: f32, // 1,2,4,8,16,32,64
    pub progression: f32,
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
            chord_bars: 4.0,
            progression: 0.0,
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
    pub echo_time_beats: f32,
    pub echo_filter: f32,
    pub echo_amount: f32,
    pub echo_feedback: f32,
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
            echo_time_beats: 1.0,
            echo_filter: 0.5,
            echo_amount: 0.0,
            echo_feedback: 0.0,
        }
    }
}

#[derive(Clone)]
pub(crate) struct TonalControls {
    pub level: f32,
    pub synth_type: f32,
    pub phrase: f32,
    pub randomness: f32,
    pub evolve_rate: f32,
    pub note_length_beats: f32,
    pub rate_beats: f32,
    pub step_interval_beats: f32,
    pub offset_beats: f32,
    pub reverb_mix: f32,
}

impl Default for TonalControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            synth_type: 0.0,
            phrase: 0.0,
            randomness: 0.5,
            evolve_rate: 0.0,
            note_length_beats: 1.5,
            rate_beats: 0.5,
            step_interval_beats: 16.0,
            offset_beats: 0.0,
            reverb_mix: 0.6,
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
            filter: 0.85,
            room: 0.0,
            body: 0.2,
        }
    }
}

#[derive(Clone)]
pub(crate) struct BassControls {
    pub level: f32,
    pub interval_beats: f32, // crops the 16-step rhythm phrase to this many beats (step length is fixed)
    pub offset_beats: f32,
    pub rhythm: f32, // 0..=3, A/B/C/D pattern selector
    pub octave: f32, // octaves relative to the chord root, e.g. -1.0 = one octave down
    pub attack_time: f32,
    pub decay_time: f32, // also used as the cutoff curve when a hit retriggers mid-decay
    pub drive: f32,
}

impl Default for BassControls {
    fn default() -> Self {
        Self {
            level: 0.0,
            interval_beats: 4.0,
            offset_beats: 0.0,
            rhythm: 0.0,
            octave: -1.0,
            decay_time: 0.05,
            attack_time: 0.01,
            drive: 0.15,
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
    pub macros: MacroControls,
}
