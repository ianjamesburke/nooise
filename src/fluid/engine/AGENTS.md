# Engine helpers

## Purpose

Audio processing helpers owned by `../engine.rs`.

## Ownership

- `gesture_audio.rs` owns the Master-bus gesture stage, parallel effect returns, and their bounded runtime storage.
- `../engine.rs` owns voice/Master routing (gestures run after Master's slot chain, before the master bus), clock publication, and session reads.
- `../gesture.rs` owns the gesture vocabulary and scalar envelopes; shared effect algorithms remain in `../../fx/`.

## Local Contracts

- Reuse the shared module processor dispatch for Filter, Reverb, and Delay. Keep gesture state separate from user slots.
- Allocate gesture processor storage before sample processing. Presses and effect tails must not grow storage.
- Gestures never clip the master: returns trade dry level for wet (Bloom/Echo duck the dry path), filters carry no resonance, and Bloom's send is high-passed at 300 Hz so sustained lows cannot pile up in its combs. At real playback level a full throw never touches the clamp. The Master output clamp stays as a backstop; headroom is the gesture stage's job.
- Every gesture lets go within `GESTURE_RELEASE_SECONDS` (10 ms, `fluid/gesture.rs`): the envelope returns in it, and Bloom's and Echo's wet returns ramp out linearly over it through `ReturnWeight`, so no reverb or echo tail rings on; the dry duck returns with the envelope, and a repress slews the return back up. Both processors then clear incrementally, and a fresh press during clearing must accept new input.
- Zero envelopes with drained tails must pass dry audio unchanged. Mute and Master output protection remain downstream.
- Tail buffers stay out of song codes; active scalar envelopes belong to the aggregate session.

## Work Guidance

- Verify audible changes and releases through rendered samples, including overlapping gestures. `wet_gestures_let_go_cleanly_within_the_release_time` renders a Bloom and an Echo release and requires no step larger than the held wash's and bit-exact dry output once the release time ends.
- After retuning any gesture constant, run the level probe: zero clamp hits on every song, and check the band deltas for the intended tilt.

## Verification

- `cargo test fluid::engine::gesture_audio` covers temporary processing.
- `cargo test fluid::gesture_audio_tests` covers raw terminal input through rendered audio and saves.
- `cargo test fluid::gesture_level_probe` renders a full Bloom+Echo+Submerge throw at real playback level through the whole engine and requires zero clamp hits and a peak under 0.6; the clamp is the backstop, never the fix.
- `cargo test --release gesture_level_probe -- --ignored --nocapture` prints real-level peak, clamp hits, and low/mid/high band deltas per song and gesture.

## Child DOX Index
