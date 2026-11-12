# Engine helpers

## Purpose

Audio processing helpers owned by `../engine.rs`.

## Ownership

- `gesture_audio.rs` owns the Master-bus gesture stage, parallel effect returns, and their bounded runtime storage.
- `downbeat_tests.rs` drives Shift+D and `/downbeat` through the production input path into rendered audio. It covers the wait for the next beat line, whole-beat rotation with no phase shift, the step-1 restart, the chord count (including a chord due on the line), a queued state, planned mute, vamp, Beat Repeat, auto, a stopped clock, and clicks.
- `vamp_loop_tests.rs` drives the vamp over a harmony loop through the production keys (Space pushes, `G` `1`, `a`, `"`, `'`) into the engine: the held loop chord, the rejoin on release, a song-code round trip and a state landing.
- `perf_probe.rs` is the ignored audio-thread cost probe: ordinary versus bar-crossing buffer time, steady CPU share, and heap traffic under UI edits.
- `downbeat_probe.rs` is the ignored downbeat-reset cost probe in the show's launch shape (one-song auto, launched stopped), full and reduced terminals, with a queued state, `G` loop, vamp, held Beat Repeat, or Motion loop: per-buffer audio cost, UI tick and frame cost, session publications and re-anchors before a press, after one, and at autorepeat rate.
- `../engine.rs` owns voice/Master routing (gestures run after Master's slot chain, before the master bus), clock publication, and session reads.
- `../gesture.rs` owns the gesture vocabulary and scalar envelopes; shared effect algorithms remain in `../../fx/`.

## Local Contracts

- Reuse the shared module processor dispatch for Filter, Reverb, and Delay. Keep gesture state separate from user slots.
- Allocate gesture processor storage before sample processing. Presses and effect tails must not grow storage.
- Gestures never clip the master: returns trade dry level for wet (Bloom/Echo duck the dry path), filters carry no resonance, and Bloom's send is band-shaped (high-passed at 220 Hz so sustained lows cannot pile up in its reverb, and low-passed at 3.2 kHz with a 10% dry duck so sharp percussive transients stay locked to the dry grid instead of slapping back through the reverb). At real playback level a full throw keeps the stage's input peak under 0.6 before the output makeup. The Master output limiter is a ceiling, not gain staging; headroom is the gesture stage's job.
- Every gesture lets go within `GESTURE_RELEASE_SECONDS` (10 ms, `fluid/gesture.rs`): the envelope returns in it, and Bloom's and Echo's wet returns ramp out linearly over it through `ReturnWeight`, so no reverb or echo tail rings on; the dry duck returns with the envelope, and a repress slews the return back up. Both processors then clear incrementally, and a fresh press during clearing must accept new input.
- Zero envelopes with drained tails must pass dry audio unchanged. Mute and Master output protection remain downstream.
- Beat Repeat records the live Master mix continuously into preallocated history sized for one quarter note at `MASTER_BPM_MIN` (plus tolerance), and never allocates in processing. A press arms at the beat the hold began in (`timing.beat` grid line); at the next grid line it copies that beat plus a 2 ms pre-roll and loops it. The slice length is the measured samples between the two grid lines, accepted only within a quarter of the nominal beat, with real pre-roll behind it; otherwise the hold re-arms on that line. The loop wrap crossfades the tail into the pre-roll so the seam lands on the audio that originally followed the slice. The length is latched at loop start, so a tempo change mid-hold does not retime it. Key-up is quantized: the loop plays on to the next grid line, then its weight slews to zero over `GESTURE_RELEASE_SECONDS`; the gesture envelope is only the key, not the loop's weight. A release before the loop engages goes Idle at once; a stopped transport fades at once; a press before the line resumes the loop. A grid line that does not follow the previous beat (state landing, restart, downbeat reset) is a `grid_break`, and no slice may have pre-roll reaching back across it, because the song under the stage can swap in one sample and the loop would repeat that cut on every beat. The slice length is `BEAT_REPEAT_BEATS`; there is no selector. A loaded hold passes dry audio until a full beat of live audio exists. A loop already playing at a downbeat reset keeps its captured beat and phase; only its quantized release follows the new grid. Captured audio stays out of song codes; active scalar envelopes belong to the aggregate session.

## Work Guidance

- Verify audible changes and releases through rendered samples, including overlapping gestures. `wet_gestures_let_go_cleanly_within_the_release_time` renders a Bloom and an Echo release and requires no step larger than the held wash's and bit-exact dry output once the release time ends.
- After retuning any gesture constant, run the level probe: nothing over the ceiling, light limiter reduction on every song, and check the band deltas for the intended tilt.
- Tests that read output timing against `tempo.beat` or a frame index must allow for the output stage's lookahead (`master_bus.limiter.latency()`).

## Verification

- `cargo test fluid::engine::gesture_audio` covers temporary processing.
- `cargo test fluid::engine::downbeat_tests` covers the downbeat reset end to end.
- `cargo test fluid::gesture_audio_tests` covers raw terminal input through rendered audio and saves.
- `cargo test fluid::gesture_level_probe` renders a full Bloom+Echo+Submerge+Beat Repeat throw at real playback level through the whole engine and bounds the limiter's reduction to what a pre-makeup peak of 0.6 needs; it also holds every built-in song at Master Level 100%, dry and with every gesture, at or under the ceiling, and a hotter-than-any-song mix held at it.
- `cargo test --release engine_perf_probe -- --ignored --nocapture --test-threads=1` prints per-buffer render cost; record `uptime` beside it, since timings are wall-clock.
- `cargo test --release gesture_level_probe -- --ignored --nocapture` prints real-level peak, samples over the ceiling, limiter reduction, and low/mid/high band deltas per song and gesture.

## Child DOX Index
