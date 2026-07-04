# Experiment: audio-reactive node visualizer

Branch: `exp-audio-visuals`

## Theme

**Every audio element has a visual body, and silence is a still, black
screen.** The visuals are split into two layers: the low end lives *in* the
fluid, the high end rides *above* it. Close your eyes, open them, and you
should be able to tell which instruments are playing, where the bass note
sits, and when the chord changes, without hearing a thing. Mute everything
and the screen goes dark and still.

## How it works

The audio thread publishes lock-free telemetry (`FluidTelemetry`): per-voice
trigger pulses, the last bass/tonal note pitch, a smoothed output level per
voice, plus chord index and beat. The UI thread reads it every frame.

**Layer 1 — the fluid field (shared, interacting).** Pad, bass, and kick sum
into one field, so they visibly interfere: a kick wave travels through the
bass rings and the pad wash. Hue blends by wave weight where sources overlap.

**Layer 2 — the surface (discrete, crisp).** Tonal notes and perc/clap
glints are sparks composited over the field, so they stay sharp even when
the field is busy.

**Hard rule:** every element's brightness is its voice's *live output
level*. No floors, no unconditional spawns. A muted voice draws nothing.

## Voice → visual mapping

| Voice | Layer | Body | What it shows |
|-------|-------|------|---------------|
| **Pad / chords** | field | the ambient medium | Level drives the whole field's base simmer; **hue follows the chord** (5-colour table). Pad silent → field black and still. |
| **Bass** | field | persistent node, low-center (0.50, 0.80), warm amber | Level sets ring brightness; **note pitch sets the wavelength** — low notes are long slow rings, high notes pack more rings. Each new note restarts the ring. |
| **Kick** | field | coherent wavefront from the whole bottom edge | One wave rises per hit and decays; brightness strictly tracks the live kick level. Watch it ripple through the bass rings. |
| **Tonal** | surface | bright spark; **height = pitch**, cyan→green by pitch | Size and brightness from the live level; successive notes fan across the field. |
| **Perc** | surface | sharp glint, left flank (x ≈ 0.13), cool blue | One glint per hit, brightness from live level. |
| **Clap** | surface | sharp glint, right flank (x ≈ 0.87), magenta | Mirrors perc on the right. |

## Focus mode

Press **`V`** for "just watch it" mode: the control panel disappears and the
full field fills the screen. **Any key** returns to the controls.

## How to try it

```
cd worktrees/exp-audio-visuals
cargo run
```

First test: pull every volume to zero — the screen must go black and still.
Bring voices back one at a time and watch each body appear alone. Then let
it all play and watch the kick waves pass through the bass rings.

## Feedback questions

1. **Is silence actually still?** All volumes at zero should be a black,
   motionless screen. Any residual activity is a bug.
2. **Does the kick read as THE kick?** A coherent wave from the bottom per
   hit — is it satisfying, and does its brightness track your kick volume?
3. **Does the field interaction land?** Kick waves passing through bass
   rings and the pad wash — does the "unified fluid" feel survive, or do the
   layers feel disconnected?
4. **Are the surface sparks crisp enough?** Tonal notes at pitch height,
   perc left / clap right — can you point at each while everything plays?
5. **Is true-black silence right, or too dead?** The old build kept a faint
   ambient breath; now the pad level gates all ambient motion. Keep or
   soften?
