# Engine helpers

## Purpose

Audio processing helpers owned by `../engine.rs`.

## Ownership

- `gesture_audio.rs` owns temporary gesture processing, parallel effect returns, and their bounded runtime storage.
- `../engine.rs` owns voice/Master routing, clock publication, and session reads.
- `../gesture.rs` owns the gesture vocabulary and scalar envelopes; shared effect algorithms remain in `../../fx/`.

## Local Contracts

- Reuse the shared module processor dispatch for Filter, Reverb, and Delay. Keep gesture state separate from user slots.
- Allocate gesture processor storage before sample processing. Target changes and effect tails must not grow storage.
- Bloom/Echo retire after bounded eight-/six-second tails with a smooth final fade and incremental clearing. A fresh press during clearing must accept new input.
- Zero envelopes with drained tails must pass dry audio unchanged. Mute and Master output protection remain downstream.
- Tail buffers stay out of song codes; active scalar envelopes belong to the aggregate session.

## Work Guidance

- Verify audible changes and release tails through rendered samples, including overlapping voice and Master gestures.

## Verification

- `cargo test fluid::engine::gesture_audio` covers temporary processing.
- `cargo test fluid::gesture_audio_tests` covers raw terminal input through rendered audio and saves.

## Child DOX Index
