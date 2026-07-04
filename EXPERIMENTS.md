# Experiment: audio-reactive node visualizer

Branch: `exp-audio-visuals`

## Theme

**One fluid, every voice strikes it, and silence is a still, black
screen.** The chord tones are the medium: four vibrating nodes stacked down
the center column, radiating micro-ripples in the chord's colour. Every
other element hits the same fluid and ripples through it, so kick waves
collide with chord ripples, tonal rings, everything. Close your eyes, open
them, and you should be able to tell which instruments are playing, where
the bass note sits, and when the chord changes, without hearing a thing.

## How it works

The audio thread publishes lock-free telemetry (`FluidTelemetry`): per-voice
trigger pulses, the last bass/tonal note pitch, a smoothed output level per
voice, plus chord index and beat. The UI thread reads it every frame, and
mirrors the pad's progression + master tune controls so it knows the *actual
four chord tones* the pad is sounding (`pad_chord`).

Everything sums into one field; hue blends by wave weight where sources
overlap, so collisions mix colour instead of flickering.

**Hard rule:** every element's brightness is its voice's *live output
level*, every frame of its life. No floors, no fixed visual clocks. A muted
voice draws nothing; a decaying voice fades its body at exactly the sound's
rate.

## Voice → visual mapping

| Voice | Body | What it shows |
|-------|------|---------------|
| **Pad / chords** | 4 vibrating nodes, center column | Each chord tone is a node at its pitch height (higher tone = higher node) radiating micro-ripples — finer and faster for higher tones. Chord changes glide the nodes to the new tones. Level drives it all + a faint chord-colour wash; pad silent → field black and still. |
| **Bass** | persistent node, low-center (0.50, 0.80), warm amber | Live level sets ring brightness (decay included); **note pitch sets the wavelength**. Each new note restarts the ring. |
| **Kick** | radial pulse from a point near the bottom | One wave per hit expands upward and outward, pushed hardest straight up; the origin wanders slowly around center, never hops. Immune to the telemetry publish race. |
| **Tonal** | ripple from its pitch spot | **Position = pitch** (pitch class → column, pitch → height); the same note always ripples from the same spot. Hue sits opposite the chord's. Rings on for the note's whole envelope. |
| **Perc** | ripple from one fixed home (0.20, 0.32) | Every hit strikes the same spot; fades at the voice's real decay rate. Hue offset from the chord's. |
| **Clap** | ripple from one fixed home (0.80, 0.32) | Mirrors perc on the right, its own hue offset. |

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
2. **Do the chord nodes carry the field?** At full chord volume the center
   column should visibly vibrate and colour the whole field. Does a chord
   change read as the nodes gliding and retuning?
3. **Do collisions read?** Kick waves passing through the chord ripples and
   the tonal rings — does the one-fluid feel land, or is it mush?
4. **Do the fixed homes work?** Perc always left, clap always right, tonal
   always at its pitch spot — is the placement now legible?
5. **Bright enough?** Chord visuals were too light before; the pad gain went
   up and the nodes add local energy. If it still reads dim, say where.
