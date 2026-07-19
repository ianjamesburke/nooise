# North Star

## Mission

100% fun. 0% work. nooise never asks the user to do a task that isn't enjoyable — no surgical mixing, no correcting a bad default, no chores.

## Feature-evaluation commandments

Before adding any control or feature, weigh it against all three:

1. **Is this the most ergonomic way to reach the intended musical result?** If a simpler gesture gets the same result, use the simpler gesture.
2. **Is it a mechanical problem?** Controls exist to solve mechanical/expressive problems (timing, pitch, texture), not to compensate for something that's off.
3. **Could it be fixed upstream instead?** If a control exists only to correct an imbalance between layers, the fix is better mixing/default balance in the engine, not a knob that hands the user a mixing job.

If a proposed control fails #3 — it's there to let the user manually correct something that should already sound right — don't ship the control. Fix the balance instead.

**Concrete case:** an EQ-tilt ("brighten/darken") control is mixing-desk territory — surgical, not playful. `master.tone` already exists (`src/fluid/controls.rs:26`, `registry.rs:506-524`) and should be re-evaluated against this commandment rather than extended (e.g. folded into the [[0020]] effects-module catalog as a stint task, [[0017]]) — open question, not yet decided.

## Onboarding North Star: 15 seconds to fully up to speed

Anyone sits down — first day, zero context — and is completely oriented in 15 seconds. An expert's saved configuration hands off to a novice with no loss of usability; all complexity is hidden behind abstractions in the UI, never exposed as prerequisite knowledge.

The entire pitch is two rules:
- **Arrow keys** move.
- **Tab** moves through pages.

That's the whole floor. Any control that requires more than that to *get started* — before a user has opted into going deeper — breaks this North Star.

## Progressive disclosure: the tucked-away pattern

This isn't a ban on new features or new controls. It's about comfort: an advanced feature should sit inside the flow a beginner is already moving through, findable the way an Easter egg is findable — not signposted, not required to get started, but there to bump into.

The chord progression control is the clearest existing example (`pad.progression`, `src/fluid/registry.rs:817-834`): eight built-in progressions, A through H, and one step past the last one lands on "Custom," which opens a chord builder (`src/fluid/voice/pad.rs:553-628`, `src/fluid/controls.rs:66-87`). A user turning the same knob they've always turned finds the advanced tool sitting at the end of it. Nobody needs to know custom progressions exist to enjoy the eight built-ins.

**Idea (not yet built):** a small "↵" glyph on the right side of a control row whenever its current value can be drilled into — `pad.progression` would show it only when set to "Custom." Gives a curious user a visible hint that more exists without any docs or guessing that Enter does something.

## Aspirational: advanced ergonomics, vim-motions-for-music

Long-term, layer power-user ergonomics on top of the floor above — fast value entry, fuzzy-find, batched multi-slider edits (already theorycrafted in [[0019]]) — plus a further "vim motions for music" layer: composable, muscle-memory-speed navigation/editing for expert use.

**Constraint:** this layer is strictly additive. It must never raise the 15-second floor above — a first-day user who only knows arrow keys + Tab must remain fully capable, and unaware the advanced layer exists.

**Prior art (research, 2026-07-14)** — closest existing analogs, none a direct copy:
- [reaper-keys](https://github.com/gwatcha/reaper-keys) / [vimper](https://github.com/ggVGc/vimper) — literal vim-modal bindings ported onto the REAPER DAW. Key-sequence composition (motion + operator, e.g. `tL` = play+loop next measure), a searchable completion overlay, `Esc` always resets to a known state. Closest real-world precedent for "vim motions, but for music."
- Trackers (Renoise, Polyend Tracker, and the Amiga/Atari-era originals) — fully keyboard-driven grid sequencing, QWERTY rows mapped to piano keys, no mouse required. Proven at both software and hardware (Polyend) scale; the standing counter-argument to "music tools need a mouse."
- [ORCA](https://github.com/hundredrabbits/Orca) — 2D-grid livecoding esolang, one letter per operator, keyboard-navigated spatial grid, outputs MIDI/OSC. The clearest existing fusion of vim's spatial-navigation model with procedural music generation.
- Livecoding pattern languages ([TidalCycles](https://github.com/tidalcycles/Tidal)/Strudel, Sonic Pi, Gibber) — terse cyclic-pattern DSLs, hot-reloaded while playing. Different axis (text-as-interface, not modal navigation) but same underlying goal: compress expert intent into very few keystrokes. nooise's own song-code (compact serialized state) is already adjacent to this idea.

None of these are directly portable — nooise's control surface is sliders/pages, not a timeline or a text buffer — but they're the reference set to study before designing the advanced layer.
