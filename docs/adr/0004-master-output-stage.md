# ADR 0004: Master output stage — fixed makeup and a -1 dBFS lookahead limiter

- Status: Accepted
- Date: 2026-11-11
- Scope: the end of `MasterBus::process` (`src/fluid/engine.rs`) and
  `src/fx/limiter.rs`. Song codes, Master Level's 0..100% range, layer levels,
  and the gesture stage are unchanged.

## Context

At 100% Master Level nooise played far quieter than ordinary line-level
material; listeners had to turn their headphones well up. The mix was
balanced for headroom, and nothing after Master Level restored loudness:
`MasterBus` multiplied by Master Level and hard-clamped at 0.95.

Measured through the whole engine (`loudness_probe`, BS.1770-4 integrated
loudness, 10 s start-up skipped, cross-checked against `ffmpeg ebur128`), the
21 built-in songs at their authored Master Level (70-80%) read -29.4 to
-16.7 LUFS, median -21.9, with sample peaks of -16.7 to -8.8 dBFS. At 100%
they read -27.4 to -14.7 LUFS, median -20.0. Spotify normalizes playback to
-14 LUFS integrated (BS.1770) by default
([Loudness normalization](https://support.spotify.com/us/artists/article/loudness-normalization/)),
so typical nooise songs came out about 6 LU under what the listener's ears
and volume knob were calibrated to.

The peaks left that headroom unused: no song came within 6 dB of the clamp,
and the clamp would have clipped any makeup that reached it.

## Decision

After Master Level, `MasterBus` applies a fixed makeup, `MASTER_MAKEUP`
(+6 dB), then `fx::limiter::Limiter`, which replaces the 0.95 clamp:

- Ceiling `CEILING` = -1 dBFS sample peak, the same ceiling as the
  downstream mixer's limiter, so nooise never feeds it a peak it has to catch.
- Stereo-linked lookahead peak limiter: the required gain
  (`CEILING / peak`) goes into a 20 ms sliding-minimum hold, recovers with a
  100 ms exponential release, and is smoothed by a 1.5 ms moving average.
  The audio is delayed by the averaging window minus one sample, so every
  output sample's gain is at or below what its own peak required. The ceiling
  holds by construction, and gain reduction arrives as a 1.5 ms ramp, not a
  step.
- Under the ceiling the gain is exactly 1.0, so the stage is a pure delay
  plus the makeup.
- Latency is the lookahead, 1.5 ms (72 samples at 48 kHz). Storage is sized
  from the sample rate once, when the engine is built. Processing never
  allocates.

+6 dB puts the median built-in song at 100% Master Level at -14.0 LUFS
(range -21.4 to -8.8). At authored levels the median is -15.9 LUFS. No song
puts a sample over the ceiling, and the limiter costs the loudest song
(9) about 0.1 LU. Master Level still scales
the signal linearly before the stage, so 0% is silence and 50% is -6 dB.

### One fixed makeup, not per-song gain

The built-in songs span 12.7 LU at their authored levels. A fixed makeup
shifts every song by the same amount and keeps that spread; the stage does
not normalize. Per-song gain would mean writing loudness into song codes or
measuring the program live. The first changes what a code carries and the
song's authored balance. The second is an AGC, which pumps over a phrase.
The spread is an authoring balance. It is fixed by re-authoring a song's
Master Level through the current encoder, not by the output stage.

### Hold and release

The hold outlasts a bass half-cycle (25 ms at 20 Hz is the extreme; 20 ms
covers 25 Hz and up), so the gain does not ripple between waveform peaks
and distort lows. The release is long against the hold and short against a
beat (500 ms at 120 BPM): after a kick-driven peak the gain is back within
about a third of a beat. Built-in songs drive the limiter rarely and
lightly (see Verification), so the release is chosen for transparency on
occasional peaks rather than for density. For comparison, the limiter
Spotify applies in its Loud mode engages at -1 dB sample value with 5 ms
attack and 100 ms decay (same source as above).

## Alternatives

- **Raise the clamp to 1.0 and add makeup.** Peaks over full scale would
  clip in the driver, and the clamp itself is hard clipping.
- **Soft clipper (tanh) instead of a limiter.** No latency, but it adds
  harmonics on every peak and its threshold lowers the clean range. It also
  cannot honour an exact ceiling without a hard clip after it.
- **Feed-forward compressor without lookahead.** Its attack lets the leading
  edge of each kick through, so it still needs a clamp, and a fast attack
  distorts lows.
- **Per-song normalization.** Rejected above.

## Consequences

- Output is 1.5 ms later than the engine clock. Tests that compare output
  against `tempo.beat` or a frame index account for
  `master_bus.limiter.latency()`.
- Gestures keep their own headroom contract: the gesture stage must keep
  its pre-makeup peak under 0.6. The limiter is a ceiling, not gain staging.
- `nooise render` writes the limited output, so its wavs carry the same
  loudness as live playback.
- Sample peak, not true peak: inter-sample peaks can exceed -1 dBFS by a
  fraction of a dB after a reconstruction filter. The downstream mixer's
  limiter is the backstop for that.

## Verification

- `cargo test fx::limiter` covers bit-exact passthrough under the ceiling,
  no sample over the ceiling for hot noise, kicks, and a 45 Hz sine at
  44.1-192 kHz, exact return to unity after a peak, and no allocation.
- `cargo test fluid::gesture_level_probe` holds every built-in song at
  100% Master Level, dry and with every gesture held, at or under the
  ceiling. It also holds a mix with every layer and Master at 100% at the
  ceiling (without the limiter it peaks at 0.986), and bounds the
  gesture-throw limiter reduction.
- `cargo test --release loudness_probe -- --ignored --nocapture` prints the
  per-song LUFS / peak / crest table. `engine_perf_probe` prints the
  limiter's cost alone.
- Listening is the remaining check: kick punch, pumping on dense songs at
  100% with gestures, and fatigue over a full set.
