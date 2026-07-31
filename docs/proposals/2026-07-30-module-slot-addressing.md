# Module slot addressing

Design for the storage and addressing half of the per-layer module system
([[0020]]). Supersedes the ID shape sketched in that task. The UI half (the `/`
add flow) and the DSP half (concrete module implementations) build on this and
are not decided here.

This document exists because the addressing decision is **permanent**.
`SONG_ID_TABLE` (`src/fluid/song_ids.rs`) is append-only: an entry is never
reordered, removed, or reused. Whatever shape lands becomes part of the on-disk
song-code format forever, so it gets settled before code.

## The decision

**A module's identity is a stored value. It is never part of a control ID.**

[[0020]] originally sketched `layer.slot7.sidechain.amount`. That puts the
module name in the ID, which multiplies the ID count by the catalog:

| shape | IDs today | cost of a new catalog module |
| --- | --- | --- |
| module name in the ID | ~640 | +128 IDs, permanently |
| module name as a value | **168** | **0** |

With the name in the ID, every module the catalog ever gains burns another
block of permanent table entries, and the table can only grow. That is the
"song saves ballooning out of control" failure the whole design is meant to
avoid.

Instead a slot exposes **family-shaped** params plus a kind selector:

```
bass.slot3.kind     which catalog module is loaded (0 = empty)
bass.slot3.amount   primary param, every family has one
bass.slot3.time     secondary param, two-knob families only
```

`kind` is an ordinary discrete control holding an enum index, exactly the idiom
`pad.type`, `bass.voice_type` and `tonal.synth_type` already use and which
already round-trips through song codes. Adding a module to the catalog appends
to an enum and costs **zero** new IDs.

## Scope of the first cut

Deliberately conservative, because appending slots later is free and removing
them is impossible.

- **7 layers**: Pads, Perc, Bass, Kick, Tonal, Clap, Arp. Macros has no
  signal path. Master already carries drive/tone/compression and needs its own
  reconciliation pass first.
- **8 slots per layer.** Not 16. Slots 9+ are a pure append whenever the eight
  prove tight.
- **3 IDs per slot** → 7 x 8 x 3 = **168 new IDs**, fixed forever.

Unoccupied slots cost **zero bytes** in a song code: container v2 prunes any
control sitting at its default, and an empty slot is `kind = 0` with both
params at their defaults. The ID count is a registry and compile-time concern,
not a code-size one.

## Slots are anonymous, and so is their domain

[[0020]] proposed two pools per layer, pre-synthesis and post-synthesis, and
then had to explain why the UI melds them back into one visible chain. That
split is unnecessary at the storage layer.

**Domain comes from the loaded module, not from the slot index.** All eight
slots are identical and can hold anything. Execution reads the chain twice:

1. Every slot whose module is `Domain::Pre` runs, in slot order, intercepting
   the grid trigger before a voice fires.
2. Every slot whose module is `Domain::Post` runs, in slot order, on the
   rendered signal.

This preserves the genuine causal ordering [[0020]] identified — a note-domain
module decides which note fires before synthesis, an audio-domain module
reshapes a signal that does not exist yet at that point — while giving the UI
one flat ordered list with no domain labels and no routing rules to explain.
It also halves the ID space against two fixed pools.

## Catalog shape

```rust
struct ModuleKind {
    id: &'static str,          // "sidechain" — fuzzy-find matching
    display_name: &'static str,
    domain: Domain,            // Pre | Post
    family: Family,            // SingleAmount | TwoKnob
}
```

`family` decides which params are live, replacing [[0020]]'s `params:
&'static [ParamKind]`. A `SingleAmount` module ignores `time`. This is what
keeps the param set closed at two, and therefore the ID count fixed.

The v1 catalog stays as [[0020]] specified it: Alcohol and Swing (pre),
Drive and Sidechain (post), with Alcohol as the worked example.

## Invariant: every module is inert at its defaults

Promoted from a performance note to a hard catalog rule, because the add flow
depends on it.

An empty slot (`kind = 0`) is skipped entirely, the same bypass idiom
`master.drive` already uses. Beyond that, **every catalog entry must be a no-op
at its default param values.** That is what makes adding a module safe to do
automatically from a search box: it changes nothing audible until a knob moves,
so it needs no confirmation and no bar-quantized commit.

A module that cannot be inert at rest does not belong in the catalog.

## Empty slots do not render

`tab_controls` filters out slots with `kind = 0`, the way
`chords_tab_controls` already filters chord slots by `pad.chord_count`. Eight
empty slots per layer must not appear as 24 blank rows — that would wreck the
15-second floor on every page.

An occupied slot renders one collapsed row (name plus primary value); `Enter`
drills into its secondary params.

## Registry generation

A `module_slot_rows!($layer, $slot)` macro emits the three specs, generalizing
the existing `chord_slot_rows!` pattern (registry.rs:755). Slot IDs are
compile-time `concat!` strings, so every slot param is automatically LFO-able
with no new plumbing — which is the reason [[0020]] required bounded static
slots in the first place: `ControlAddress` is `&'static str`-keyed and a
dynamic `Vec<Box<dyn Effect>>` cannot produce one.

## Consequences and open threads

- `song_ids_cover_every_registry_control` fails the build if any of the 168 new
  controls lacks a table slot, so none can become unsaveable.
- **Resolved 2026-07-31, closing [[0017]]:** the six per-voice effect sliders
  (`perc.swing`, `tonal.swing`, `arp.swing`, `bass.drive`, `kick.drive`,
  `clap.room`) were folded into pre-loaded slot 1 on their layer at their old
  defaults. One mechanism, unchanged factory sound, and a layer that never had
  an effect can now be given one. Done before the release deliberately:
  container v2 had already broken every old song code and nothing had shipped,
  so retiring those ids cost no migration. That window closes at Volume 1.
  `master.drive` is untouched — Master has no module slots yet and needs its
  own reconciliation pass alongside tone and compression.
- Slot reordering is out of scope, as in [[0020]]. Processing order is slot
  order.
- This lands before the format is frozen and Volume 1 mixtapes are cut.
  Appending to `SONG_ID_TABLE` is supported; freezing first would buy a
  migration on day one.
