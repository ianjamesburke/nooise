# Effects module system: `Effect` trait / per-instrument stackable chain

Design-only proposal for stint 0017. No engine code changes accompany this doc.

## Current state (grounded in code)

Four independent effect patterns exist today, none sharing an abstraction:

1. **Shared ambient bus** — `AmbientReverbSend` (`src/fluid/engine.rs:421-479`) owns one
   `Freeverb` instance. `FluidEngine` calls `process(pad, tonal, arp, pad_mix, tonal_mix,
   arp_mix)` once per sample, mixing three voices' sends into one reverb tank and
   returning per-voice dry gain plus a single wet pair. Send levels
   (`AMBIENT_REVERB_PAD_SEND` etc.) are fixed constants; only the mix (dry/wet balance,
   `pad.reverb_mix` / `tonal.reverb_mix` / `arp.reverb_mix`) is a per-voice registry
   control.
2. **Private per-voice reverb** — `ClapEngine` (`src/fluid/voice/clap.rs:7-42`) owns its
   own `Freeverb`, wired directly into `ClapEngine::next`. `clap.room` (`registry.rs:1404`)
   drives both the send level *and* selects a dry/wet blend inline (`dry_scale = 1.0 -
   room*0.5`) — a different mix law than the ambient bus's `dry_gain`.
3. **Bass lowpass** — `BassLowPass` (`src/fluid/voice/bass.rs:89-107`), one-pole RC filter,
   owned directly by `BassEngine`, applied post-voice-dispatch to the summed stereo
   output. `bass.cutoff` recomputes the filter coefficient every sample (supports
   LFO/envelope modulation) and a max-value bypass check skips the `process` call
   entirely to preserve byte-identical default renders.
4. **Tonal low cut** — `TonalLowCut` (`src/fluid/voice/tonal.rs:471-496`), a fixed-coefficient
   highpass computed once at construction, no user-facing control, applied unconditionally
   before mixing.

Every one of these is hand-wired: a struct field on the owning engine, a call site inside
that engine's `next`, and (for 1-3) a `ControlSpec` row wired directly to that struct's
field via a `get`/`set` closure pair. There is no `Effect` trait, no `Vec<Box<dyn Effect>>`,
no `fx/chain.rs`. `src/fx/reverb.rs` is a single concrete DSP struct, not an abstraction.

The registry (`src/fluid/registry.rs`) is one static table of `ControlSpec` rows, each with
a compile-time string ID (`"bass.cutoff"`, `"clap.room"`, ...), a `fn(&FluidControls) -> f32`
getter and `fn(&mut FluidControls, f32)` setter closure baked in at compile time. `song.rs`'s
snapshot codec is fully generic over `all_specs()` — any state expressible as a
`ControlSpec` row round-trips automatically. This registry shape is the central constraint:
IDs are compile-time string literals tied 1:1 to a fixed field on `FluidControls`. Nothing
today creates a `ControlSpec` row at runtime, and nothing today needs to.

## The core tension

The stackable-module UX vision (fuzzy-find picker, add modules one at a time, arbitrary
order per instrument) implies **dynamic effect instances** — the user can add a second
delay, or skip a filter entirely, per song. But the registry's `ControlSpec` is a **static
table of compile-time IDs** wired to fixed struct fields, and song-code persistence is
built entirely on that static table. These two things are in direct conflict, and that
conflict is the actual hard part of this design — not the trait shape.

## Alternatives considered

**A. Static per-voice chain slots.** Each voice type ships a fixed, compile-time list of
effect slots (e.g. Bass always has `[filter_slot]`, Clap always has `[reverb_slot]`, a
future voice might have `[filter_slot, reverb_slot, delay_slot]`). Each slot holds an
`Option<Box<dyn Effect>>` or an enum-of-effect-types, and its params are ordinary
`ControlSpec` rows (`bass.slot1.cutoff`, etc.) exactly like today. This preserves the
registry's static-ID model completely — zero changes to `song.rs` or `registry.rs`'s
fundamental shape. It gets you the trait (shared code between reverb/filter/whatever) and
the shared-bus-vs-private reconciliation, but it does **not** deliver the fuzzy-find
"stack any module in any order" UX — slots are still fixed per voice at compile time, just
now polymorphic in *what* fills them.

**B. Fully dynamic per-instrument chains.** Each voice owns a `Vec<Box<dyn Effect>>` the
user builds via the fuzzy-find picker: add reverb, add filter, reorder, remove. This is
the vision as stated, but it breaks the registry's core invariant — there is no compile-time
string ID for "the 3rd module in Bass's chain, param 2" because that slot may not exist in
another user's song. Making this work requires *inventing* a stable-ID scheme for dynamic
instances (e.g. `bass.chain[2].cutoff` synthesized from a module-type tag + position, or a
persisted UUID per chain entry) and teaching `song.rs` a second, non-generic encoding path
for chain state (module type, order, per-module param blob) alongside the existing flat
`ControlSpec` codec. This is real, durable-format work: chain edits must round-trip through
song codes forever, including old codes that predate a given module type.

**C. Hybrid — bounded slot count per voice, module identity dynamic.** Each voice gets N
fixed slots (say 3), each slot is `Option<ModuleId>` where `ModuleId` is a `Copy` enum
(`Reverb`, `Filter`, `Swing`, ...). Slot *position* is a static `ControlSpec`-addressable
discrete control (`bass.slot1.module` — like `pad.type`'s enum-index pattern already used
for `PadTone`/`BassVoice::voice_type`), and each module's own params are namespaced by slot
(`bass.slot1.reverb.mix`, `bass.slot1.filter.cutoff`, ...) as static rows that only apply
when that slot's module matches — inactive rows are simply inert, the same idiom
`arp.gain` defaulting to 0 already uses to make a whole voice a no-op. This keeps the
entire registry/song-code static-ID model intact (bounded, enumerable at compile time) while
giving the fuzzy-find UX real "pick a module for this slot" behavior and reordering (swap
which slot holds which module).

**Recommendation: C**, with N chosen per-voice by expected use (2 for Bass/Tonal, 3-4 for
Pad — reverb + filter + swing + one future slot) rather than one global constant. Rationale:

- It satisfies the "100-year" bar the registry was built to — the registry's whole
  reason to exist is *no per-function match arms, no runtime ID minting*. Option B
  reopens that solved problem for the sake of literally unbounded chains, which the
  UX vision doesn't actually require (a fuzzy-find picker filling 3 fixed slots feels
  identical to a user as fully dynamic stacking, until they want a 4th of the same
  type — a rare case that can be a deliberate rejection, at least for v1).
- It reuses an idiom the codebase already trusts: enum-index controls
  (`pad.type`, `bass.type`) selecting behavior, with per-branch params living as
  ordinary always-present rows that are inert when their branch isn't selected. No new
  song-code version, no new persistence path — `song.rs` needs zero changes.
- Option A is strictly worse than C for the same static-ID cost: A hardcodes which
  effect goes in which voice at compile time, C makes that a runtime user choice within
  the same static-row budget. There's no reason to pick A once C is on the table.
- Option B should be revisited only if real usage hits the slot ceiling constantly — that's
  a cheap future migration (raise N, add more static rows) versus adopting B's dynamic-ID
  song-code format now for a UX benefit nobody has asked to exceed yet.

## The `Effect` trait

```rust
/// One DSP module in a voice's effect chain. Mono or stereo input/output;
/// stateful (owns its own buffers/coefficients), sample-at-a-time to match
/// every existing voice's per-sample `next()` idiom.
pub(crate) trait Effect {
    /// Process one stereo sample. Effects that are naturally mono-in/stereo-out
    /// (reverb) or stereo-in/mono-out still take/return (f32, f32); callers sum
    /// to mono where needed, matching Freeverb::process's existing convention.
    fn process(&mut self, left: f32, right: f32) -> (f32, f32);

    /// Re-seed any internal RNG state. No-op default for deterministic effects
    /// (filters). Required for render --seed byte-reproducibility per any
    /// effect that uses randomness (a future chorus/random modulation module).
    fn reseed(&mut self, _seed: u64) {}
}
```

Deliberately **not** included: no `params()` / dynamic parameter introspection on the
trait. Params stay owned by the static `ControlSpec` rows per Option C above — the trait
is pure DSP, the registry is pure control surface, and a slot's active module type is what
routes control values into the right `Effect` impl's setters (via the same enum-dispatch
pattern `BassVoice::new` already uses for `voice_type`). Keeping the trait DSP-only avoids
forcing every future module to describe its own UI shape in Rust trait methods when the
registry already owns that job better (ranges, steps, display formatting, LFO snap).

Existing effects become trait impls with no behavior change:

```rust
impl Effect for Freeverb { fn process(&mut self, l: f32, r: f32) -> (f32, f32) { self.process(l, r) } }
impl Effect for BassLowPass { /* needs cutoff_hz + sample_rate as process args today;
    wrap in a small adapter struct holding those, or extend the trait with a
    second method for filters that need side-channel params — see Migration below */ }
```

Filters like `BassLowPass`/`TonalLowCut` take `cutoff_hz`/`sample_rate` as `process()`
arguments rather than storing them, because `bass.cutoff` can carry a live LFO/envelope
route recomputed every sample. The plain two-arg `Effect::process(l, r)` above doesn't fit
that. Rather than bloat the trait signature for every module, give filter-family modules a
thin wrapper that captures the per-sample modulated value from the owning engine before
calling `process`:

```rust
pub(crate) struct ModulatedLowPass<'a> { filter: &'a mut BassLowPass, cutoff_hz: f32, sample_rate: f32 }
impl Effect for ModulatedLowPass<'_> {
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        (self.filter.process(l, self.cutoff_hz, self.sample_rate),
         self.filter.process(r, self.cutoff_hz, self.sample_rate))
    }
}
```

This keeps `BassLowPass` itself unchanged (still byte-identical, still the true-bypass
check stays in the caller) while making it chain-compatible. The chain owner constructs
this wrapper fresh each sample from the slot's live modulated control value — cheap, no
allocation (it's a stack-local borrow), matches the existing per-sample recompute pattern
`bass.rs:212` already uses.

## Shared bus vs. private instance: migrate both, converge on shared-bus-with-per-slot-mix

Recommendation: **migrate the ambient bus first (validates the trait), then fold Clap's
private reverb into the same bus, deleting `ClapEngine`'s own `Freeverb` and `room` field
entirely** — do not keep both patterns as permanently coexisting idioms.

Reasoning: the two patterns exist today by accident of implementation order, not by
intentional design (Clap's reverb predates the ambient bus's mix-control refactor in
`fd9aa25`/`1e74b3e`). A private reverb per voice costs one Freeverb's worth of CPU
(8 combs + 4 allpasses, stereo) per voice that wants a "send some to reverb" effect;
letting every voice that wants reverb share the ambient tank is strictly cheaper and is
already the pattern 3 of 4 voices use. Clap's `dry_scale = 1.0 - room*0.5` mix law is also
inconsistent with the ambient bus's `dry_gain` law (`1.0 - mix*AMBIENT_REVERB_DRY_DUCK`,
duck=0.3) — unifying removes a second, subtly different reverb-mix formula for the user to
learn.

The one real cost: Clap's reverb is tuned differently (`room_size=0.28, damp=0.62,
wet=0.85` vs. ambient's `0.9/0.44/1.0`) — a distinct room character (short/bright vs.
long/dark). Folding Clap into the shared bus means either (a) accepting one shared
reverb character for all sends (simplest, matches "ambient" framing — Clap becomes another
sender into the same space), or (b) giving the shared bus two tanks (an "ambient" tank and
a "room" tank) if Clap's distinct character is judged worth preserving. Given Clap is a
short transient (claps decay in tens of ms) sent into a shared long ambient tank, the
tonal color difference will be audible and might be a real aesthetic loss — **flag this as
an open call for the repo owner, not something this doc can settle from code alone.**
Default recommendation if no strong preference: (a), single shared tank, re-tune Clap's
send level (a new `AMBIENT_REVERB_CLAP_SEND` constant) to taste rather than carrying two
tanks — simpler, one fewer place for reverb character to drift, and the existing
`AmbientReverbSend::process` signature already takes N voice mixes so adding Clap as a 4th
sender is mechanical.

## MIDI-domain modules: separate trait family, not a shared `Effect`

Arp's note generation, a note randomizer, or a future arpeggiator-as-module all operate on
**note events + timing** (which pitch fires, when) — they run *before* a voice synthesizes
audio, not on the audio stream after. `Effect::process(f32, f32) -> (f32, f32)` has no
sensible meaning for "which MIDI note should fire next" — there's no audio signal to
transform. Forcing both into one trait means either (a) a trait with two incompatible
`process` signatures behind feature-flag-style dead methods, or (b) audio-domain effects
wrapping note data in a fake "audio" representation, both worse than just having two
traits.

Recommendation: a second, distinct trait —

```rust
/// One MIDI-domain module: transforms/generates note events on a per-step or
/// per-trigger basis, ahead of audio synthesis. Distinct from Effect because
/// there is no audio signal at this stage — only note number, velocity/gain,
/// and grid timing.
pub(crate) trait NoteModule {
    /// Given the chord/scale context and the current step, return the note(s)
    /// to trigger this step (0 or more — a randomizer might skip a step, an
    /// arpeggiator might not).
    fn notes_for_step(&mut self, ctx: &NoteContext, step: usize) -> Vec<i32>;
}
```

Both traits can still live in the same chain-slot picker at the UX layer (the fuzzy-find
list just tags each entry audio-domain vs. MIDI-domain and only shows MIDI modules in a
voice's *pre-synthesis* slot group and audio modules in its *post-synthesis* slot group) —
they don't need to be the same Rust trait to present as one unified "stack modules on this
instrument" UI. This mirrors how a real DAW instrument channel separates "MIDI FX" from
"audio FX" as two distinct rack sections even though both feel like "add an effect" to the
user.

## Swing as first non-reverb candidate

Swing (delaying every other grid step by a ratio) is **also MIDI-domain**, not audio-domain
— it modifies trigger timing, not the audio signal. It validates `NoteModule`/timing-stage
modules, not `Effect`. Concretely: a swing module would need to intercept the
`GridTrigger::pop` call (`engine.rs`) and offset alternate hits' effective beat position
before the trigger fires, which is a different integration point entirely from where
`Effect` sits (post-voice-synthesis, on the audio stream). This doc should be explicit that
swing is *not* evidence for the `Effect` trait's generality — it's evidence for the
`NoteModule` trait, and the trait split above already accounts for it. If the repo owner
wants a single migration to validate "the stackable UX" broadly, swing plus one audio
effect (see below) together validate both halves; swing alone does not touch `Effect`.

## Migration plan: ambient reverb (pad/tonal/arp) as first `Effect` validation

Chosen per the task's suggestion, now that `arp.reverb_mix` has landed (`1e74b3e`) giving
all three voices a symmetric per-voice mix control.

1. Wrap `Freeverb` in the `Effect` impl shown above (trivial — it's already
   `process(f32, f32) -> (f32, f32)`; no signature change needed since it doesn't need
   per-sample side params like the filters do).
2. Give `FluidEngine` one `Box<dyn Effect>` slot (or just the concrete `Freeverb`, since
   there is exactly one ambient tank and Option C above doesn't require dynamic dispatch
   for a slot that only ever holds one module type) — no behavior change, this step only
   proves the trait compiles and `process` behavior is unchanged.
3. Verify with `nooise render --seed N` before/after: byte-identical output required,
   since this step is pure refactor with zero control/behavior change.
4. Once (1)-(3) land and are verified, fold Clap's private reverb in as a 4th sender per
   the Shared-vs-Private section above — this is the step that actually changes audio
   (Clap's send now goes through the shared tank), so it needs the repo owner's call on
   tank-sharing vs. dual-tank first.
5. Only after both reverbs are unified behind `Effect`, introduce the second module type
   (bass filter, wrapped per the `ModulatedLowPass` adapter) into a real Option-C slot
   system, proving the trait hosts more than one DSP shape before any UI/fuzzy-find work
   starts.

This order (prove trait on the module with zero behavior risk → reconcile the two reverb
patterns → add a second, different-shaped module) is deliberately conservative: each step
is independently verifiable by `cargo test` + `nooise render --seed` diffing, and no step
requires the dynamic-slot UI to exist yet.

## Hard constraints, addressed

- **Registry stable IDs / song-code encodability**: Option C requires zero changes to
  `song.rs` or the `ControlSpec`/ID model — slot-module-selection and per-module params are
  ordinary static rows (`bass.slot1.module`, `bass.slot1.filter.cutoff`, ...), inert when
  not selected, exactly like `arp.gain=0` today makes the whole Arp voice inert. Old song
  codes missing a newer slot ID decode to default (module: none) automatically, same as
  every other registry addition.
- **Seeded determinism**: `Effect::reseed(seed)` is on the trait for any future module with
  RNG state (a chorus with randomized modulation, a random-drift filter). All 4 effects
  migrated in the plan above are deterministic (no RNG), so `reseed` is a no-op default
  until a module needs it — matches the existing `FluidEngine::reseed` contract that all
  voice RNGs must be reachable from one reseed call.
- **`GainSmoothers` / `ControlKind::Gain`**: unaffected — mix/gain-typed params inside a
  module (e.g. `bass.slot1.reverb.mix`) get `ControlKind::Gain` exactly like
  `pad.reverb_mix` today, and `GainSmoothers::new` already derives smoothers from
  `all_specs()` filtered by `kind.smooths_audio()`, so nothing chain-specific is needed —
  a static row is a static row regardless of which slot it's attached to.
- **`modulated_control_value_full` / LFO routing (`f`)**: also unaffected by construction —
  every module param is a `ControlSpec` row with a stable string ID, so it's addressable by
  the existing automation system exactly like `bass.cutoff` is today (which already proves
  a filter-family effect param can carry a live LFO route through this exact path). No
  chain-specific automation plumbing is needed; this is the main payoff of choosing Option
  C over B.

## Summary of calls

| Question | Call |
|---|---|
| Trait shape | `Effect` (audio, `process(f32,f32)->(f32,f32)`, `reseed`), separate `NoteModule` (MIDI-domain) |
| Static vs. dynamic chain | Bounded per-voice slots (Option C), module identity dynamic via enum-index control, not fully dynamic `Vec<Box<dyn Effect>>` |
| Shared vs. private reverb | Migrate both onto one shared bus; Clap's distinct room character is an open aesthetic call, default to single shared tank |
| MIDI-domain modules | Separate `NoteModule` trait, unified only at the fuzzy-find UI layer |
| Swing | Validates `NoteModule`, not `Effect` — timing-stage, not audio-stage |
| First migration | Ambient reverb (pad/tonal/arp) — zero-behavior-change trait wrap first, then Clap fold-in, then bass filter as 2nd module shape |
