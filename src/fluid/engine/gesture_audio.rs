//! The fixed Master-bus processors for temporary live gesture audio.

use super::*;

const SUBMERGE_MIN_CUTOFF_HZ: f32 = 600.0;
const SUBMERGE_OPEN_CUTOFF_HZ: f32 = 8_000.0;
const LIFT_MIN_CUTOFF_HZ: f32 = 40.0;
const LIFT_MAX_CUTOFF_HZ: f32 = 320.0;
const BLOOM_SEND_GAIN: f32 = 0.32;
const BLOOM_DRY_DUCK: f32 = 0.10;
const BLOOM_ROOM_SIZE: f32 = 0.80;
/// Damping softens the reverberant high end while the dry transient stays clear.
const BLOOM_DAMPING: f32 = 0.48;
/// The send is band-shaped so sustained lows never pile up in the feedback network and
/// sharp drum/pluck clicks stay on the dry grid instead of flamming in the wash.
const BLOOM_SEND_HIGH_PASS_HZ: f32 = 220.0;
const BLOOM_SEND_LOW_PASS_HZ: f32 = 3_200.0;
const ECHO_SEND_GAIN: f32 = 0.35;
const ECHO_DRY_DUCK: f32 = 0.2;
const ECHO_MAX_FEEDBACK: f32 = 0.5;
const PROCESSOR_CLEAR_SAMPLES_PER_FRAME: usize = 512;
const ECHO_FEEDBACK_SECONDS: f32 = 0.03;

struct GestureFx {
    repeat: BeatRepeat,
    submerge: SlotFx,
    lift: SlotFx,
    bloom: SlotFx,
    bloom_send_filter: StereoFilter,
    bloom_send_lowpass: (f32, f32),
    bloom_send_lowpass_alpha: f32,
    echo: SlotFx,
    submerge_active: bool,
    lift_active: bool,
    bloom_has_history: bool,
    echo_has_history: bool,
    bloom_wet: ReturnWeight,
    echo_wet: ReturnWeight,
    bloom_clear_cursor: usize,
    echo_clear_cursor: usize,
    echo_feedback: f32,
    echo_feedback_target: f32,
    echo_feedback_alpha: f32,
    previous_echo_amount: f32,
}

impl GestureFx {
    fn new(sample_rate: f32, max_delay_samples: usize) -> Self {
        Self {
            repeat: BeatRepeat::new(sample_rate),
            submerge: SlotFx::Filter(StereoFilter::default()),
            lift: SlotFx::Filter(StereoFilter::default()),
            bloom: SlotFx::Reverb(FdnReverb::new(sample_rate)),
            bloom_send_filter: StereoFilter::default(),
            bloom_send_lowpass: (0.0, 0.0),
            bloom_send_lowpass_alpha: 1.0
                - (-std::f32::consts::TAU * BLOOM_SEND_LOW_PASS_HZ / sample_rate).exp(),
            echo: SlotFx::Delay(StereoDelay::new(max_delay_samples)),
            submerge_active: false,
            lift_active: false,
            bloom_has_history: false,
            echo_has_history: false,
            bloom_wet: ReturnWeight::default(),
            echo_wet: ReturnWeight::default(),
            bloom_clear_cursor: 0,
            echo_clear_cursor: 0,
            echo_feedback: 0.25,
            echo_feedback_target: 0.25,
            echo_feedback_alpha: 1.0 - (-1.0 / (ECHO_FEEDBACK_SECONDS * sample_rate)).exp(),
            previous_echo_amount: 0.0,
        }
    }

    fn process(
        &mut self,
        sample: (f32, f32),
        amounts: [f32; GESTURE_COUNT],
        timing: TimingContext,
        max_delay_samples: usize,
        sample_rate: f32,
    ) -> (f32, f32) {
        // The repeat records every live sample, held or not, so a press always
        // finds the beat it began in.
        let repeated =
            self.repeat
                .process(sample, amounts[GestureKind::BeatRepeat as usize], timing);

        if amounts == [0.0; GESTURE_COUNT]
            && !self.submerge_active
            && !self.lift_active
            && !self.bloom_has_history
            && !self.echo_has_history
        {
            return sample;
        }

        let mut source = repeated;

        let submerge_amount = amounts[GestureKind::Submerge as usize];
        if submerge_amount > f32::EPSILON {
            self.submerge_active = true;
            let cutoff_hz = SUBMERGE_OPEN_CUTOFF_HZ
                * (SUBMERGE_MIN_CUTOFF_HZ / SUBMERGE_OPEN_CUTOFF_HZ).powf(submerge_amount);
            let slot = ModuleSlot {
                amount: 1.0,
                time: cutoff_hz,
                right_time: 0.0,
                feedback: 0.0,
                ..ModuleSlot::default()
            };
            let input = source;
            source = ModuleFxBank::process_slot_fx(
                &mut self.submerge,
                &slot,
                source,
                timing,
                max_delay_samples,
                sample_rate,
            );
            source = mix_stereo(input, source, submerge_amount);
        } else if self.submerge_active {
            self.submerge = SlotFx::Filter(StereoFilter::default());
            self.submerge_active = false;
        }

        let lift_amount = amounts[GestureKind::Lift as usize];
        if lift_amount > f32::EPSILON {
            self.lift_active = true;
            let cutoff_hz =
                LIFT_MIN_CUTOFF_HZ + (LIFT_MAX_CUTOFF_HZ - LIFT_MIN_CUTOFF_HZ) * lift_amount;
            let slot = ModuleSlot {
                amount: 1.0,
                time: cutoff_hz,
                right_time: 0.0,
                feedback: 1.0,
                ..ModuleSlot::default()
            };
            let filtered = ModuleFxBank::process_slot_fx(
                &mut self.lift,
                &slot,
                source,
                timing,
                max_delay_samples,
                sample_rate,
            );
            source = mix_stereo(source, filtered, lift_amount);
        } else if self.lift_active {
            self.lift = SlotFx::Filter(StereoFilter::default());
            self.lift_active = false;
        }

        let bloom_amount = amounts[GestureKind::Bloom as usize];
        let bloom_input = scale(source, bloom_amount * BLOOM_SEND_GAIN);
        let bloom_return = self.process_bloom(
            bloom_input,
            bloom_amount,
            timing,
            max_delay_samples,
            sample_rate,
        );

        let echo_amount = amounts[GestureKind::Echo as usize];
        let echo_input = scale(source, echo_amount * ECHO_SEND_GAIN);
        let echo_return = self.process_echo(
            echo_input,
            echo_amount,
            timing,
            max_delay_samples,
            sample_rate,
        );

        // The dry path ducks as each parallel return rises, so a full throw
        // trades dry level for wet instead of stacking on top of it.
        let dry = (1.0 - BLOOM_DRY_DUCK * bloom_amount) * (1.0 - ECHO_DRY_DUCK * echo_amount);
        (
            source.0 * dry + bloom_return.0 + echo_return.0,
            source.1 * dry + bloom_return.1 + echo_return.1,
        )
    }

    fn process_bloom(
        &mut self,
        input: (f32, f32),
        amount: f32,
        timing: TimingContext,
        max_delay_samples: usize,
        sample_rate: f32,
    ) -> (f32, f32) {
        let fresh = amount > f32::EPSILON && !self.bloom_has_history;
        if amount > f32::EPSILON {
            self.bloom_has_history = true;
            self.bloom_clear_cursor = 0;
        }
        if !self.bloom_has_history {
            return (0.0, 0.0);
        }
        let wet = self.bloom_wet.advance(amount, fresh, sample_rate);
        if amount <= f32::EPSILON && wet == 0.0 {
            if let SlotFx::Reverb(reverb) = &mut self.bloom
                && reverb.clear_chunk(
                    &mut self.bloom_clear_cursor,
                    PROCESSOR_CLEAR_SAMPLES_PER_FRAME,
                )
            {
                self.bloom_has_history = false;
                self.bloom_clear_cursor = 0;
                self.bloom_send_filter = StereoFilter::default();
                self.bloom_send_lowpass = (0.0, 0.0);
            }
            return (0.0, 0.0);
        }

        let input = self.bloom_send_filter.process(
            input,
            FilterParams {
                sample_rate,
                cutoff_hz: BLOOM_SEND_HIGH_PASS_HZ,
                resonance: 0.0,
                filter_type: FilterType::High,
                amount: 1.0,
            },
        );
        self.bloom_send_lowpass.0 +=
            self.bloom_send_lowpass_alpha * (input.0 - self.bloom_send_lowpass.0);
        self.bloom_send_lowpass.1 +=
            self.bloom_send_lowpass_alpha * (input.1 - self.bloom_send_lowpass.1);
        let input = self.bloom_send_lowpass;
        let slot = ModuleSlot {
            amount: 1.0,
            time: BLOOM_ROOM_SIZE,
            feedback: BLOOM_DAMPING,
            ..ModuleSlot::default()
        };
        let processed = ModuleFxBank::process_slot_fx(
            &mut self.bloom,
            &slot,
            input,
            timing,
            max_delay_samples,
            sample_rate,
        );
        scale(subtract(processed, input), wet)
    }

    fn clear_echo(&mut self) {
        if let SlotFx::Delay(delay) = &mut self.echo
            && delay.clear_chunk(
                &mut self.echo_clear_cursor,
                PROCESSOR_CLEAR_SAMPLES_PER_FRAME,
            )
        {
            self.echo_has_history = false;
            self.echo_clear_cursor = 0;
            self.echo_feedback = 0.25;
            self.echo_feedback_target = 0.25;
            self.previous_echo_amount = 0.0;
            self.echo_wet = ReturnWeight::default();
        }
    }

    fn process_echo(
        &mut self,
        input: (f32, f32),
        amount: f32,
        timing: TimingContext,
        max_delay_samples: usize,
        sample_rate: f32,
    ) -> (f32, f32) {
        let fresh = amount > f32::EPSILON && !self.echo_has_history;
        if amount > f32::EPSILON {
            self.echo_has_history = true;
            self.echo_clear_cursor = 0;
            if amount > self.previous_echo_amount {
                self.echo_feedback_target = 0.25 + amount * (ECHO_MAX_FEEDBACK - 0.25);
            }
        }
        if !self.echo_has_history {
            return (0.0, 0.0);
        }
        self.previous_echo_amount = amount;
        let wet = self.echo_wet.advance(amount, fresh, sample_rate);
        if amount <= f32::EPSILON && wet == 0.0 {
            self.clear_echo();
            return (0.0, 0.0);
        }
        self.echo_feedback +=
            (self.echo_feedback_target - self.echo_feedback) * self.echo_feedback_alpha;

        let slot = ModuleSlot {
            amount: 1.0,
            time: 250.0,
            right_time: 375.0,
            clock: DelayClock::Free.value(),
            right_clock: DelayClock::Free.value(),
            feedback: self.echo_feedback,
            vintage: 0.12,
            ..ModuleSlot::default()
        };
        let processed = ModuleFxBank::process_slot_fx(
            &mut self.echo,
            &slot,
            input,
            timing,
            max_delay_samples,
            sample_rate,
        );
        scale(subtract(processed, input), wet)
    }
}

/// Beat Repeat slice length in quarter notes. The only length: one beat.
const BEAT_REPEAT_BEATS: i64 = 1;
/// Pre-roll blended over the loop seam. The wrap then lands on the audio that
/// originally followed the slice, so a loop of arbitrary material has no click.
const BEAT_REPEAT_SEAM_SECONDS: f32 = 0.002;
/// A slice is accepted when its measured length is within a quarter of the
/// nominal beat; a transport restart or jump between grid lines is not a beat.
const BEAT_REPEAT_LENGTH_TOLERANCE_DIVISOR: usize = 4;

enum RepeatState {
    Idle,
    /// Pressed: waiting for the grid line that ends the beat the hold began in.
    Armed {
        start: u64,
        beat: i64,
    },
    /// `pos` reads the slice body; `gain` fades the loop in over the release time.
    Looping {
        pos: usize,
        len: usize,
        gain: f32,
    },
}

/// Tempo-synced repeat of the live Master mix. History records continuously;
/// a hold arms at the beat it began in, copies that beat when its grid line
/// ends it, and loops the copy until release. Storage is allocated up front.
struct BeatRepeat {
    history: Vec<(f32, f32)>,
    /// `seam` pre-roll samples, then the loop body.
    slice: Vec<(f32, f32)>,
    /// Samples recorded so far; the next write lands at `frame % history.len()`.
    frame: u64,
    beat_index: Option<i64>,
    beat_start: u64,
    state: RepeatState,
    seam: usize,
    max_len: usize,
}

impl BeatRepeat {
    fn new(sample_rate: f32) -> Self {
        let seam = ((sample_rate * BEAT_REPEAT_SEAM_SECONDS) as usize).max(1);
        // Longest accepted slice: the beat at the slowest tempo, plus tolerance.
        let beat_samples_max = f64::from(sample_rate) * 60.0 / f64::from(MASTER_BPM_MIN);
        let max_len = (beat_samples_max * BEAT_REPEAT_BEATS as f64 * 1.25).ceil() as usize + 1;
        Self {
            history: vec![(0.0, 0.0); max_len + seam + 2],
            slice: vec![(0.0, 0.0); max_len + seam],
            frame: 0,
            beat_index: None,
            beat_start: 0,
            state: RepeatState::Idle,
            seam,
            max_len,
        }
    }

    /// Returns `sample` with the loop mixed in by `amount`.
    fn process(&mut self, sample: (f32, f32), amount: f32, timing: TimingContext) -> (f32, f32) {
        let pressed = amount > f32::EPSILON;
        if !pressed {
            self.state = RepeatState::Idle;
        }

        let beat_index = timing.beat.floor() as i64;
        if self.beat_index != Some(beat_index) {
            self.beat_index = Some(beat_index);
            self.on_grid_line(beat_index, timing);
            self.beat_start = self.frame;
        }
        if pressed && matches!(self.state, RepeatState::Idle) {
            self.state = RepeatState::Armed {
                start: self.beat_start,
                beat: beat_index,
            };
        }

        let mut output = sample;
        if let RepeatState::Looping { pos, len, gain } = &mut self.state {
            let mut looped = self.slice[self.seam + *pos];
            let tail = (*pos + self.seam).saturating_sub(*len);
            if tail > 0 {
                let t = tail as f32 / self.seam as f32;
                looped = mix_stereo(looped, self.slice[tail - 1], t);
            }
            *gain = (*gain + 1.0 / (GESTURE_RELEASE_SECONDS as f32 * timing.sample_rate as f32))
                .min(1.0);
            output = mix_stereo(sample, looped, amount * *gain);
            *pos = (*pos + 1) % *len;
        }

        let capacity = self.history.len() as u64;
        self.history[(self.frame % capacity) as usize] = sample;
        self.frame += 1;
        output
    }

    /// The clock entered a new beat at the current frame. An armed hold whose
    /// beat just ended copies it and starts looping; a hold that cannot use
    /// this line (no pre-roll yet, or not one beat long) re-arms on it.
    fn on_grid_line(&mut self, beat_index: i64, timing: TimingContext) {
        let RepeatState::Armed { start, beat } = self.state else {
            return;
        };
        let elapsed_beats = beat_index - beat;
        if elapsed_beats > 0 && elapsed_beats < BEAT_REPEAT_BEATS {
            return;
        }
        let len = (self.frame - start) as usize;
        let nominal =
            (timing.sample_rate * 60.0 / timing.bpm * BEAT_REPEAT_BEATS as f64).round() as usize;
        // The pre-roll must be real audio too, or the seam would dip to silence.
        let plausible = elapsed_beats == BEAT_REPEAT_BEATS
            && start >= self.seam as u64
            && len >= self.seam * 2
            && len <= self.max_len
            && len.abs_diff(nominal) * BEAT_REPEAT_LENGTH_TOLERANCE_DIVISOR <= nominal;
        if plausible {
            self.copy_history(start as i64 - self.seam as i64, self.seam + len);
            self.state = RepeatState::Looping {
                pos: 0,
                len,
                gain: 0.0,
            };
        } else {
            self.state = RepeatState::Armed {
                start: self.frame,
                beat: beat_index,
            };
        }
    }

    /// Copies `count` recorded samples starting at frame `from` into `slice`;
    /// frames before the first sample read as silence.
    fn copy_history(&mut self, from: i64, count: usize) {
        let capacity = self.history.len();
        let mut done = 0;
        if from < 0 {
            done = (from.unsigned_abs() as usize).min(count);
            self.slice[..done].fill((0.0, 0.0));
        }
        let mut frame = (from + done as i64) as u64;
        while done < count {
            let index = (frame % capacity as u64) as usize;
            let run = (count - done).min(capacity - index);
            self.slice[done..done + run].copy_from_slice(&self.history[index..index + run]);
            done += run;
            frame += run as u64;
        }
    }
}

/// Linear weight on a wet return. It slews to 0 over
/// `GESTURE_RELEASE_SECONDS` while the envelope falls, so on key-up the wash
/// itself stops instead of ringing on as a tail, and back toward 1 otherwise.
#[derive(Default)]
struct ReturnWeight {
    wet: f32,
    previous_amount: f32,
}

impl ReturnWeight {
    fn advance(&mut self, amount: f32, fresh: bool, sample_rate: f32) -> f32 {
        let releasing = amount <= f32::EPSILON || amount < self.previous_amount;
        self.previous_amount = amount;
        if fresh {
            // An empty processor has nothing to fade in, so a fresh press
            // opens the return at once and the attack is the send alone.
            self.wet = 1.0;
        }
        let step = 1.0 / (GESTURE_RELEASE_SECONDS as f32 * sample_rate);
        self.wet = if releasing {
            (self.wet - step).max(0.0)
        } else {
            (self.wet + step).min(1.0)
        };
        self.wet
    }
}

fn scale(sample: (f32, f32), gain: f32) -> (f32, f32) {
    (sample.0 * gain, sample.1 * gain)
}

fn subtract(sample: (f32, f32), dry: (f32, f32)) -> (f32, f32) {
    (sample.0 - dry.0, sample.1 - dry.1)
}

pub(super) struct GestureAudioBank {
    fx: GestureFx,
    max_delay_samples: usize,
    sample_rate: f32,
}

impl GestureAudioBank {
    pub(super) fn new(sample_rate: f32) -> Self {
        let max_delay_samples = (sample_rate * (DELAY_FREE_MAX_MS / 1_000.0)).ceil() as usize;
        Self {
            fx: GestureFx::new(sample_rate, max_delay_samples),
            max_delay_samples,
            sample_rate,
        }
    }

    pub(super) fn process(
        &mut self,
        sample: (f32, f32),
        amounts: [f32; GESTURE_COUNT],
        timing: TimingContext,
    ) -> (f32, f32) {
        self.fx.process(
            sample,
            amounts,
            timing,
            self.max_delay_samples,
            self.sample_rate,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BPM: f64 = 120.0;
    const QUARTER: usize = (SAMPLE_RATE as f64 * 60.0 / BPM) as usize;

    /// Timing for output frame `frame` of a constant-tempo transport that
    /// starts on beat zero.
    fn clock(frame: usize, bpm: f64) -> TimingContext {
        TimingContext::new(
            SAMPLE_RATE as f64,
            bpm,
            frame as f64 * bpm / (60.0 * SAMPLE_RATE as f64),
        )
    }

    /// A signal whose value names its own frame, so a loop's origin is readable.
    fn ramp(frame: usize) -> (f32, f32) {
        let value = frame as f32 * 1e-6;
        (value, -value)
    }

    fn repeat_amounts(amount: f32) -> [f32; GESTURE_COUNT] {
        let mut amounts = [0.0; GESTURE_COUNT];
        amounts[GestureKind::BeatRepeat as usize] = amount;
        amounts
    }

    /// Plays `frames` of the ramp starting at `from`, holding Beat Repeat from `press`.
    fn play(
        bank: &mut GestureAudioBank,
        from: usize,
        frames: usize,
        press: usize,
    ) -> Vec<(f32, f32)> {
        (from..from + frames)
            .map(|frame| {
                let amount = if frame >= press { 1.0 } else { 0.0 };
                bank.process(ramp(frame), repeat_amounts(amount), clock(frame, BPM))
            })
            .collect()
    }

    #[test]
    fn beat_repeat_loops_exactly_one_quarter_note_starting_on_the_grid() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        // Press 0.3 of the way into beat 1.
        let press = QUARTER + QUARTER * 3 / 10;
        let out = play(&mut bank, 0, QUARTER * 6, press);

        let RepeatState::Looping { len, .. } = bank.fx.repeat.state else {
            panic!("hold did not reach the loop");
        };
        assert!(
            len.abs_diff(QUARTER) <= 1,
            "loop was {len} samples, not {QUARTER}"
        );
        let seam = bank.fx.repeat.seam;
        let origin = bank.fx.repeat.slice[seam].0;
        assert!(
            (origin - (QUARTER as f32 * 1e-6)).abs() < 1.5e-6,
            "slice began at {origin}, not on beat 1"
        );
        // Dry until the next grid line, then the loop repeats the captured beat.
        assert_eq!(out[press], ramp(press));
        let engage = QUARTER * 2 + 480;
        let period = &out[engage..engage + len];
        for later in 1..=2 {
            let next = &out[engage + later * len..engage + (later + 1) * len];
            for (a, b) in period.iter().zip(next) {
                assert!((a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5);
            }
        }
        assert!(period.iter().all(|x| x.0 >= QUARTER as f32 * 1e-6 - 2e-6));
    }

    #[test]
    fn beat_repeat_length_follows_the_tempo_at_capture() {
        for bpm in [60.0, 96.0, 174.0] {
            let mut bank = GestureAudioBank::new(SAMPLE_RATE);
            let frames = (SAMPLE_RATE as f64 * 60.0 / bpm * 3.5) as usize;
            for frame in 0..frames {
                let amount = if frame > 100 { 1.0 } else { 0.0 };
                bank.process(ramp(frame), repeat_amounts(amount), clock(frame, bpm));
            }
            let RepeatState::Looping { len, .. } = bank.fx.repeat.state else {
                panic!("{bpm} BPM: hold did not reach the loop");
            };
            let beat = (SAMPLE_RATE as f64 * 60.0 / bpm).round() as usize;
            assert!(
                len.abs_diff(beat) <= 1,
                "{bpm} BPM: loop {len}, beat {beat}"
            );
        }
    }

    #[test]
    fn beat_repeat_loop_length_is_latched_when_tempo_changes_mid_hold() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        play(&mut bank, 0, QUARTER * 3, QUARTER / 2);
        let RepeatState::Looping { len, .. } = bank.fx.repeat.state else {
            panic!("hold did not reach the loop");
        };
        for frame in QUARTER * 3..QUARTER * 6 {
            bank.process(ramp(frame), repeat_amounts(1.0), clock(frame, 90.0));
        }
        let RepeatState::Looping { len: after, .. } = bank.fx.repeat.state else {
            panic!("tempo change dropped the loop");
        };
        assert_eq!(len, after);
    }

    #[test]
    fn beat_repeat_seam_and_engage_are_smooth_for_non_periodic_material() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let tone = |frame: usize| {
            let x = (std::f32::consts::TAU * 100.3 * frame as f32 / SAMPLE_RATE).sin() * 0.2;
            (x, x)
        };
        let mut previous = (0.0, 0.0);
        for frame in 0..QUARTER * 8 {
            let amount = if frame > QUARTER / 3 { 1.0 } else { 0.0 };
            let next = bank.process(tone(frame), repeat_amounts(amount), clock(frame, BPM));
            assert!(
                (next.0 - previous.0).abs() < 0.012,
                "frame {frame}: {previous:?} -> {next:?}"
            );
            previous = next;
        }
    }

    #[test]
    fn beat_repeat_release_returns_to_live_within_the_fade_without_allocation() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let (history, slice) = (
            bank.fx.repeat.history.capacity(),
            bank.fx.repeat.slice.capacity(),
        );
        let tone = |frame: usize| {
            let x = (std::f32::consts::TAU * 100.3 * frame as f32 / SAMPLE_RATE).sin() * 0.2;
            (x, x)
        };
        let held = QUARTER * 4;
        for frame in 0..held {
            bank.process(tone(frame), repeat_amounts(1.0), clock(frame, BPM));
        }
        let fade = (GESTURE_RELEASE_SECONDS as f32 * SAMPLE_RATE) as usize;
        let mut previous = bank.process(tone(held), repeat_amounts(1.0), clock(held, BPM));
        for step in 0..fade {
            let frame = held + 1 + step;
            let amount = 1.0 - (step + 1) as f32 / fade as f32;
            let next = bank.process(tone(frame), repeat_amounts(amount), clock(frame, BPM));
            assert!(
                (next.0 - previous.0).abs() < 0.012,
                "release: {previous:?} -> {next:?}"
            );
            previous = next;
        }
        let frame = held + 1 + fade;
        assert_eq!(
            bank.process(tone(frame), [0.0; GESTURE_COUNT], clock(frame, BPM)),
            tone(frame)
        );
        assert_eq!(bank.fx.repeat.history.capacity(), history);
        assert_eq!(bank.fx.repeat.slice.capacity(), slice);
    }

    #[test]
    fn beat_repeat_loaded_hold_waits_for_live_audio_then_fades_in() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut previous = (0.3, 0.3);
        for frame in 0..QUARTER * 3 {
            let tone = (0.3, 0.3);
            let next = bank.process(tone, repeat_amounts(1.0), clock(frame, BPM));
            assert!((next.0 - previous.0).abs() < 0.005, "frame {frame}");
            previous = next;
            if frame < QUARTER * 2 {
                assert_eq!(next, tone, "no loop before a full beat of live pre-roll");
            }
        }
        assert!(matches!(bank.fx.repeat.state, RepeatState::Looping { .. }));
    }

    #[test]
    fn beat_repeat_press_while_transport_is_stopped_stays_dry() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        for frame in 0..QUARTER * 2 {
            let timing = TimingContext {
                transport: Transport::Stopped,
                ..clock(0, BPM)
            };
            assert_eq!(
                bank.process(ramp(frame), repeat_amounts(1.0), timing),
                ramp(frame)
            );
        }
    }

    const SAMPLE_RATE: f32 = 48_000.0;

    fn timing() -> TimingContext {
        TimingContext::new(SAMPLE_RATE as f64, 120.0, 0.0)
    }

    fn active(kind: GestureKind) -> [f32; GESTURE_COUNT] {
        let mut amounts = [0.0; GESTURE_COUNT];
        amounts[kind as usize] = 1.0;
        amounts
    }

    #[test]
    fn zero_submerge_is_an_exact_dry_bypass() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let input = (0.4, -0.2);

        assert_eq!(bank.process(input, [0.0; GESTURE_COUNT], timing()), input);
    }

    #[test]
    fn full_submerge_audibly_removes_high_frequency_alternation() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut energy = 0.0;
        for index in 0..512 {
            let input = if index % 2 == 0 {
                (1.0, 1.0)
            } else {
                (-1.0, -1.0)
            };
            let output = bank.process(input, active(GestureKind::Submerge), timing());
            energy += output.0.abs() + output.1.abs();
        }

        assert!(
            energy < 32.0,
            "submerge left high-frequency energy at {energy}"
        );
    }

    #[test]
    fn full_lift_audibly_removes_low_frequency_content() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let (mut dry_energy, mut low_energy) = (0.0, 0.0);
        for index in 0..SAMPLE_RATE as usize {
            let phase = index as f32 * std::f32::consts::TAU * 80.0 / SAMPLE_RATE;
            let input = (phase.sin(), phase.sin());
            let output = bank.process(input, active(GestureKind::Lift), timing());
            dry_energy += input.0.abs() + input.1.abs();
            low_energy += output.0.abs() + output.1.abs();
        }

        assert!(
            low_energy < dry_energy / 8.0,
            "lift left low-frequency energy at {low_energy} of {dry_energy}"
        );
    }

    #[test]
    fn submerge_onset_is_continuous_from_exact_dry() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let input = (0.8, -0.6);
        let mut amounts = [0.0; GESTURE_COUNT];
        amounts[GestureKind::Submerge as usize] = 1e-6;

        let output = bank.process(input, amounts, timing());

        assert!(
            (output.0 - input.0).abs() < 2e-6 && (output.1 - input.1).abs() < 2e-6,
            "Submerge onset jumped from {input:?} to {output:?}"
        );
    }

    #[test]
    fn submerge_release_resets_filter_history_before_repress() {
        let mut used = GestureAudioBank::new(SAMPLE_RATE);
        for frame in 0..512 {
            let sample = if frame % 2 == 0 {
                (1.0, 1.0)
            } else {
                (-1.0, -1.0)
            };
            used.process(sample, active(GestureKind::Submerge), timing());
        }
        let input = (0.3, -0.2);
        assert_eq!(used.process(input, [0.0; GESTURE_COUNT], timing()), input);
        let mut shallow = [0.0; GESTURE_COUNT];
        shallow[GestureKind::Submerge as usize] = 0.001;
        let repressed = used.process(input, shallow, timing());
        let mut fresh = GestureAudioBank::new(SAMPLE_RATE);

        let clean_start = fresh.process(input, shallow, timing());

        assert_eq!(repressed, clean_start);
    }

    #[test]
    fn bloom_adds_a_parallel_reverb_return_without_removing_dry() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut changed = false;
        for frame in 0..8_000 {
            let input = if frame == 0 { (1.0, 1.0) } else { (0.0, 0.0) };
            let output = bank.process(input, active(GestureKind::Bloom), timing());
            changed |= output != input;
        }

        assert!(changed, "Bloom produced no reverb return");
    }

    #[test]
    fn echo_adds_a_delayed_parallel_return() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut changed = false;
        for frame in 0..24_000 {
            let input = if frame == 0 { (1.0, -1.0) } else { (0.0, 0.0) };
            let output = bank.process(input, active(GestureKind::Echo), timing());
            changed |= output != input;
        }

        assert!(changed, "Echo produced no delayed return");
    }

    #[test]
    fn every_wet_return_stops_within_the_release_time() {
        for kind in [GestureKind::Bloom, GestureKind::Echo] {
            let mut bank = GestureAudioBank::new(SAMPLE_RATE);
            for _ in 0..20_000 {
                bank.process((0.5, -0.5), active(kind), timing());
            }
            let release_samples = (GESTURE_RELEASE_SECONDS as f32 * SAMPLE_RATE) as usize;
            let mut amounts = [0.0; GESTURE_COUNT];
            for frame in 0..release_samples {
                // The envelope falls linearly from full over the same window.
                amounts[kind as usize] = 1.0 - frame as f32 / release_samples as f32;
                bank.process((0.0, 0.0), amounts, timing());
            }

            let after = bank.process((0.0, 0.0), [0.0; GESTURE_COUNT], timing());

            assert_eq!(
                after,
                (0.0, 0.0),
                "{} rang on past its release",
                kind.name()
            );
        }
    }

    /// The stage trades dry level for its returns, so even every gesture
    /// thrown at once on a sustained tone stays near the input's own peak.
    #[test]
    fn every_gesture_at_full_throw_stays_near_input_level() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        let mut peak = 0.0_f32;
        for frame in 0..48_000 {
            let input = (((frame as f32 * 0.07).sin() * 0.25), 0.0);
            let output = bank.process(input, [1.0; GESTURE_COUNT], timing());
            peak = peak.max(output.0.abs()).max(output.1.abs());
        }

        assert!(peak < 0.25 * 1.26, "gesture stage peak was {peak}");
    }

    #[test]
    fn drained_tails_clear_incrementally_then_restore_exact_dry_bypass() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        bank.process((0.5, -0.5), [1.0, 0.0, 1.0, 0.0, 0.0], timing());
        bank.fx.bloom_wet = ReturnWeight::default();
        bank.fx.echo_wet = ReturnWeight::default();
        for _ in 0..1_000 {
            bank.process((0.0, 0.0), [0.0; GESTURE_COUNT], timing());
            let layer = &bank.fx;
            if !layer.bloom_has_history && !layer.echo_has_history {
                break;
            }
        }
        let layer = &bank.fx;
        assert!(!layer.bloom_has_history && !layer.echo_has_history);

        let input = (0.31, -0.27);
        assert_eq!(bank.process(input, [0.0; GESTURE_COUNT], timing()), input);
    }

    #[test]
    fn repress_during_incremental_clear_accepts_the_new_send() {
        let mut bank = GestureAudioBank::new(SAMPLE_RATE);
        bank.process((1.0, -1.0), active(GestureKind::Echo), timing());
        bank.fx.echo_wet = ReturnWeight::default();
        for _ in 0..8 {
            bank.process((0.0, 0.0), [0.0; GESTURE_COUNT], timing());
        }
        assert!(bank.fx.echo_clear_cursor > 0);
        let mut shallow = [0.0; GESTURE_COUNT];
        shallow[GestureKind::Echo as usize] = 0.001;
        bank.process((1.0, -1.0), shallow, timing());

        // Still held: the return only rings while the gesture does.
        let heard_new_send =
            (0..20_000).any(|_| bank.process((0.0, 0.0), shallow, timing()) != (0.0, 0.0));

        assert!(
            heard_new_send,
            "repress lost the new Echo send while clearing"
        );
        assert_eq!(bank.fx.echo_clear_cursor, 0);
    }

    #[test]
    fn full_user_slot_bank_does_not_block_the_temporary_stage() {
        let mut user_bank = ModuleFxBank::new(SAMPLE_RATE);
        let user_slots = [preset_slot("drive", 0.2); MODULE_SLOTS];
        let after_user_slots = user_bank.process(Tab::Perc, &user_slots, (0.4, -0.2), timing());
        let mut gesture_bank = GestureAudioBank::new(SAMPLE_RATE);

        let output =
            gesture_bank.process(after_user_slots, active(GestureKind::Submerge), timing());

        assert_ne!(output, after_user_slots);
    }
}
