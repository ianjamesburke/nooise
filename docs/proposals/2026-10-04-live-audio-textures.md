# Live audio tracks and three sound explorations

Status: agreed product direction; proposed design and listening milestones. This document does not change runtime behavior. Audio capture, routing, DSP algorithms, and control ranges still need implementation design and audition.

## Direction

Add audio-input tracks so a microphone or external synth can be heard through nooise and processed with its shared effects. Explore three sound families: resonant objects, granular soundscapes, and bowed textures. Resonators and granular processing should work on incoming audio as well as nooise's synthesized sounds.

Follow the [North Star](../NORTH_STAR.md): generate instrument sounds procedurally, bundle no recorded samples, and allow live audio processing with temporary buffers. Borrow modular synthesis's relationship between an excitation and a resonating body, while keeping the interface approachable through normal browsing.

## First milestone: audio-input tracks

An audio-input track receives a selected hardware input, provides explicit monitoring, and feeds the same shared effects system as an instrument layer. A microphone, guitar, or external synth becomes a playable source within nooise.

Proposed signal path:

```text
Input device/channel -> audio-input track -> shared effect slots -> master/output
Internal synth layer --------------------> shared effect slots -> master/output
```

An internal synth can host Resonator or Granular directly in its own effect slots. Sending a synth into another track is a separate routing decision, not a prerequisite for hearing these effects on it.

The initial audio-track scope is live input and monitoring. Recording to disk, imported clips, a timeline, and persistent audio capture remain outside this first milestone.

Proposed behavior:

- Select an input device and a mono channel or stereo pair; show signal activity and connection status.
- Make monitoring an explicit action. Start new input tracks with monitoring off, and avoid silently opening a microphone when loading a song.
- Provide track level, mute, and the existing shared effect-slot interaction. Add effects through `/`; keep arrows and Tab available throughout.
- Preserve the dry signal when effects are bypassed. Resolve duplicate hardware direct monitoring during listening checks so it does not disguise latency or create comb filtering.
- On input loss, stop feeding stale audio, let bounded effect tails decay, and show the missing source. Never silently substitute another microphone.
- Keep capture and DSP bounded: preallocated buffers, defined overflow/underflow behavior, and no blocking device work in the audio callback. Device selection, sample-rate differences, and input/output clock drift need an explicit design before coding.

Song codes should carry portable track configuration, routing intent, effect controls, and chosen persistent modulation state. They must not contain captured audio or device-specific handles. Decide the local device-binding format and load-time monitoring policy before implementation. Loading a code restores the processing setup, not an earlier live performance.

## 1. Resonant objects

Listening target: ceramic knocks, bronze strikes, glass bells, and hollow metallic percussion. Each character should have a recognizable body when heard dry.

Proposed engine: a bank of damped resonances. Their frequencies, strengths, and decays describe an imagined object. A synthesized impulse or noise burst excites it as an instrument; microphone or synth audio excites it as a shared Resonator effect. Both uses should share the resonator DSP instead of developing unrelated versions.

Start with Ceramic, Bronze, and Glass as audition recipes. Proposed controls are character, tuning, decay, and effect Amount. Author comparable levels and useful defaults in the engine. Explore optional tuning to the song's harmony without making every object sound like the same pitched bell.

Listening milestone: play the same short phrase through all three characters, then feed speech, tapping, and a synth phrase into the effect. Quiet details should excite an audible body; loud transients should stay bounded. At zero Amount, the effect must return the dry input exactly. Repeated strikes, tuning changes, and long decays must remain stable and free of unintended clicks.

## 2. Granular soundscapes

Listening target: a voice spreading into a cloud, a synth phrase becoming suspended particles, and rhythmic fragments drifting around a recognizable live source.

Proposed engine: a short rolling buffer of incoming audio read by overlapping, windowed grains. Vary grain duration, density, read position, pitch, and stereo position within bounded ranges. This is live waveform manipulation and requires no bundled samples or disk recording.

Ship this exploration as a shared Granular effect on audio-input tracks and suitable synth layers. Start with a few authored characters, such as tight flutter, tonal shimmer, and diffuse cloud. Expose only the controls that make those results easier to play; exact names and ranges depend on audition. Pitch spread can offer harmonically related intervals as a proposed default, but that does not imply automatic pitch correction of microphone audio.

Proposed first version continuously refreshes its buffer. Freeze and indefinite capture are deferred because a held external sound cannot be restored by a short song code under the current no-audio persistence rule. A reload starts with an empty audio buffer and fills from the current source; it must fade in without stale memory or a burst of sound.

Listening milestone: compare dry and processed speech, sustained synth notes, and a percussive phrase. The effect should move from clear fragments to a continuous cloud without accidental gaps, zipper noise, or runaway level. Measure grain limits, CPU use, memory, startup behavior, and algorithmic delay, including at extreme controls and after input loss.

## 3. Bowed textures

Listening target: warm sustained strings, glassy upper harmonics, and gentle friction that responds to movement. A long note should feel continuously energized.

Proposed engine: a string model with continuous friction excitation. A noise-fed resonator is a useful early comparison, but it is not proof of a convincing bowed string. Audition both before choosing the implementation depth.

Begin with a self-contained procedural instrument character. Proposed expressive dimensions are bow pressure, movement, and contact position, combined into a small playable surface after listening. Preserve musical pitch and usable loudness across the ordinary range. More abrasive sounds can live at deliberate extremes.

Microphone-driven excitation of the resonator belongs to the Resonator effect above. A separate bow model can share resonator or string primitives where they fit; it should not force every external-input effect through a friction model.

Listening milestone: hold a note, move slowly from a soft onset into stronger friction, and play a short chord progression. Listen for convincing sustain, stable pitch, useful low and high registers, and changes without accidental squeals or large level jumps.

## Delivery order and evaluation

1. Prove one audio-input track with dry monitoring, one existing shared effect, explicit source selection, and device-loss handling.
2. Audition resonant objects as both procedural percussion and an effect on the input track.
3. Audition Granular on the same live input and on an internal synth layer.
4. Compare bowed synthesis approaches and retain the one that sounds and feels worth playing.

The sound engines can have isolated prototypes before capture is complete. The live Resonator and Granular milestones depend on the audio-track foundation.

For each milestone, pair deterministic DSP checks using generated test signals with hands-on listening. Test mono and stereo input, effect bypass, rapid control edits, silence, loud transients, source loss, and song-code reload. Measure end-to-end monitoring latency on actual hardware and publish the settings used; choose a latency budget before accepting the capture design. CPU and buffer limits need measurements at the supported sample rates and track counts.

Live acceptance requires an actual microphone or external synth and a separate internal-synth pass. Use headphones for the first microphone check to avoid speaker feedback. Rendered audio and successful device opening alone do not establish live usability.

## Decisions still open

- Initial track count, device selection UI, and whether separate input/output devices are supported in the first version.
- Local hardware bindings versus portable song-code routing, and the precise monitoring behavior on reload or reconnect.
- Buffer duration, grain/polyphony limits, sample-rate conversion, latency budget, and CPU budget.
- Whether internal sends are useful enough to add after direct per-layer effects work.
- Final character names, control ranges, and the depth of the bowed-string model, chosen by audition.

Accepted architecture should be recorded under [ADRs](../adr/AGENTS.md) when these decisions are made. Shipped interaction behavior belongs in [Performance](../PERFORMANCE.md).
