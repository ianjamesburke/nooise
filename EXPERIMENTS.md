# Experiment: audio-reactive node visualizer

Branch: `exp-audio-visuals`

## Theme

**Every audio element has a visual body.** The background is no longer a generic
plasma with kick ripples. It is now a system of resonating **nodes** — one per
sounding voice — that radiate waves into a shared field. The field you see is
the superposition. Close your eyes, open them, and you should be able to tell
which instruments are playing, where the bass note sits, and when the chord
changes, without hearing a thing.

## How it works

The audio thread publishes lock-free telemetry (`FluidTelemetry`): per-voice
trigger pulses, the last bass/tonal note pitch, a smoothed output level per
voice, plus chord index and beat. The UI thread reads it every frame and does
all the visual math. Two kinds of body:

- **Persistent nodes** (bass, pad, and faint perc/clap flank glows) sit at a
  home position and radiate concentric waves whose amplitude tracks the voice's
  live level. When a voice goes silent its node fades to dark and still.
- **Transient wavelets** are spawned on each trigger (kick, tonal, perc, clap)
  and travel outward, then fade.

Waves from every source sum into the field with a `tanh` soft clamp. The
brightest local source colours each cell, so where two voices overlap you see
their hues meet. Cell brightness tracks local wave energy — quiet regions are
near-black, active regions light up.

## Voice → visual mapping

| Voice | Body | Home / position | What it shows |
|-------|------|-----------------|---------------|
| **Bass** | persistent node | low-center (0.50, 0.80), warm amber | Level sets brightness; **note pitch sets the wavelength** — a low note is long slow rings, a high note packs more rings. Each new note restarts the ring. |
| **Pad / chords** | persistent node | wide upper arc (0.50, 0.24), broad | Level sets brightness; **hue follows the chord** (5-colour table) and washes the whole field's ambient tint. Slow undulation. |
| **Kick** | transient ripples | bottom edge, warm white | One tight ripple rising from the bottom per hit (as before), scattered by golden angle. Brightness tracks kick level. |
| **Tonal** | transient wavelet | drifting mid-field; **height = pitch** (high notes higher), bright cyan→green by pitch | A short bright wavelet at the note's pitch-mapped position; successive notes fan across the field. |
| **Perc** | left-flank glint + faint glow | (0.13, mid) cool blue | Small sharp glint per hit on the left; a steady stream lights the left flank. |
| **Clap** | right-flank glint + faint glow | (0.87, mid) magenta | Small sharp glint per hit on the right; mirrors perc. |

## Focus mode

Press **`V`** for "just watch it" mode: the control panel disappears and the
full field fills the screen. **Any key** returns to the controls.

## How to try it

```
cd worktrees/exp-audio-visuals
cargo run
```

Let it play. Watch the bass rings in the lower center retune as the bassline
moves, the upper field shift colour on each chord change, tonal notes flick
across the middle at their pitch height, and the flanks flash on perc/clap.
Press `V` to hide the panel and just watch. Change voice levels in the tabs to
see nodes brighten and dim.

## Feedback questions

1. **Can you identify the bass by eye?** Does the low-center node clearly read
   as the bass, and can you see its note change (wavelength) as the line moves?
2. **Is the chord change legible?** When the pad advances, does the colour shift
   register as "the chord changed"?
3. **Are the voices separable, or do they smear?** With everything playing, can
   you still point at each voice, or does the superposition turn to mush? Should
   individual nodes be more localized (smaller `reach`) or more contrasty?
4. **Are the level→brightness gains right?** Do quiet voices stay visible, and
   do silent voices actually go dark? (Per-voice gains are `*_LEVEL_GAIN` in
   `ui.rs` — easy to retune.)
5. **Is focus mode (`V`) worth keeping, and does "any key returns" feel right,**
   or would you want a dedicated exit key so you can still adjust while watching?
