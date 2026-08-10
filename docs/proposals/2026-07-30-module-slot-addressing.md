# Module slot addressing

Design for the storage and addressing half of the per-layer module system
([[0020]]). Supersedes the ID shape sketched in that task. The UI half (the `/`
add flow), family-shaped detail scopes, and stateful post-DSP execution are
included because their persistence is part of this permanent storage contract.

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
| module name in the ID | ~720 | +128 IDs, permanently |
| module name as a value | **512** | **0** |

With the name in the ID, every module the catalog ever gains burns another
block of permanent table entries, and the table can only grow. That is the
"song saves ballooning out of control" failure the whole design is meant to
avoid.

Instead a slot exposes **family-shaped** params plus a kind selector:

```
bass.slot3.kind     which catalog module is loaded (0 = empty)
bass.slot3.amount   primary param, every family has one
bass.slot3.time     left time (active unit depends on its clock)
bass.slot3.right_time
bass.slot3.clock    Sync | Free
bass.slot3.right_clock
bass.slot3.feedback
bass.slot3.vintage  delay wet-path colour / reusable unit parameter
```

`kind` is an ordinary discrete control holding an enum index, exactly the idiom
`pad.type`, `bass.voice_type` and `tonal.synth_type` already use and which
already round-trips through song codes. Adding a module to the catalog appends
to an enum and costs **zero** new IDs.

## Scope of the first cut

Deliberately conservative, because appending slots later is free and removing
them is impossible.

- **8 chains**: Pads, Perc, Bass, Kick, Tonal, Clap, Arp, and Master. Macros
  has no signal path.
- **8 slots per layer.** Not 16. Slots 9+ are a pure append whenever the eight
  prove tight.
- **8 IDs per slot** → 8 x 8 x 8 = **512 new IDs**, fixed forever.

Unoccupied slots cost **zero bytes** in a song code: container v2 prunes any
control sitting at its default, and an empty slot is `kind = 0` with all
remaining fields at their defaults. The ID count is a registry and compile-time concern,
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
    family: Family,            // SingleAmount | TwoKnob | Delay | Reverb | Compression
}
```

`family` decides which params are live, replacing [[0020]]'s `params:
&'static [ParamKind]`. A `SingleAmount` module ignores all detail values. The
fixed slot shape remains closed: later families reuse these fields or require
a deliberate append-only storage decision.

The append-only catalog retains Alcohol and Sidechain indexes, but they remain
unavailable until they have real execution paths. The addable set is Swing
(pre), plus Drive, Reverb, Delay, and Compression (post). Each catalog entry
owns an ordered static parameter schema used by the detail view and its scoped
palette aliases. Delay stores an
independent unit for each channel; `T` on Left Time or Right Time switches only
that row, without rendering separate mode controls. Sync-to-Free derives
milliseconds at the current BPM and rounds to the free-time step; Free-to-Sync
derives beats and rounds to the beat grid. It does not retain a second hidden
representation. Delay-time edits crossfade to the new tap position over a
fixed window instead of jumping the buffer and clicking or sliding the read
head into a pitch transition. Vintage
is one wet-only macro: its pitch depth rises across the sweep and blooms near
the end, alongside level-compensated
saturation without driving or detuning the dry signal. Reverb exposes
Amount/Size/Damping. Compression exposes Amount/Threshold/Ratio/Release/Makeup.

## Invariant: every module is inert at its defaults

Promoted from a performance note to a hard catalog rule, because the add flow
depends on it.

An empty slot (`kind = 0`) is skipped entirely. Beyond that, **every addable
catalog entry must be a no-op
at its default param values.** That is what makes adding a module safe to do
automatically from a search box: it changes nothing audible until a knob moves,
so it needs no confirmation and no bar-quantized commit.

A module that cannot be inert at rest does not belong in the catalog.

## Empty slots do not render

`tab_controls` filters out slots with `kind = 0`, the way
`chords_tab_controls` already filters chord slots by `pad.chord_count`. Eight
empty slots per layer must not appear as 24 blank rows — that would wreck the
15-second floor on every page.

An occupied slot renders one collapsed Amount row at the track root. Pads use
this same sibling effect-chain scope; module rows never enter the custom chord
progression or chord-slot drills. `Enter` opens a reusable
module-detail scope and `Esc` returns to that exact row. Within that scope the
slash palette exposes human paths such as `clap.delay.feedback`; those are
aliases only, resolving to the stable slot IDs above.

## Registry generation

A `module_slot_rows!($layer, $slot)` macro emits the eight specs, generalizing
the existing `chord_slot_rows!` pattern (registry.rs:755). Slot IDs are
compile-time `concat!` strings, so every slot param is automatically LFO-able
with no new plumbing — which is the reason [[0020]] required bounded static
slots in the first place: `ControlAddress` is `&'static str`-keyed and a
dynamic `Vec<Box<dyn Effect>>` cannot produce one.

## Consequences and open threads

- `song_ids_cover_every_registry_control` fails the build if any of the 512 new
  controls lacks a table slot, so none can become unsaveable.
- **Resolved:** every legacy Reverb, Drive, Swing, and Master Compression
  control was folded into the shared chains. The default song template
  preloads the former factory effects, while Swing stays optional. Retired IDs
  remain in the append-only table, but nothing translates them: a code that
  sets one is refused, and the built-in `AUTO_STATES` were re-authored through
  the current encoder instead.
- Slot reordering is out of scope, as in [[0020]]. Processing order is slot
  order.
- Delay, Reverb, and Compression run through one slot-addressed post-DSP bank
  in chain order. Delay buffers, Reverb tails, and compressor envelopes are
  never saved — a song code carries controls only, and a loaded song rebuilds
  every tail from playback.
- Pads, Tonal, Clap, and Arp have no separate ambient or voice-local Reverb
  paths. Their template Reverb slots and every newly added effect use the same
  module bank. Master Release exists only inside shared Compression detail.
- This lands before the format is frozen and Volume 1 mixtapes are cut.
  Appending to `SONG_ID_TABLE` is supported; freezing first would buy a
  migration on day one.
