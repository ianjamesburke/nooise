//! Stereo saturation used by every Drive module slot: a character curve
//! (`DriveType`), an input bias for even harmonics, and a dry blend.
//!
//! Soft with no Bias is the original stateless curve, kept bit-for-bit so
//! untouched Drives render exactly as before. Every other setting runs
//! through first-order antiderivative antialiasing (ADAA), which Hard's and
//! Fold's corners need most: the output
//! is the curve's antiderivative differenced across one sample, which cancels
//! most of the aliasing a corner folds back below Nyquist without
//! oversampling (Parker, Zavalishin, Le Bivic, "Reducing the Aliasing of
//! Nonlinear Waveshaping Using Continuous-Time Convolution", DAFx-16).

/// Bias at 100% shifts the driven signal by this much before the curve, which
/// is where its second harmonic sits about 14 dB under the fundamental.
const BIAS_MAX_OFFSET: f32 = 0.5;

/// A changed Type crossfades between the two curves over this window instead
/// of jumping, which would click on a sustained bass.
const TYPE_FADE_SECONDS: f32 = 0.01;

/// Corner frequency of the wet path's DC blocker. A biased curve's offset
/// follows the signal's envelope, so it cannot be subtracted as a constant.
const DC_BLOCK_HZ: f32 = 5.0;

/// Below this sample-to-sample input change ADAA's difference quotient loses
/// precision, so the curve is evaluated directly at the midpoint.
const ADAA_MIN_STEP: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DriveType {
    /// `x / (1 + |x|)`: the original rounded saturation.
    Soft,
    /// Clip at ±1: flat-topped and buzzy.
    Hard,
    /// Triangle wavefolder: past ±1 the signal folds back, adding upper
    /// harmonics that rise and fall as the input level moves.
    Fold,
}

/// Dial order. Append-only: song codes store a slot's Type as this index.
pub(crate) const DRIVE_TYPES: [DriveType; 3] = [DriveType::Soft, DriveType::Hard, DriveType::Fold];

impl DriveType {
    pub(crate) fn from_value(value: f32) -> Self {
        DRIVE_TYPES[(value.round() as i64).clamp(0, DRIVE_TYPES.len() as i64 - 1) as usize]
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::Hard => "Hard",
            Self::Fold => "Fold",
        }
    }

    /// Output trim matching each curve's rendered level to Soft's across the
    /// Amount range (`every_drive_type_renders_near_soft_level`).
    fn trim(self) -> f32 {
        match self {
            Self::Soft => 1.0,
            Self::Hard => 0.68,
            Self::Fold => 0.84,
        }
    }

    fn shape(self, u: f64) -> f64 {
        match self {
            Self::Soft => u / (1.0 + u.abs()),
            Self::Hard => u.clamp(-1.0, 1.0),
            Self::Fold => 1.0 - 4.0 * (((u + 1.0) * 0.25).rem_euclid(1.0) - 0.5).abs(),
        }
    }

    /// Antiderivative of `shape`, used by ADAA.
    fn antiderivative(self, u: f64) -> f64 {
        match self {
            Self::Soft => u.abs() - (1.0 + u.abs()).ln(),
            Self::Hard => {
                if u.abs() <= 1.0 {
                    0.5 * u * u
                } else {
                    u.abs() - 0.5
                }
            }
            // The fold has period 4 and zero mean, so its antiderivative is
            // periodic: a parabola over the rising edge [-1, 1] and its
            // mirror over the falling edge [1, 3].
            Self::Fold => {
                let w = u - 4.0 * ((u + 1.0) * 0.25).floor();
                if w <= 1.0 {
                    0.5 * w * w
                } else {
                    2.0 * w - 0.5 * w * w - 1.0
                }
            }
        }
    }

    /// The curve at `u` with first-order ADAA against the previous input.
    pub(crate) fn adaa(self, u: f64, previous: f64) -> f64 {
        let step = u - previous;
        if step.abs() < ADAA_MIN_STEP {
            self.shape(0.5 * (u + previous))
        } else {
            (self.antiderivative(u) - self.antiderivative(previous)) / step
        }
    }
}

/// Everything about a Drive that can change per sample.
pub(crate) struct DriveParams {
    pub(crate) amount: f32,
    pub(crate) drive_type: DriveType,
    /// 0..1, the share of `BIAS_MAX_OFFSET` added before the curve.
    pub(crate) bias: f32,
    /// 0..1 crossfade back to the clean input; 0 is fully driven.
    pub(crate) dry: f32,
}

#[derive(Default)]
struct ChannelState {
    /// Last sample's curve input, for ADAA. `None` after a bypass, so the
    /// first driven sample is evaluated directly instead of differenced
    /// against a stale value.
    previous: Option<f64>,
    dc_input: f32,
    dc_output: f32,
}

impl ChannelState {
    fn drive(
        &mut self,
        x: f32,
        params: &DriveParams,
        fading_from: Option<(DriveType, f32)>,
        dc_block: Option<f32>,
    ) -> f32 {
        let amount = params.amount.clamp(0.0, 1.0) * 0.5;
        let gain = 1.0 + amount * 8.0;
        let makeup = 1.0 / (1.0 + amount);
        let bias = f64::from(params.bias.clamp(0.0, 1.0) * BIAS_MAX_OFFSET);
        let u = f64::from(x * gain) + bias;
        let previous = self.previous.replace(u).unwrap_or(u);
        let curve = |kind: DriveType| {
            ((kind.adaa(u, previous) - kind.shape(bias)) as f32) * makeup * kind.trim()
        };
        let mut wet = curve(params.drive_type);
        if let Some((from, weight)) = fading_from {
            wet += (curve(from) - wet) * weight;
        }
        if let Some(coefficient) = dc_block {
            let blocked = wet - self.dc_input + coefficient * self.dc_output;
            self.dc_input = wet;
            self.dc_output = blocked;
            wet = blocked;
        }
        wet + (x - wet) * params.dry.clamp(0.0, 1.0)
    }
}

/// One Drive slot's processor. Holds only ADAA history, the DC blocker, and
/// an in-flight Type crossfade: nothing that outlives a second of playback.
pub(crate) struct StereoDrive {
    left: ChannelState,
    right: ChannelState,
    drive_type: DriveType,
    fading_from: DriveType,
    fade_remaining: u32,
    fade_total: u32,
    dc_coefficient: f32,
    /// Set the first time this processor runs anything but unbiased Soft, and
    /// never cleared: switching the blocker back out would step the phase of
    /// whatever is sounding.
    dc_engaged: bool,
}

impl StereoDrive {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            left: ChannelState::default(),
            right: ChannelState::default(),
            drive_type: DriveType::Soft,
            fading_from: DriveType::Soft,
            fade_remaining: 0,
            fade_total: (TYPE_FADE_SECONDS * sample_rate).max(1.0) as u32,
            dc_coefficient: (-std::f32::consts::TAU * DC_BLOCK_HZ / sample_rate).exp(),
            dc_engaged: false,
        }
    }

    pub(crate) fn process(&mut self, input: (f32, f32), params: DriveParams) -> (f32, f32) {
        if params.drive_type != self.drive_type {
            self.fading_from = self.drive_type;
            self.drive_type = params.drive_type;
            self.fade_remaining = self.fade_total;
        }
        if params.amount.clamp(0.0, 1.0) * 0.5 <= f32::EPSILON {
            self.left.previous = None;
            self.right.previous = None;
            return input;
        }
        self.dc_engaged |= params.drive_type != DriveType::Soft || params.bias > 0.0;
        if !self.dc_engaged {
            return soft(input, params.amount);
        }
        let fading_from = (self.fade_remaining > 0).then(|| {
            self.fade_remaining -= 1;
            (
                self.fading_from,
                self.fade_remaining as f32 / self.fade_total as f32,
            )
        });
        let dc_block = Some(self.dc_coefficient);
        (
            self.left.drive(input.0, &params, fading_from, dc_block),
            self.right.drive(input.1, &params, fading_from, dc_block),
        )
    }
}

/// The original stateless curve, with a dry blend.
///
/// The knob's full 0..1 range only ever reaches the old curve's 0..0.5 half —
/// what used to sit at 50% is now the new maximum — so the same slider travel
/// covers half the old drive range at twice the resolution. Output gain is
/// then pulled back down as drive increases to compensate for the loudness a
/// saturator adds by its nature, rather than stacking a boost on top of it.
fn soft(input: (f32, f32), amount: f32) -> (f32, f32) {
    let amount = amount.clamp(0.0, 1.0) * 0.5;
    let makeup = 1.0 / (1.0 + amount);
    let drive = |sample: f32| {
        let driven = sample * (1.0 + amount * 8.0);
        driven / (1.0 + driven.abs()) * makeup
    };
    (drive(input.0), drive(input.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;

    fn params(amount: f32, drive_type: DriveType) -> DriveParams {
        DriveParams {
            amount,
            drive_type,
            bias: 0.0,
            dry: 0.0,
        }
    }

    fn render(
        drive: &mut StereoDrive,
        amplitude: f32,
        hz: f32,
        frames: usize,
        p: impl Fn() -> DriveParams,
    ) -> Vec<f32> {
        (0..frames)
            .map(|n| {
                let x = amplitude * (std::f32::consts::TAU * hz * n as f32 / SAMPLE_RATE).sin();
                drive.process((x, x), p()).0
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn zero_amount_is_an_exact_stereo_bypass_for_every_setting() {
        for drive_type in DRIVE_TYPES {
            let mut drive = StereoDrive::new(SAMPLE_RATE);
            let p = DriveParams {
                amount: 0.0,
                drive_type,
                bias: 1.0,
                dry: 0.5,
            };
            assert_eq!(drive.process((0.4, -0.2), p), (0.4, -0.2));
        }
    }

    #[test]
    fn unbiased_soft_keeps_the_original_curve_bit_for_bit() {
        let mut drive = StereoDrive::new(SAMPLE_RATE);
        for (input, amount) in [((0.6, -0.3), 1.0), ((0.5, 0.25), 0.75), ((-0.9, 0.1), 0.15)] {
            let a = amount * 0.5;
            let legacy = |x: f32| {
                let driven = x * (1.0 + a * 8.0);
                driven / (1.0 + driven.abs()) * (1.0 / (1.0 + a))
            };
            assert_eq!(
                drive.process(input, params(amount, DriveType::Soft)),
                (legacy(input.0), legacy(input.1))
            );
        }
    }

    #[test]
    fn volume_compensation_keeps_heavier_drive_no_louder_than_lighter_drive() {
        let mut drive = StereoDrive::new(SAMPLE_RATE);
        let light = drive.process((0.8, 0.0), params(0.2, DriveType::Soft)).0;
        let heavy = drive.process((0.8, 0.0), params(1.0, DriveType::Soft)).0;
        assert!(heavy.abs() <= light.abs());
    }

    #[test]
    fn every_drive_type_renders_near_soft_level() {
        // Fold's level swings as the input crosses fold points, which is its
        // character; it is held to a wider window than Hard.
        for (drive_type, tolerance_db) in [(DriveType::Hard, 2.5), (DriveType::Fold, 4.0)] {
            for amplitude in [0.15, 0.3, 0.5, 0.8] {
                for amount in [0.1, 0.25, 0.5, 0.75, 1.0] {
                    let level = |kind| {
                        let mut drive = StereoDrive::new(SAMPLE_RATE);
                        rms(&render(&mut drive, amplitude, 110.0, 12_000, || {
                            params(amount, kind)
                        })[2_000..])
                    };
                    let difference = 20.0 * (level(drive_type) / level(DriveType::Soft)).log10();
                    assert!(
                        difference.abs() < tolerance_db,
                        "{drive_type:?} at {amplitude}/{amount}: {difference:.1} dB"
                    );
                }
            }
        }
    }

    #[test]
    fn antialiasing_cuts_hard_clip_aliases_by_twenty_decibels() {
        // A 2.1 kHz tone hard-clipped at 48 kHz: its 23rd, 69th and 45th
        // harmonics fold back to 300, 900 and 1500 Hz, where the clipped
        // spectrum itself has nothing.
        let hz = 2_100.0;
        let frames = 16_384;
        let mut drive = StereoDrive::new(SAMPLE_RATE);
        let adaa = render(&mut drive, 0.8, hz, frames, || params(1.0, DriveType::Hard));
        let naive: Vec<f32> = (0..frames)
            .map(|n| {
                let x = 0.8 * (std::f32::consts::TAU * hz * n as f32 / SAMPLE_RATE).sin();
                (x * 5.0).clamp(-1.0, 1.0) / 1.5 * 0.68
            })
            .collect();
        // Hann-windowed DFT power at one frequency, past the DC blocker's
        // settling; the window keeps the fundamental from leaking in.
        let power = |samples: &[f32], at: f32| {
            let tail = &samples[4_096..];
            let len = tail.len() as f32;
            let (re, im) = tail
                .iter()
                .enumerate()
                .fold((0.0f64, 0.0f64), |(re, im), (n, s)| {
                    let window = 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / len).cos();
                    let phase = f64::from(std::f32::consts::TAU * at * n as f32 / SAMPLE_RATE);
                    let sample = f64::from(s * window);
                    (re + sample * phase.cos(), im + sample * phase.sin())
                });
            re * re + im * im
        };
        for alias in [300.0, 900.0, 1_500.0] {
            let reduction = power(&adaa, alias) / power(&naive, alias);
            assert!(
                reduction < 0.01,
                "{alias} Hz alias only fell to {reduction}"
            );
        }
    }

    #[test]
    fn bias_adds_a_second_harmonic_without_leaving_dc() {
        let mut drive = StereoDrive::new(SAMPLE_RATE);
        let out = render(&mut drive, 0.4, 110.0, 96_000, || DriveParams {
            bias: 1.0,
            ..params(0.5, DriveType::Soft)
        });
        let tail = &out[48_000..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 1e-3, "{mean}");
        let harmonic = |hz: f32| {
            let (re, im) = tail
                .iter()
                .enumerate()
                .fold((0.0f32, 0.0f32), |(re, im), (n, s)| {
                    let phase = std::f32::consts::TAU * hz * n as f32 / SAMPLE_RATE;
                    (re + s * phase.cos(), im + s * phase.sin())
                });
            (re * re + im * im).sqrt()
        };
        assert!(harmonic(220.0) > harmonic(110.0) * 0.1);
    }

    #[test]
    fn dry_blends_the_clean_input_back() {
        let mut drive = StereoDrive::new(SAMPLE_RATE);
        let full_dry = drive.process(
            (0.5, -0.5),
            DriveParams {
                dry: 1.0,
                ..params(1.0, DriveType::Hard)
            },
        );
        assert!((full_dry.0 - 0.5).abs() < 1e-6 && (full_dry.1 + 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_type_change_crossfades_instead_of_stepping() {
        let mut drive = StereoDrive::new(SAMPLE_RATE);
        let mut previous = 0.0;
        let mut largest_step = 0.0f32;
        for n in 0..9_600 {
            let x = 0.6 * (std::f32::consts::TAU * 55.0 * n as f32 / SAMPLE_RATE).sin();
            // Switch where the curves disagree, not at a zero crossing.
            let kind = if n < 4_900 {
                DriveType::Soft
            } else {
                DriveType::Fold
            };
            let out = drive.process((x, x), params(1.0, kind)).0;
            if n > 0 {
                largest_step = largest_step.max((out - previous).abs());
            }
            previous = out;
        }
        // A 55 Hz tone moves well under 0.05 per sample at this level; an
        // unfaded switch between curves jumps by several times that.
        assert!(largest_step < 0.05, "{largest_step}");
    }
}
