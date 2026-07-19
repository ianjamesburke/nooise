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

/// Shared click transient + amplitude envelope + drive + soft-attack + pan
/// machinery behind every `kick.type` voice: a single exponential amplitude
/// decay (also gates voice life via `is_done`), an optional short noise click
/// layered in at onset, the shared soft-clip `drive_stage`, an optional
/// linear fade-in that rounds off the onset transient, and a fixed per-voice
/// stereo pan drawn once at construction. Each variant supplies its own
/// pitch/oscillator body and filter around this; `shape` only covers the
/// parts identical across all four types. `shape` updates `amp` for the
/// *next* call after using today's value to build `s`, so moving it ahead of
/// a caller-applied filter stage never changes the sample actually returned
/// (the filter never reads `amp`).
///
/// Both softening parameters are per-variant and inert at their Sub values:
/// `attack_samples` 0 leaves the fade-in branch untaken, and `click_scale`
/// 1.0 is an exact f32 identity on `c.click`. Sub therefore stays
/// byte-identical to its pre-`kick.type` render (enforced by
/// `kick_type_zero_matches_legacy_sub_voice_exactly`).
pub(crate) struct KickVoiceCore {
    pub(crate) amp: f32,
    pub(crate) amp_decay: f32,
    pub(crate) click_remaining: u64,
    pub(crate) click_level: f32,
    pub(crate) drive: f32,
    pub(crate) attack_remaining: u64,
    pub(crate) attack_gain: f32,
    pub(crate) attack_inc: f32,
    pub(crate) pan_gains: (f32, f32),
}

impl KickVoiceCore {
    pub(crate) fn new(
        c: &KickControls,
        sample_rate: f32,
        rng: &mut StdRng,
        attack_ms: f32,
        click_scale: f32,
    ) -> Self {
        let amp_tau = (c.amp_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let attack_samples = (attack_ms * 0.001 * sample_rate).round().max(0.0) as u64;
        Self {
            amp: c.level,
            amp_decay: (-1.0 / amp_tau).exp(),
            click_remaining: (c.amp_decay_ms * 0.001 * sample_rate * 0.04).round() as u64,
            click_level: c.click * click_scale,
            drive: c.drive,
            attack_remaining: attack_samples,
            attack_gain: 0.0,
            attack_inc: if attack_samples == 0 {
                0.0
            } else {
                1.0 / attack_samples as f32
            },
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
        // Applied to the output sample, never to `amp`, so the decay envelope
        // math (and `is_done`) is identical with or without an attack ramp.
        if self.attack_remaining > 0 {
            s *= self.attack_gain;
            self.attack_gain = (self.attack_gain + self.attack_inc).min(1.0);
            self.attack_remaining -= 1;
        }
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
/// `KickEngine::next` above. Types 1-3 are all authored soft and textural for
/// ambient use: each takes a short onset fade-in and a scaled-down click via
/// `KickVoiceCore` so none of them reads as a drum-machine transient.
pub(crate) enum KickVoice {
    A(SubKickVoice),
    B(WarmKickVoice),
    C(WoodKickVoice),
    D(FeltKickVoice),
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
            1 => Self::B(WarmKickVoice::new(c, sample_rate, rng)),
            2 => Self::C(WoodKickVoice::new(c, sample_rate, rng)),
            _ => Self::D(FeltKickVoice::new(c, sample_rate, rng)),
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
            core: KickVoiceCore::new(c, sample_rate, rng, 0.0, 1.0),
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

/// Shallower pitch-drop ratio than Sub's fixed 0.28x, so the Warm voice's
/// body settles a little above Sub without reading as a second sub layer.
const KICK_WARM_PITCH_DROP_RATIO: f32 = 0.42;
/// Modulator-to-carrier ratio. Below Sub's fixed 2x: a 1.5 ratio places
/// sidebands at non-harmonic-series intervals that read hollow and woody
/// rather than bright. Ratios at or above 3x produce the metallic clang this
/// voice deliberately avoids.
const KICK_WARM_FM_MOD_RATIO: f32 = 1.5;
/// FM depth, well below Sub's fixed 3.5, so the modulator rounds the body out
/// instead of adding a hard transient edge.
const KICK_WARM_FM_DEPTH: f32 = 1.2;
/// Lowpass mapping bias: nudged up from Sub's `-2.5` so this voice's slightly
/// higher body isn't over-attenuated, but kept most of the way back toward
/// Sub's so it stays dark rather than bright.
const KICK_WARM_FILTER_BIAS: f32 = -2.35;
/// Linear onset fade-in. Rounds off the transient snap so the hit reads as a
/// swell into a body rather than a drum-machine attack.
const KICK_WARM_ATTACK_MS: f32 = 6.0;
/// The broadband onset noise burst is the single most aggressive-sounding
/// element of a kick; scaled well down from the user's `kick.click`.
const KICK_WARM_CLICK_SCALE: f32 = 0.45;
/// Output trim: matches Sub's perceived loudness at the same `kick.level`.
const KICK_WARM_OUTPUT_GAIN: f32 = 0.9;

/// Type 1: a warm, round FM body. Same FM-thud/pitch-glide approach as Sub,
/// but with a shallow FM depth at a hollow, woody modulator ratio, a slightly
/// shallower pitch drop, a soft attack ramp, and a scaled-down click. Shares
/// Sub's click/drive/one-pole-lowpass/pan machinery via `KickVoiceCore`.
pub(crate) struct WarmKickVoice {
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

impl WarmKickVoice {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        Self {
            core: KickVoiceCore::new(
                c,
                sample_rate,
                rng,
                KICK_WARM_ATTACK_MS,
                KICK_WARM_CLICK_SCALE,
            ),
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * KICK_WARM_PITCH_DROP_RATIO,
            freq_glide: 1.0 / tau,
            fm_depth: KICK_WARM_FM_DEPTH,
            fm_depth_decay: (-1.0 / fm_tau).exp(),
            lp_state: 0.0,
            lp_coeff: 10_f32
                .powf(c.filter * 3.0 + KICK_WARM_FILTER_BIAS)
                .clamp(0.01, 0.99),
            sample_rate,
        }
    }

    pub(crate) fn next<R: Rng>(&mut self, rng: &mut R) -> (f32, f32) {
        if self.core.is_done() {
            return (0.0, 0.0);
        }

        self.freq += (self.target_freq - self.freq) * self.freq_glide;

        let mod_freq = self.freq * KICK_WARM_FM_MOD_RATIO;
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
        let mut s = self.core.shape(body, rng) * KICK_WARM_OUTPUT_GAIN;

        self.lp_state += self.lp_coeff * (s - self.lp_state);
        s = self.lp_state;

        (s * self.core.pan_gains.0, s * self.core.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// `kick.filter` maps to the Wood bandpass's center frequency across this
/// range instead of a lowpass cutoff — an exponential (perceptually even)
/// sweep across the low-mid range where a struck wooden body resonates.
/// Deliberately narrow and low: higher centers read as a thin, hollow tom.
const KICK_WOOD_CENTER_MIN_HZ: f32 = 110.0;
const KICK_WOOD_CENTER_MAX_HZ: f32 = 400.0;
/// SVF damping factor (Chamberlin topology): lower = more resonant. Set high
/// enough that the band colors the body without the long "boingy" ring a
/// lightly-damped SVF produces.
const KICK_WOOD_DAMP: f32 = 0.9;
/// Pitch-drop ratio, slightly shallower than Sub's 0.28x so the resonant body
/// has a clearer starting pitch to color.
const KICK_WOOD_PITCH_DROP_RATIO: f32 = 0.35;
/// Bandpass/dry blend. The bandpass alone throws away the carrier's low end
/// and reads thin; blending the post-drive dry signal back in keeps the
/// weight underneath the wooden coloration.
const KICK_WOOD_BANDPASS_MIX: f32 = 0.6;
/// Linear onset fade-in, rounding off the transient snap.
const KICK_WOOD_ATTACK_MS: f32 = 5.0;
/// Scales the user's `kick.click` down; the broadband burst fights the soft
/// wooden body.
const KICK_WOOD_CLICK_SCALE: f32 = 0.35;
/// Output trim: the heavily-damped bandpass has no resonant peak to make up
/// for and sheds most of the carrier's energy, so even blended against the
/// dry signal this needs a substantial boost to sit at Sub's perceived
/// loudness.
const KICK_WOOD_OUTPUT_GAIN: f32 = 1.7;

/// Type 2: a soft wooden body. Runs the post-drive signal through a
/// hand-rolled, heavily-damped 2-pole bandpass (Chamberlin state-variable
/// filter) centered in the 110-400Hz range via `kick.filter`, then blends
/// that band back against the dry signal so the struck-wood coloration sits
/// on top of the kick's own low end rather than replacing it. Keeps the same
/// trigger/pitch-envelope/click/drive/pan structure as Sub.
pub(crate) struct WoodKickVoice {
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

impl WoodKickVoice {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        let filter = c.filter.clamp(0.0, 1.0);
        let center_hz = KICK_WOOD_CENTER_MIN_HZ
            * (KICK_WOOD_CENTER_MAX_HZ / KICK_WOOD_CENTER_MIN_HZ).powf(filter);
        // Chamberlin SVF frequency coefficient; clamped well below the
        // stability limit (2.0) since `center_hz` can reach 400Hz even at low
        // sample rates used in tests.
        let svf_f = (2.0 * (std::f32::consts::PI * center_hz / sample_rate).sin()).clamp(0.0, 1.9);
        Self {
            core: KickVoiceCore::new(
                c,
                sample_rate,
                rng,
                KICK_WOOD_ATTACK_MS,
                KICK_WOOD_CLICK_SCALE,
            ),
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * KICK_WOOD_PITCH_DROP_RATIO,
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
        let dry = self.core.shape(body, rng);

        // Chamberlin state-variable filter, bandpass output: two running
        // integrators (`svf_low`, `svf_band`) plus a feedback resonance term
        // gated by `KICK_WOOD_DAMP`.
        let high = dry - self.svf_low - KICK_WOOD_DAMP * self.svf_band;
        self.svf_band += self.svf_f * high;
        self.svf_low += self.svf_f * self.svf_band;
        let s = (self.svf_band * KICK_WOOD_BANDPASS_MIX + dry * (1.0 - KICK_WOOD_BANDPASS_MIX))
            * KICK_WOOD_OUTPUT_GAIN;

        (s * self.core.pan_gains.0, s * self.core.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}

/// Pitch-drop ratio, matching Sub's default so only the carrier waveform and
/// filter darkness change for this type.
const KICK_FELT_PITCH_DROP_RATIO: f32 = 0.28;
/// FM depth, well below Sub's fixed 3.5: the triangle carrier already brings
/// its own odd harmonics, so Sub's depth would push this into buzz.
const KICK_FELT_FM_DEPTH: f32 = 1.8;
/// Lowpass mapping bias: below Sub's `-2.5`, so the same `kick.filter` range
/// lands darker and duller — the felt-beater muffling.
const KICK_FELT_FILTER_BIAS: f32 = -2.9;
/// Linear onset fade-in, the longest of the three soft types: a felt mallet
/// compresses on contact rather than striking instantly.
const KICK_FELT_ATTACK_MS: f32 = 8.0;
/// Scales the user's `kick.click` furthest down; a felt beater has almost no
/// broadband contact noise.
const KICK_FELT_CLICK_SCALE: f32 = 0.25;
/// Output trim: matches Sub's perceived loudness at the same `kick.level`.
const KICK_FELT_OUTPUT_GAIN: f32 = 0.95;

/// Type 3: a soft mallet/felt character. Swaps Sub's sine carrier for a naive
/// (non-band-limited, consistent with this codebase's additive-approximation
/// approach elsewhere — see `bass.rs`'s Saw voice) triangle: odd harmonics
/// only, falling off as 1/n², so it thickens the body without adding edge.
/// Uses the user's `kick.drive` unmodified via `KickVoiceCore` and a darker
/// lowpass mapping than Sub. Keeps the same pitch-envelope/click/pan
/// structure as Sub.
pub(crate) struct FeltKickVoice {
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

impl FeltKickVoice {
    pub(crate) fn new(c: &KickControls, sample_rate: f32, rng: &mut StdRng) -> Self {
        let tau = (c.pitch_decay_ms * 0.001 * sample_rate / 3.0).max(1.0);
        let fm_tau = (c.pitch_decay_ms * 0.001 * sample_rate / 9.0).max(1.0);
        Self {
            core: KickVoiceCore::new(
                c,
                sample_rate,
                rng,
                KICK_FELT_ATTACK_MS,
                KICK_FELT_CLICK_SCALE,
            ),
            phase: 0.0,
            mod_phase: 0.0,
            freq: c.start_freq,
            target_freq: c.start_freq * KICK_FELT_PITCH_DROP_RATIO,
            freq_glide: 1.0 / tau,
            fm_depth: KICK_FELT_FM_DEPTH,
            fm_depth_decay: (-1.0 / fm_tau).exp(),
            lp_state: 0.0,
            lp_coeff: 10_f32
                .powf(c.filter * 3.0 + KICK_FELT_FILTER_BIAS)
                .clamp(0.01, 0.99),
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

        // Naive bipolar triangle from the (FM-modulated) phase. Written to be
        // well-defined for any phase value, since FM can push `phase` outside
        // the wrapped [0, TAU) range.
        let t = self.phase / TAU;
        let body = 4.0 * (t - (t + 0.5).floor()).abs() - 1.0;
        let mut s = self.core.shape(body, rng) * KICK_FELT_OUTPUT_GAIN;

        self.lp_state += self.lp_coeff * (s - self.lp_state);
        s = self.lp_state;

        (s * self.core.pan_gains.0, s * self.core.pan_gains.1)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.core.is_done()
    }
}
