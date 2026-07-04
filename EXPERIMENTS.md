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
level*, every frame of its life. No floors, no unconditional spawns, no
fixed visual clocks. A muted voice draws nothing; a decaying voice fades
its body at exactly the sound's rate.

## Voice → visual mapping

| Voice | Layer | Body | What it shows |
|-------|-------|------|---------------|
| **Pad / chords** | field | the flowing medium | Level drives the whole field's flow; **each chord has its own wave character** (spatial frequency + drift speed, morphed on change) plus its own hue. Pad silent → field black and still. |
| **Bass** | field | persistent node, low-center (0.50, 0.80), warm amber | Live level sets ring brightness (decay included); **note pitch sets the wavelength**. Each new note restarts the ring. |
| **Kick** | field | radial wave from a point near the bottom | One ring per hit expands upward and outward, pushed hardest straight up; the origin wanders slowly around center, never hops. Every hit waves — trigger capture is immune to the telemetry publish race. |
| **Tonal** | surface | bright spark; **position = pitch** (pitch class → column, pitch → height), cyan→green | The same note always lands at the same spot; higher notes sit higher. Brightness tracks the live envelope for the note's whole length. |
| **Perc** | surface | sharp glint, left flank (x ≈ 0.13), cool blue | One glint per hit; fades at the voice's real decay rate. |
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
2. **Does decay read?** Lengthen the tonal/perc decay knobs — do the sparks
   audibly-visibly hold and release with the sound?
3. **Does the radial kick land?** A ring from near the bottom pushing up and
   out — better than the old full-width band? Right size at birth?
4. **Do the chords flow like the original?** Each chord should have its own
   wave character, not just its own colour. Does a chord change visibly
   reshape the flow?
5. **Are the surface sparks crisp enough?** Tonal notes at pitch height,
   perc left / clap right — can you point at each while everything plays?
