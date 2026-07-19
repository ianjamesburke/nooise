use super::*;

// ============================================================
// Kick engine
// ============================================================

pub(crate) struct KickEngine {
    pub(crate) sample_rate: f32,
    pub(crate) trigger: GridTrigger,
    pub(crate) voices: Vec<KickVoice>,
    pub(crate) rng: StdRng,
    pub(crate) telemetry: Arc<FluidTelemetry>,
}

impl KickEngine {
    pub(crate) fn new(sample_rate: f32, telemetry: Arc<FluidTelemetry>) -> Self {
        Self {
            sample_rate,
            trigger: GridTrigger::new(),
            voices: Vec::with_capacity(4),
            rng: StdRng::from_entropy(),
            telemetry,
        }
    }

    pub(crate) fn next(&mut self, c: &KickControls, timing: TimingContext) -> (f32, f32) {
        if self.trigger.pop(timing, c.interval_beats, c.offset_beats) {
            self.voices.push(KickVoice::new(
                kick_type_index(c.voice_type),
                c,
                self.sample_rate,
                &mut self.rng,
            ));
            self.telemetry.kick_pulse.fetch_add(1, Ordering::Relaxed);
        }

        let rng = &mut self.rng;
        mix_and_retain(&mut self.voices, |v| v.next(rng), KickVoice::is_done)
    }
}

/// Shared click transient + amplitude envelope + drive + pan machinery behind
/// every `kick.type` voice: a single exponential amplitude decay (also gates
/// voice life via `is_done`), an optional short noise click layered in at
/// onset, the shared soft-clip `drive_stage`, and a fixed per-voice stereo
/// pan drawn once at construction. Each variant supplies its own
/// pitch/oscillator body and filter around this; `shape` only covers the
/// parts identical across all four types. `shape` updates `amp` for the
/// *next* call after using today's value to build `s`, so moving it ahead of
/// a caller-applied filter stage never changes the sample actually returned
/// (the filter never reads `amp`).
pub(crate) struct KickVoiceCore {
    pub(crate) amp: f32,
    pub(crate) amp_decay: f32,
    pub(crate) click_remaining: u64,
    pub(crate) click_level: f32,
    pub(crate) drive: f32,
    pub(crate) pan_gains: (f32, f32),
}

impl KickVoiceCore {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let amp_tau = (c.amp_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        Self {
            amp: c.level,
            amp_decay: (-1.0 / amp_tau).exp(),
            click_remaining: (c.amp_decay_ms * 0.001 * sample_rate * 0.04).round() as u64,
            click_level: c.click,
            drive: c.drive,
            pan_gains: StereoPanner::gains(rng.gen_range(-0.15f32..0.15)),
        }
    }

    #[inline]
    pub(crate) fn shape<R: Rng>(&mut self, body: f32, rng: &mut R) -> f32 {
        let mut s = body * self.amp;
        if self.click_remaining > 0 {
            s += rng.gen_range(-1.0f32..1.0) * self.click_level * self.amp;
            self.click_remaining -= 1;
        }
        s = drive_stage(s, self.drive);
        self.amp *= self.amp_decay;
        s
    }

    pub(crate) fn is_done(&self) -> bool {
        self.amp < 0.0001
    }
}

/// `kick.type` selects the voice character used for every new kick hit.
/// Index 0 (`Sub`) is the legacy voice, unchanged and the default; switching
/// type never touches the shared trigger/scheduling path in
/// `KickEngine::next` above.
pub(crate) enum KickVoice {
    A(SubKickVoice),
    B(PunchyKickVoice),
    C(MembraneKickVoice),
    D(DrivenKickVoice),
}

impl KickVoice {
    pub(crate) fn new(
        voice_type: usize,
        c: &KickControls,
        sample_rate: f32,
        rng: &mut StdRng,
    ) -> Self {
        match voice_type {
            0 => Self::A(SubKickVoice::new(c, sample_rate, rng)),
            1 => Self::B(PunchyKickVoice::new(c, sample_rate, rng)),
            2 => Self::C(MembraneKickVoice::new(c, sample_rate, rng)),
            _ => Self::D(DrivenKickVoice::new(c, sample_rate, rng)),
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        match self {
            Self::A(voice) => voice.next(rng),
            Self::B(voice) => voice.next(rng),
            Self::C(voice) => voice.next(rng),
            Self::D(voice) => voice.next(rng),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Self::A(voice) => voice.is_done(),
            Self::B(voice) => voice.is_done(),
            Self::C(voice) => voice.is_done(),
            Self::D(voice) => voice.is_done(),
        }
    }
}

/// Type 0 (default): the original kick voice, byte-for-byte unchanged. A
/// sine carrier phase-modulated by a 2x-ratio sine modulator with decaying
/// depth (a tight FM thud), an exponential pitch glide from `start_freq` down
/// to `start_freq * 0.28`, an onset noise click, the shared drive stage, and
/// a one-pole lowpass mapped from `kick.filter`.
pub(crate) struct SubKickVoice {
    pub(crate) core: KickVoiceCore,
    pub(crate) phase: f32,
    pub(crate) mod_phase: f32,
    pub(crate) freq: f32,
    pub(crate) target_freq: f32,
    pub(crate) freq_glide: f32,
    pub(crate) fm_depth: f32,
    pub(crate) fm_depth_decay: f32,
    pub(crate) lp_state: f32,
    pub(crate) lp_coeff: f32,
    pub(crate) sample_rate: f32,
}

impl SubKickVoice {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        // FM depth decays ~3x faster than pitch for a tight transient thud
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        Self {
            core: KickVoiceCore::new(c, sample_rate, rng),
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * 0.28,
            freq_glide: 1.0 / tau,
            fm_depth: 3.5,
            fm_depth_decay: (-1.0 / fm_tau).exp(),
            lp_state: 0.0,
            lp_coeff: 10_f32.powf(c.filter * 3.0 - 2.5).clamp(0.01, 0.99),
            sample_rate,
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        if self.core.is_done() {
            return (0.0, 0.0);
        }

        self.freq += (self.target_freq - self.freq) * self.freq_glide;

        // FM: modulator at 2x carrier freq, decaying depth
        let mod_freq = self.freq * 2.0;
        self.mod_phase += TAU * mod_freq / self.sample_rate;
        if self.mod_phase >= TAU {
            self.mod_phase -= TAU;
        }
        let fm = self.mod_phase.sin() * self.fm_depth * self.freq;
        self.fm_depth *= self.fm_depth_decay;

        self.phase += TAU * (self.freq + fm) / self.sample_rate;
        if self.phase >= TAU {
            self.phase -= TAU;
        }

        let body = self.phase.sin();
        let mut s = self.core.shape(body, rng);

        self.lp_state += self.lp_coeff * (s - self.lp_state);
        s = self.lp_state;

        (s * self.core.pan_gains.0, s * self.core.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// Shallower pitch-drop ratio than Sub's fixed 0.28x, so the Punch voice's
/// body settles higher and reads punchier/less sub-heavy.
const KICK_PUNCH_PITCH_DROP_RATIO: f32 = 0.48;
/// Modulator-to-carrier ratio, brighter than Sub's fixed 2x, for a more
/// metallic FM transient.
const KICK_PUNCH_FM_MOD_RATIO: f32 = 3.0;
/// FM depth, higher than Sub's fixed 3.5, for more metallic bite.
const KICK_PUNCH_FM_DEPTH: f32 = 4.5;
/// Lowpass mapping bias: shifted up from Sub's `-2.5` so the same
/// `kick.filter` range keeps this voice's higher body frequency from being
/// over-attenuated.
const KICK_PUNCH_FILTER_BIAS: f32 = -2.0;
/// Output trim: Punch's higher fundamental and metallic FM sidebands sit in a
/// more perceptually sensitive frequency range than Sub's deep fundamental,
/// so this is trimmed down to match Sub's perceived loudness at the same
/// `kick.level`.
const KICK_PUNCH_OUTPUT_GAIN: f32 = 0.85;

/// Type 1: a punchier, more metallic character. Same FM-thud/pitch-glide
/// approach as Sub, but with a shallower pitch drop, a brighter modulator
/// ratio, and more FM depth. Shares Sub's click/drive/one-pole-lowpass/pan
/// machinery via `KickVoiceCore`, with the lowpass mapping and output gain
/// retuned for the brighter body.
pub(crate) struct PunchyKickVoice {
    pub(crate) core: KickVoiceCore,
    pub(crate) phase: f32,
    pub(crate) mod_phase: f32,
    pub(crate) freq: f32,
    pub(crate) target_freq: f32,
    pub(crate) freq_glide: f32,
    pub(crate) fm_depth: f32,
    pub(crate) fm_depth_decay: f32,
    pub(crate) lp_state: f32,
    pub(crate) lp_coeff: f32,
    pub(crate) sample_rate: f32,
}

impl PunchyKickVoice {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        Self {
            core: KickVoiceCore::new(c, sample_rate, rng),
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * KICK_PUNCH_PITCH_DROP_RATIO,
            freq_glide: 1.0 / tau,
            fm_depth: KICK_PUNCH_FM_DEPTH,
            fm_depth_decay: (-1.0 / fm_tau).exp(),
            lp_state: 0.0,
            lp_coeff: 10_f32
                .powf(c.filter * 3.0 + KICK_PUNCH_FILTER_BIAS)
                .clamp(0.01, 0.99),
            sample_rate,
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        if self.core.is_done() {
            return (0.0, 0.0);
        }

        self.freq += (self.target_freq - self.freq) * self.freq_glide;

        let mod_freq = self.freq * KICK_PUNCH_FM_MOD_RATIO;
        self.mod_phase += TAU * mod_freq / self.sample_rate;
        if self.mod_phase >= TAU {
            self.mod_phase -= TAU;
        }
        let fm = self.mod_phase.sin() * self.fm_depth * self.freq;
        self.fm_depth *= self.fm_depth_decay;

        self.phase += TAU * (self.freq + fm) / self.sample_rate;
        if self.phase >= TAU {
            self.phase -= TAU;
        }

        let body = self.phase.sin();
        let mut s = self.core.shape(body, rng) * KICK_PUNCH_OUTPUT_GAIN;

        self.lp_state += self.lp_coeff * (s - self.lp_state);
        s = self.lp_state;

        (s * self.core.pan_gains.0, s * self.core.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// `kick.filter` maps to the Membrane bandpass's center frequency across this
/// range instead of a lowpass cutoff — an exponential (perceptually even)
/// sweep from a low tom-like body up to a tighter, higher membrane resonance.
const KICK_MEMBRANE_CENTER_MIN_HZ: f32 = 150.0;
const KICK_MEMBRANE_CENTER_MAX_HZ: f32 = 800.0;
/// SVF damping factor (Chamberlin topology): lower = more resonant. Chosen
/// for an audible resonant peak without runaway self-oscillation.
const KICK_MEMBRANE_DAMP: f32 = 0.35;
/// Pitch-drop ratio, slightly shallower than Sub's 0.28x so the resonant body
/// has a clearer starting pitch to color.
const KICK_MEMBRANE_PITCH_DROP_RATIO: f32 = 0.35;
/// Output trim: unlike a lowpass, a narrow bandpass discards most of the
/// carrier's broadband energy (everything outside the resonant band), so
/// this compensates upward to keep perceived loudness comparable to
/// Sub/Punch/Driven.
const KICK_MEMBRANE_OUTPUT_GAIN: f32 = 2.6;

/// Type 2: a membrane/resonant character. Replaces Sub's one-pole lowpass
/// with a hand-rolled 2-pole resonant bandpass (Chamberlin state-variable
/// filter) centered in the 150-800Hz range via `kick.filter`, for an acoustic
/// drum-head-like mid body instead of pure sub content. Keeps the same
/// trigger/pitch-envelope/click/drive/pan structure as Sub.
pub(crate) struct MembraneKickVoice {
    pub(crate) core: KickVoiceCore,
    pub(crate) phase: f32,
    pub(crate) mod_phase: f32,
    pub(crate) freq: f32,
    pub(crate) target_freq: f32,
    pub(crate) freq_glide: f32,
    pub(crate) fm_depth: f32,
    pub(crate) fm_depth_decay: f32,
    pub(crate) svf_low: f32,
    pub(crate) svf_band: f32,
    pub(crate) svf_f: f32,
    pub(crate) sample_rate: f32,
}

impl MembraneKickVoice {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        let filter = c.filter.clamp(0.0, 1.0);
        let center_hz = KICK_MEMBRANE_CENTER_MIN_HZ
            * (KICK_MEMBRANE_CENTER_MAX_HZ / KICK_MEMBRANE_CENTER_MIN_HZ).powf(filter);
        // Chamberlin SVF frequency coefficient; clamped well below the
        // stability limit (2.0) since `center_hz` can reach 800Hz even at low
        // sample rates used in tests.
        let svf_f = (2.0 * (std::f32::consts::PI * center_hz / sample_rate).sin()).clamp(0.0, 1.9);
        Self {
            core: KickVoiceCore::new(c, sample_rate, rng),
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * KICK_MEMBRANE_PITCH_DROP_RATIO,
            freq_glide: 1.0 / tau,
            fm_depth: 3.5,
            fm_depth_decay: (-1.0 / fm_tau).exp(),
            svf_low: 0.0,
            svf_band: 0.0,
            svf_f,
            sample_rate,
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        if self.core.is_done() {
            return (0.0, 0.0);
        }

        self.freq += (self.target_freq - self.freq) * self.freq_glide;

        let mod_freq = self.freq * 2.0;
        self.mod_phase += TAU * mod_freq / self.sample_rate;
        if self.mod_phase >= TAU {
            self.mod_phase -= TAU;
        }
        let fm = self.mod_phase.sin() * self.fm_depth * self.freq;
        self.fm_depth *= self.fm_depth_decay;

        self.phase += TAU * (self.freq + fm) / self.sample_rate;
        if self.phase >= TAU {
            self.phase -= TAU;
        }

        let body = self.phase.sin();
        let mut s = self.core.shape(body, rng) * KICK_MEMBRANE_OUTPUT_GAIN;

        // Chamberlin state-variable filter, bandpass output: two running
        // integrators (`svf_low`, `svf_band`) plus a feedback resonance term
        // gated by `KICK_MEMBRANE_DAMP`.
        let high = s - self.svf_low - KICK_MEMBRANE_DAMP * self.svf_band;
        self.svf_band += self.svf_f * high;
        self.svf_low += self.svf_f * self.svf_band;
        s = self.svf_band;

        (s * self.core.pan_gains.0, s * self.core.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// Pitch-drop ratio, matching Sub's default so only the carrier waveform and
/// drive amount change for this type.
const KICK_DRIVEN_PITCH_DROP_RATIO: f32 = 0.28;
/// Drive amount is boosted well past the user-set `kick.drive`, for
/// harmonic-rich saturation intended to cut through a busy mix.
const KICK_DRIVEN_DRIVE_MULT: f32 = 1.6;
const KICK_DRIVEN_DRIVE_FLOOR: f32 = 0.35;
/// Output trim: a naive sawtooth carrier through heavy `drive_stage`
/// saturation reads louder (more high-frequency/presence energy, saturation
/// raises average level toward peak) than Sub's clean sine, so this is
/// trimmed down to match perceived loudness.
const KICK_DRIVEN_OUTPUT_GAIN: f32 = 0.68;

/// Type 3: a driven/harmonic character. Swaps Sub's sine carrier for a naive
/// (non-band-limited, consistent with this codebase's additive-approximation
/// approach elsewhere — see `bass.rs`'s Saw voice) sawtooth, run through the
/// shared `drive_stage` helper at a boosted drive amount for harmonic-rich
/// saturation. Keeps the same pitch-envelope/click/filter/pan structure as
/// Sub.
pub(crate) struct DrivenKickVoice {
    pub(crate) core: KickVoiceCore,
    pub(crate) phase: f32,
    pub(crate) mod_phase: f32,
    pub(crate) freq: f32,
    pub(crate) target_freq: f32,
    pub(crate) freq_glide: f32,
    pub(crate) fm_depth: f32,
    pub(crate) fm_depth_decay: f32,
    pub(crate) lp_state: f32,
    pub(crate) lp_coeff: f32,
    pub(crate) sample_rate: f32,
}

impl DrivenKickVoice {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        let mut core = KickVoiceCore::new(c, sample_rate, rng);
        core.drive = (c.drive * KICK_DRIVEN_DRIVE_MULT + KICK_DRIVEN_DRIVE_FLOOR).clamp(0.0, 1.0);
        Self {
            core,
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * KICK_DRIVEN_PITCH_DROP_RATIO,
            freq_glide: 1.0 / tau,
            fm_depth: 3.5,
            fm_depth_decay: (-1.0 / fm_tau).exp(),
            lp_state: 0.0,
            lp_coeff: 10_f32.powf(c.filter * 3.0 - 2.5).clamp(0.01, 0.99),
            sample_rate,
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        if self.core.is_done() {
            return (0.0, 0.0);
        }

        self.freq += (self.target_freq - self.freq) * self.freq_glide;

        let mod_freq = self.freq * 2.0;
        self.mod_phase += TAU * mod_freq / self.sample_rate;
        if self.mod_phase >= TAU {
            self.mod_phase -= TAU;
        }
        let fm = self.mod_phase.sin() * self.fm_depth * self.freq;
        self.fm_depth *= self.fm_depth_decay;

        self.phase += TAU * (self.freq + fm) / self.sample_rate;
        if self.phase >= TAU {
            self.phase -= TAU;
        }

        // Naive bipolar sawtooth from the (FM-modulated) phase.
        let body = self.phase / TAU * 2.0 - 1.0;
        let mut s = self.core.shape(body, rng) * KICK_DRIVEN_OUTPUT_GAIN;

        self.lp_state += self.lp_coeff * (s - self.lp_state);
        s = self.lp_state;

        (s * self.core.pan_gains.0, s * self.core.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}
