//! Stereo feedback delay used by post-synthesis module slots.

/// Fixed duration of the crossfade between a delay tap's old and new positions.
/// Short enough to track tempo-synced retargeting without becoming an
/// audible effect of its own, long enough to hide the splice.
const CROSSFADE_MS: f32 = 20.0;

/// One read position of a delay line, plus (while retargeting) the position
/// it's fading out from. Neither position ever moves once set: a delay-time
/// change is handled by crossfading to a new fixed position rather than
/// gliding the existing one, so it never pitch-bends the signal underneath.
#[derive(Clone, Copy, Default)]
struct Tap {
    current: Option<f32>,
    previous: Option<f32>,
    fade: f32,
}

impl Tap {
    /// Start a crossfade to `target` if settled and it differs from
    /// `current`. A retarget mid-crossfade is ignored until the in-flight
    /// one finishes, so a fast-moving target (e.g. BPM smoothing) produces a
    /// chain of short crossfades instead of overlapping ones.
    fn retarget(&mut self, target: f32) {
        match (self.current, self.previous) {
            (None, _) => self.current = Some(target),
            (Some(current), None) if current != target => {
                self.previous = Some(current);
                self.current = Some(target);
                self.fade = 0.0;
            }
            _ => {}
        }
    }

    /// Positions to read this sample, each with its crossfade weight.
    /// Advances the fade and drops `previous` once it completes.
    fn positions(&mut self, fade_step: f32) -> ((f32, f32), Option<(f32, f32)>) {
        let current = self.current.expect("retarget populates current first");
        let Some(previous) = self.previous else {
            return ((current, 1.0), None);
        };
        let theta = self.fade * std::f32::consts::FRAC_PI_2;
        let (weight_out, weight_in) = (theta.cos(), theta.sin());
        self.fade += fade_step;
        if self.fade >= 1.0 {
            self.previous = None;
        }
        ((current, weight_in), Some((previous, weight_out)))
    }
}

/// One fixed-capacity stereo feedback line. Capacity is allocated once when a
/// Delay module first becomes audible; processing itself never allocates.
pub(crate) struct StereoDelay {
    left: Vec<f32>,
    right: Vec<f32>,
    write: usize,
    left_tap: Tap,
    right_tap: Tap,
}

impl StereoDelay {
    pub(crate) fn new(max_delay_samples: usize) -> Self {
        let len = max_delay_samples.max(1) + 1;
        Self {
            left: vec![0.0; len],
            right: vec![0.0; len],
            write: 0,
            left_tap: Tap::default(),
            right_tap: Tap::default(),
        }
    }

    /// Left read-tap crossfade state: `(current, previous, fade)`. Exists so
    /// the retarget tests can assert the tap's behaviour without a public
    /// snapshot type.
    #[cfg(test)]
    fn left_tap(&self) -> (Option<f32>, Option<f32>, f32) {
        (
            self.left_tap.current,
            self.left_tap.previous,
            self.left_tap.fade,
        )
    }

    pub(crate) fn process(&mut self, input: (f32, f32), params: DelayParams) -> (f32, f32) {
        let vintage = params.vintage.clamp(0.0, 1.0);
        self.left_tap.retarget(params.left_delay_samples as f32);
        self.right_tap.retarget(params.right_delay_samples as f32);
        let crossfade_samples = (params.sample_rate * (CROSSFADE_MS / 1_000.0)).max(1.0);
        let fade_step = 1.0 / crossfade_samples;
        let phase = self.write as f32 / self.left.len() as f32 * std::f32::consts::TAU;
        // Keep the first half playable, then let the worn-tape pitch motion
        // bloom aggressively near the end of the Vintage sweep.
        let modulation = vintage * 4.0 + vintage.powi(3) * 12.0;
        let read = |buffer: &[f32], delay: f32, offset: f32| {
            let delay = (delay + offset).clamp(1.0, (buffer.len() - 1) as f32);
            let floor = delay.floor();
            let fraction = delay - floor;
            let first = (self.write + buffer.len() - floor as usize) % buffer.len();
            let second = (first + buffer.len() - 1) % buffer.len();
            buffer[first] + (buffer[second] - buffer[first]) * fraction
        };
        let blend = |buffer: &[f32], tap: &mut Tap, offset: f32| {
            let ((current, current_weight), previous) = tap.positions(fade_step);
            let sample = read(buffer, current, offset) * current_weight;
            match previous {
                Some((position, weight)) => sample + read(buffer, position, offset) * weight,
                None => sample,
            }
        };
        let delayed_left = blend(
            &self.left,
            &mut self.left_tap,
            modulation * (phase.sin() * 0.5 + 0.5),
        );
        let delayed_right = blend(
            &self.right,
            &mut self.right_tap,
            modulation * ((phase + 1.7).sin() * 0.5 + 0.5),
        );
        let feedback = params.feedback.clamp(0.0, 0.95);
        self.left[self.write] = input.0 + delayed_left * feedback;
        self.right[self.write] = input.1 + delayed_right * feedback;
        self.write = (self.write + 1) % self.left.len();
        let color = |sample: f32| {
            if vintage <= f32::EPSILON {
                sample
            } else {
                let drive = 1.0 + vintage * 0.75;
                (sample * drive).tanh() / drive
            }
        };
        (
            input.0 + color(delayed_left) * params.amount.clamp(0.0, 1.0),
            input.1 + color(delayed_right) * params.amount.clamp(0.0, 1.0),
        )
    }
}

/// Grouped to keep `process` under clippy's argument-count lint.
pub(crate) struct DelayParams {
    pub(crate) left_delay_samples: usize,
    pub(crate) right_delay_samples: usize,
    pub(crate) feedback: f32,
    pub(crate) amount: f32,
    pub(crate) vintage: f32,
    pub(crate) sample_rate: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 44_100.0;

    fn params(left: usize, right: usize, feedback: f32, vintage: f32) -> DelayParams {
        DelayParams {
            left_delay_samples: left,
            right_delay_samples: right,
            feedback,
            amount: 1.0,
            vintage,
            sample_rate: RATE,
        }
    }

    #[test]
    fn process_returns_the_delayed_sample_at_each_channel_time() {
        let mut delay = StereoDelay::new(8);
        delay.process((1.0, 2.0), params(1, 2, 0.0, 0.0));
        let output = delay.process((0.0, 0.0), params(1, 2, 0.0, 0.0));
        assert_eq!(output, (1.0, 0.0));
    }

    #[test]
    fn vintage_colors_only_the_delayed_signal() {
        let mut clean = StereoDelay::new(32);
        let mut vintage = StereoDelay::new(32);
        let clean_dry = clean.process((0.8, 0.8), params(4, 4, 0.0, 0.0));
        let vintage_dry = vintage.process((0.8, 0.8), params(4, 4, 0.0, 1.0));
        assert_eq!(clean_dry, vintage_dry);

        let mut clean_wet = (0.0, 0.0);
        let mut vintage_wet = (0.0, 0.0);
        for _ in 0..16 {
            clean_wet = clean.process((0.0, 0.0), params(4, 4, 0.0, 0.0));
            vintage_wet = vintage.process((0.0, 0.0), params(4, 4, 0.0, 1.0));
            if clean_wet != (0.0, 0.0) || vintage_wet != (0.0, 0.0) {
                break;
            }
        }
        assert_ne!(clean_wet, vintage_wet);
    }

    #[test]
    fn delay_time_change_starts_a_crossfade_instead_of_sliding_the_read_head() {
        let mut delay = StereoDelay::new(2_000);
        delay.process((0.0, 0.0), params(100, 100, 0.0, 0.0));
        delay.process((0.0, 0.0), params(1_000, 1_000, 0.0, 0.0));
        let (current, previous, fade) = delay.left_tap();
        // The new target is latched immediately; the old position fades out
        // in the background rather than the read head sliding toward it.
        assert_eq!(current, Some(1_000.0));
        assert_eq!(previous, Some(100.0));
        assert!(fade > 0.0 && fade < 1.0);
    }

    #[test]
    fn large_delay_time_jumps_crossfade_without_a_click() {
        let mut delay = StereoDelay::new(100_000);
        delay.process((0.0, 0.0), params(441, 441, 0.0, 0.0));
        delay.process((0.0, 0.0), params(88_200, 88_200, 0.0, 0.0));
        let (current, previous, _) = delay.left_tap();
        // A huge jump snaps the target immediately; only the amplitude
        // blend is gradual, so there's no cap on how far it can move.
        assert_eq!(current, Some(88_200.0));
        assert_eq!(previous, Some(441.0));
    }

    #[test]
    fn retarget_mid_crossfade_is_ignored_until_it_settles() {
        let mut delay = StereoDelay::new(2_000);
        delay.process((0.0, 0.0), params(100, 100, 0.0, 0.0));
        delay.process((0.0, 0.0), params(500, 500, 0.0, 0.0));
        delay.process((0.0, 0.0), params(900, 900, 0.0, 0.0));
        let (current, previous, _) = delay.left_tap();
        assert_eq!(current, Some(500.0));
        assert_eq!(previous, Some(100.0));
    }

    #[test]
    fn crossfade_completes_after_the_fixed_window() {
        let mut delay = StereoDelay::new(2_000);
        delay.process((0.0, 0.0), params(100, 100, 0.0, 0.0));
        delay.process((0.0, 0.0), params(1_000, 1_000, 0.0, 0.0));
        let window = (RATE * (CROSSFADE_MS / 1_000.0)).ceil() as usize;
        for _ in 0..window {
            delay.process((0.0, 0.0), params(1_000, 1_000, 0.0, 0.0));
        }
        let (current, previous, _) = delay.left_tap();
        assert_eq!(current, Some(1_000.0));
        assert_eq!(previous, None);
    }
}
