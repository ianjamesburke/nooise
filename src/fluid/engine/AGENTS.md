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
- Gestures never raise the master: returns trade dry level for wet (Bloom/Echo duck the dry path), filters carry no resonance, and a full throw of any gesture stays within about +2 dB sample peak of the song without it. The Master output stage is a hard clamp, so headroom is the gesture stage's job.
- Bloom/Echo retire after bounded eight-/six-second tails with a smooth final fade and incremental clearing. A fresh press during clearing must accept new input.
- Zero envelopes with drained tails must pass dry audio unchanged. Mute and Master output protection remain downstream.
- Tail buffers stay out of song codes; active scalar envelopes belong to the aggregate session.

## Work Guidance

- Verify audible changes and release tails through rendered samples, including overlapping gestures.
- After retuning any gesture constant, run the level probe and keep every peak delta near 0 dB.

## Verification

- `cargo test fluid::engine::gesture_audio` covers temporary processing.
- `cargo test fluid::gesture_audio_tests` covers raw terminal input through rendered audio and saves.
- `cargo test --release gesture_level_probe -- --ignored --nocapture` prints peak/RMS deltas per song and gesture.

## Child DOX Index
