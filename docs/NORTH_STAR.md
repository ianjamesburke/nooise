# North Star

## Mission

100% fun. 0% work. nooise never asks the user to do a task that isn't enjoyable — no surgical mixing, no correcting a bad default, no chores.

## Feature-evaluation commandments

Before adding any control or feature, weigh it against every commandment below:

1. **Is this the most ergonomic way to reach the intended musical result?** If a simpler gesture gets the same result, use the simpler gesture.
2. **Is it a mechanical problem?** Controls exist to solve mechanical/expressive problems (timing, pitch, texture), not to compensate for something that's off.
3. **Could it be fixed upstream instead?** If a control exists only to correct an imbalance between layers, the fix is better mixing/default balance in the engine, not a knob that hands the user a mixing job.

4. **Does it change what a song code carries?** A code saves the state a person chose, and nothing else. It never saves audio — no delay lines, no reverb tails, no analysis buffers, no envelope followers. If it rebuilds itself from a second of playback, it is not state, it is sound, and sound is not saved. A code has to stay short enough to paste into a message; the day one Delay slot pushed a code past a million characters, the code stopped being shareable and the feature stopped existing.

   The chosen state is musical state. Device routing and gear-dependent switches belong to launch setup under Commandment 6. A song code is one compact snapshot; a future session document records a timed performance, and a rig declaration describes external devices. Recording a performance must not make every shared song code carry a full history or make file management necessary to start playing.

5. **Does another layer already have it as a module?** A capability shared across layers (Swing, Drive, Filter, Room, Delay, Compression) is one module in the catalog, added through `/` the same way on every layer it fits. Never give one layer a private control for it: the palette then shows one layer's version as a plain row next to everyone else's module, and it reads as a bug. A layer the module does not fit is excluded in `module_available_on` for a sound reason, not a UI one. Pads' private Swing row was retired onto the module for exactly this; `no_layer_duplicates_an_available_module_as_a_bespoke_control` enforces it.

6. **Is the computer keyboard still the best-supported way to play?** Every musical capability must have a complete, discoverable keyboard path. Optimize ordinary browsing and searchable `/` operations first, preserving the arrows-and-Tab onboarding floor. External gear is optional and uses the same validated musical operations. A plain launch must remain playable without configuring a rig.

7. **Does it stay free of bundled samples?** nooise generates instrument sounds through synthesis, physical models, and waveform manipulation; it does not bundle sample libraries or recorded instrument assets. Live audio from microphones or synths may be processed, including granular effects using temporary rolling audio buffers, without requiring a recording saved to disk.

8. **Does adding an effect preserve the sound until its amount is moved?** New effect modules and modulation recipes start in a passthrough state, with Amount at zero where applicable. Inserting a recipe must not alter the target's base value. Focus the new recipe's Amount so the player can bring it in immediately. `/drift` uses a four-beat cycle and starts at zero Amount. Current recipe defaults still need to adopt this rule.

**Retirement, not migration.** When a control goes away, its saved values go away with it. Codes that carry a retired control are refused with a message naming it (`SongCodeError::RetiredControl`), never loaded with the value silently dropped to a default. Built-in songs are re-authored through the current encoder instead; nooise carries no translation layer between old and new control names.

If a proposed control fails #3 — it's there to let the user manually correct something that should already sound right — don't ship the control. Fix the balance instead.

**Concrete case:** an EQ-tilt ("brighten/darken") control is mixing-desk territory — surgical, not playful. `master.tone` already exists (`src/fluid/controls.rs:26`, `registry.rs:506-524`) and should be re-evaluated against this commandment rather than extended (e.g. folded into the [[0020]] effects-module catalog as a stint task, [[0017]]) — open question, not yet decided.

## External gear and persistence

The priority is the computer keyboard workflow, shared musical behavior and persistence, then optional device adapters. Stabilize existing features before the next hardware sprint; APC40 support is wanted soon afterward, while microphone input is actively being explored separately. MIDI, OSC and Ableton Link may extend the same instrument; incoming controls must use the same registry validation and session publication rules as keyboard edits. Physical devices must not become prerequisites for editing or playing a song.

| Musical state | Launch or rig configuration |
| --- | --- |
| Chords, notes, sound, Master Tune, rhythm, automation, mutes, and chosen evolving state | MIDI ports/channels, per-layer MIDI In/Out, Pad MIDI triggering, MIDI output gate settings, MIDI row visibility, OSC destinations, Link participation, and device mappings |

Recorded musical notes or actions may belong to a future session document even when they originated on a MIDI keyboard. The originating port does not become necessary to replay them internally.

**Implementation gap:** current song codes still save Pad/Arp/Lead MIDI direction, Pad MIDI trigger, Arp/Lead MIDI gate, and MIDI row visibility. Stint 0069 owns moving these settings to launch configuration and the explicit incompatible-code refusal or format cut. Until that change lands, preserve their current decoding; this policy does not authorize silently discarding saved fields. Stint 0065's future song format must use the classification above.

Keep adapters in the same application while a plain launch preserves its sound, first screen, and onboarding. Revisit the boundary if stage work changes that default, needs a persistent setlist/venue interface, or requires gig releases to freeze while the ambient player advances. First extract a shared engine library when one of those pressures becomes concrete; a second application is a separate decision.

## Onboarding North Star: 15 seconds to fully up to speed

Anyone sits down — first day, zero context — and is completely oriented in 15 seconds. An expert's saved configuration hands off to a novice with no loss of usability; all complexity is hidden behind abstractions in the UI, never exposed as prerequisite knowledge.

The entire pitch is two rules:
- **Arrow keys** move.
- **Tab** moves through pages.

That's the whole floor. Any control that requires more than that to *get started* — before a user has opted into going deeper — breaks this North Star.

## Progressive disclosure: the tucked-away pattern

This isn't a ban on new features or new controls. It's about comfort: an advanced feature should sit inside the flow a beginner is already moving through, findable the way an Easter egg is findable — not signposted, not required to get started, but there to bump into.

The chord progression control is the clearest existing example (`pad.progression` in `src/fluid/registry.rs`). Arrow keys choose a complete built-in progression or Custom. Enter opens its eight chords, and Enter on a chord opens the shared builder. Root, quality, extensions, bass and spacing describe the chord already playing. A first edit changes that slot; switching progressions keeps each one's edits. `/restore chord` returns it to the authored version. Beginners can keep choosing whole progressions while curious players go deeper on the same row.

## Aspirational: advanced ergonomics, vim-motions-for-music

Long-term, layer power-user ergonomics on top of the floor above — fast value entry, fuzzy-find, batched multi-slider edits (already theorycrafted in [[0019]]) — plus a further "vim motions for music" layer: composable, muscle-memory-speed navigation/editing for expert use.

**Constraint:** this layer is strictly additive. It must never raise the 15-second floor above — a first-day user who only knows arrow keys + Tab must remain fully capable, and unaware the advanced layer exists.

**Prior art (research, 2026-07-14)** — closest existing analogs, none a direct copy:
- [reaper-keys](https://github.com/gwatcha/reaper-keys) / [vimper](https://github.com/ggVGc/vimper) — literal vim-modal bindings ported onto the REAPER DAW. Key-sequence composition (motion + operator, e.g. `tL` = play+loop next measure), a searchable completion overlay, `Esc` always resets to a known state. Closest real-world precedent for "vim motions, but for music."
- Trackers (Renoise, Polyend Tracker, and the Amiga/Atari-era originals) — fully keyboard-driven grid sequencing, QWERTY rows mapped to piano keys, no mouse required. Proven at both software and hardware (Polyend) scale; the standing counter-argument to "music tools need a mouse."
- [ORCA](https://github.com/hundredrabbits/Orca) — 2D-grid livecoding esolang, one letter per operator, keyboard-navigated spatial grid, outputs MIDI/OSC. The clearest existing fusion of vim's spatial-navigation model with procedural music generation.
- Livecoding pattern languages ([TidalCycles](https://github.com/tidalcycles/Tidal)/Strudel, Sonic Pi, Gibber) — terse cyclic-pattern DSLs, hot-reloaded while playing. Different axis (text-as-interface, not modal navigation) but same underlying goal: compress expert intent into very few keystrokes. nooise's own song-code (compact serialized state) is already adjacent to this idea.

None of these are directly portable — nooise's control surface is sliders/pages, not a timeline or a text buffer — but they're the reference set to study before designing the advanced layer.
