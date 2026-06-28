# GOTCHAS

## Perc decay → white noise transition: crossfade doesn't kill beating

**Feature:** At the top 10% of `perc.decay_ms` (1820–2000ms), transition to pure continuous white noise with zero audible pulsing.

**Approach tried:** Added a `white_noise_blend` scalar and crossfaded `pulsed_out * (1 - blend) + continuous_out * blend`. Stopped firing new triggers at blend > 0. Still audibly beating.

**Why it fails:** The PercEngine fires `NoiseHit`s triggered by `GridTrigger` every 0.25 beats. Each hit has a linear amplitude decay. The "beating" is not just from discrete transients — it's from the **amplitude ripple** in the summed overlapping decays. With N overlapping linear-decay hits firing every T samples, their amplitude sum forms a periodic sawtooth ripple at the trigger rate. This ripple persists even when multiplied by a small `(1 - blend)` factor, because the ear is sensitive to amplitude modulation even at low levels.

**What to try instead:** The trigger mechanism itself must be eliminated, not attenuated. Start with a test that renders audio and measures amplitude modulation in the actual listening path. Do not rely only on `PercEngine` internals.

## Perc decay -> white noise transition: analytical RMS switch also failed

**Feature:** At the top 10% of `perc.decay_ms` (1820-2000ms), full decay should be pure continuous white noise with no audible pulsing.

**Approach tried:** At `decay_ms >= 1820`, hard-switched `PercEngine` away from `GridTrigger` and `NoiseHit` processing. Cleared existing hits, reset the trigger, used a dedicated continuous `WhiteNoise`, reused the same filter smoothing (`10^(filter*4-4)`), and scaled the continuous noise with a closed-form RMS estimate for overlapping linear-decay hits.

**Observed result:** Still audibly beating at full decay in `cargo run`.

**What this proves:** Tests that only prove "no `NoiseHit`s are scheduled" are too shallow. If the full app still beats after `PercEngine` stops firing hits, the remaining pulse is probably outside that narrow branch or comes from a control/modulation path still applied to the continuous signal.

**Likely boundaries to prove next:**
1. Full mix vs solo perc. Kick, clap, tonal voices, or pad/chord changes may be triggering the master compressor and amplitude-modulating the continuous noise.
2. Master bus vs raw perc. `MasterBus` gain reduction can turn other transients into perceived noise pumping.
3. Perc LFO vs no LFO. The continuous branch still applies `vol_lfo` through `effective_level`; it is not trigger-rate pulsing by design, but it is still amplitude modulation.
4. UI state vs engine state. Verify the live snapshot is actually at `decay_ms = 2000` and that the binary being heard is the checkout being edited.

**Next attempt:** Build an offline render harness or debug mode that can render raw perc, full pre-master mix, and post-master output for a fixed control snapshot. Measure envelope modulation around the reported pulse rate before changing DSP again.

**Reimagined control direction:** Add a beat/subdivision control for perc. The current perc engine is hard-wired to `0.25` beat triggers, which makes the decay knob fight the rhythm system. Let the user choose the beat value, and make one end of the range mean "no beat, full continuous stream." Fully open or fully closed should bypass discrete hits entirely instead of trying to disguise them with decay.
