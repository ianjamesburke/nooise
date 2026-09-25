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
- Bloom/Echo retire after bounded eight-/six-second tails with a smooth final fade and incremental clearing. A fresh press during clearing must accept new input.
- Zero envelopes with drained tails must pass dry audio unchanged. Mute and Master output protection remain downstream.
- Tail buffers stay out of song codes; active scalar envelopes belong to the aggregate session.

## Work Guidance

- Verify audible changes and release tails through rendered samples, including overlapping gestures.
- After retuning any gesture constant, run the level probe: zero clamp hits on every song, and check the band deltas for the intended tilt.

## Verification

- `cargo test fluid::engine::gesture_audio` covers temporary processing.
- `cargo test fluid::gesture_audio_tests` covers raw terminal input through rendered audio and saves.
- `cargo test fluid::gesture_level_probe` renders a full Bloom+Echo+Submerge throw at real playback level through the whole engine and requires zero clamp hits and a peak under 0.6; the clamp is the backstop, never the fix.
- `cargo test --release gesture_level_probe -- --ignored --nocapture` prints real-level peak, clamp hits, and low/mid/high band deltas per song and gesture.

## Child DOX Index
