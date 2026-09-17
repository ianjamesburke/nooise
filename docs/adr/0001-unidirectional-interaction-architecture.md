# ADR 0001: Unidirectional interaction architecture

- Status: Accepted; implemented, amended 2026-09-16 for normal-browsing gestures
- Date: 2026-07-29
- Scope: terminal input, UI state transitions, live-session effects, and
  rendering

## Context

Before this architecture, the `src/fluid/ui.rs` loop owned terminal I/O,
frame pacing, interaction state, live-session mutation, and rendering. It
represented mutually exclusive interactions with independent variables and
drained every queued event before drawing again.

The Performance Deck and Performance Sequence prototypes each had a locally
correct state machine. They still appeared to freeze because duplicate
activation events could enter and exit a mode in the same unbounded input
batch, before any frame showed the intermediate state. A permanently busy
event source can also postpone rendering without limit. These are composition
failures at the event-source -> batching -> transition -> effect -> render
boundary.

The advanced “vim motions for music” layer must remain strictly additive to
the arrow-key + Tab floor in `docs/NORTH_STAR.md`. That requires keyboard
ownership, cancellation, terminal fallback, and visible feedback to be
architecture contracts instead of conventions in match-arm ordering.

## Decision

nooise uses a small, application-specific model/update/view architecture:

```text
terminal event
    -> raw input adapter + capability-aware normalization
    -> semantic intent
    -> pure update(model, intent) = (model, ordered effects)
    -> ordered effect executor
    -> immutable view model
    -> Ratatui render
```

The runtime schedules these phases and enforces input budgets and render
deadlines. The interaction kernel contains no Crossterm or Ratatui types, does
not read clocks or shared state, and performs no I/O. It is deliberately
nooise-specific: its types name pages, control selection, numeric entry,
automation editing, palette use, and future performance grammar directly.

### Rejected alternatives

- **Direct raw-key mutation** is rejected because terminal details, ownership,
  state transition, and side effects become sensitive to branch order. Rust
  can type-check each branch but cannot prove invariants hidden across
  independent variables and an unbounded loop.
- **A generic Flux dispatcher or framework** is rejected because nooise needs
  a small deterministic transition function, not runtime registration,
  stringly typed actions, middleware, or another abstraction between domain
  intent and domain state. Generic dispatch would obscure the invariants the
  types are meant to expose.
- **Timing-based debounce** is rejected because elapsed time cannot distinguish
  deliberate taps from terminal repeat, varies across terminals, adds latency,
  and only hides scheduler starvation. Input phase and render fairness must be
  represented explicitly.
- **Preserving the current `ui_loop` shape during migration** is not a
  constraint. Temporary adapters may support staged delivery, but the legacy
  raw-key mutation path is deleted at migration completion.

## Required invariants

### Keyboard ownership and legal states

1. Exactly one `InteractionMode` variant owns keyboard input at a time.
   Browsing, numeric entry, palette, automation editing, Sequence, and Lead
   play cannot be simultaneously active. Temporary effect gestures belong to
   Browsing and do not take over navigation. Their audio returns can continue
   after another mode takes keyboard ownership.
2. Mode-specific data is stored inside its owning enum variant. Illegal
   combinations cannot be constructed; there are no parallel
   `Option<NumericEntry>`, `Option<PaletteState>`, and performance-active flags.
3. Navigation context that does not own the keyboard, such as page, selected
   row, and a page-local drill path, lives in one cohesive model and is
   preserved or reset only by an explicit transition.
4. Input normalization routes a key to the current owner exactly once. An
   intent handled by a mode cannot fall through to global bindings.

### Press, repeat, release, and cancellation

5. `Press` may activate a command once. `Repeat` produces intents only for
   commands explicitly marked repeatable, such as navigation or continuous
   adjustment. Mode leaders, toggles, confirmation, cancellation, save, quit,
   and other edge-triggered commands ignore `Repeat`.
6. `Release` ends an explicit held interaction and otherwise has no semantic
   effect. A hold gesture is offered only when release is a negotiated
   capability; the kernel never synthesizes release from elapsed time.
7. Activating an already active leader is idempotent unless the interaction
   grammar explicitly assigns it a non-cancellation meaning. Activation keys
   do not double as implicit cancel keys. This prevents repeated activation
   bytes from creating an invisible enter-then-exit transition.
8. `Escape` means “return one interaction depth toward ordinary browsing.”
   It cancels uncommitted text or sequences, closes the current editor or
   drill, and eventually becomes a no-op in base browsing. It never commits an
   edit, quits the application, or depends on timing. In Browsing, held or
   restored gestures are released first without changing navigation; with
   only returns/tails remaining, ordinary drill/back behavior applies.
9. The arrow-key + Tab floor remains available in ordinary browsing on every
   terminal. Advanced modes may own those keys only after explicit entry and
   must provide the Escape path above.

### Determinism and effects

10. For the same initial model, capability set, ordered intents, and explicit
    context values, `update` produces structurally equal models and ordered
    effects. It cannot read `Instant`, telemetry, environment, clipboard,
    terminal state, or shared atomics.
11. Effects execute in emission order. Controls, automation, and user-audible
    runtime session state that must change coherently live in one immutable
    `LiveSessionSnapshot`. Each live-session transaction reads one aggregate,
    applies the complete domain edit, and publishes one replacement `Arc`
    through one `ArcSwap`. All writers use this transaction boundary and must
    prevent lost updates through a single writer or compare-and-swap retry.
    Sequential stores to independent controls, automation, or runtime-state
    swaps do not satisfy atomic publication.
12. Effect results re-enter the kernel as new semantic intents on a later
    bounded scheduler turn. The executor never reaches into the model, and a
    failed effect cannot partially advance kernel state as if it succeeded.
13. Rendering reads one immutable view model derived after the turn's accepted
    transitions and synchronous session effects. Render code has no session
    mutation, input polling, or interaction transitions.
14. The audio callback loads the aggregate session `Arc` without locking and
    observes one coherent generation. Aggregation may change the shape of the
    existing `ArcSwap` boundaries, but it cannot add a reader-side mutex,
    blocking channel, or allocation to the audio path.

### Render fairness

15. Input admission is bounded by both event count and elapsed scheduler
    budget. A pending event source cannot keep the runtime in input processing
    indefinitely.
16. A state-changing intent requests a frame. The runtime completes a draw
    before admitting another input batch when its render deadline is due.
17. The runtime targets the existing 33 ms cadence. While input remains
    continuously available, no more than **50 ms** of application-controlled
    time may elapse between completed visible frames, and the first frame
    reflecting a state-changing intent must complete within the same 50 ms
    ceiling. Blocking inside the terminal backend is measured and surfaced
    but cannot be bounded by application scheduling; deterministic runtime
    tests use a nonblocking backend and fake clock.
18. Repeat traffic may be coalesced only when doing so preserves the defined
    semantic result. Coalescing never crosses a mode transition, an
    effect-ordering boundary, or a requested frame.

### Terminal capabilities

19. Terminal setup and restoration are owned by one adapter and are
    symmetrical on normal return and error unwind. The adapter records which
    keyboard enhancements were actually enabled.
20. Normalization receives an explicit capability value. When distinct repeat
    or release phases are unavailable, ordinary press commands continue to
    work, ambiguous repeat is never interpreted as a hold, and hold-only
    performance gestures are unavailable or mapped to documented one-shot
    alternatives.
21. Capability fallback cannot alter persisted music state, silently change a
    command's direction, or degrade arrow-key + Tab navigation. Browsing
    gestures are inactive without trustworthy releases and the footer names
    that requirement.
22. Gesture Press pins a target; Repeat cannot retrigger or retarget it.
    Matching key Release works after modifier changes. Modal entry releases
    gestures and quarantines still-down keys until their physical release.
    Reported focus loss and shutdown also release held gestures.
23. Gesture envelopes live in the aggregate session and advance against an
    audio-owned monotonic seconds clock. Normal edits and auto-morph continue
    independently. Compact song records preserve active envelope amounts and
    direction; processor buffers remain excluded audio state.

## Target module responsibilities

The production modules under `src/fluid/` follow these boundaries:

- `interaction.rs` owns the pure model, typed intents, phase policies, mode
  transitions, and ordered effect requests.
- `runtime.rs` owns terminal capability negotiation, normalized input,
  lifecycle restoration, and bounded scheduling.
- `coordinator.rs` is the production ordering seam shared with replay.
- `effect.rs` executes effects and acknowledges results; `edit.rs` owns
  control and automation mutations.
- `session.rs` owns aggregate publication and the shared audio clock.
- `view.rs` projects immutable render data; `ui.rs` draws it.
- `gesture.rs` owns scalar performance envelopes; `engine.rs` and
  `engine/gesture_audio.rs` apply them through shared audio processors.
- `replay.rs` exercises the production input/coordinator/render path with
  recorded phases, fake time, and bounded scheduler checks.

Dependencies flow inward toward domain types and the pure kernel:

```text
input adapter ─┐
runtime ───────┼─> intent -> update <-> model -> effect requests
effect adapter ┘                         |
view derivation <------------------------┘
render <-------------------------------- view model
```

Neither `update` nor `model` depends on an adapter. `render` does not depend on
the runtime or effect executor.

## Original migration sequence

1. **Freeze the contract.** Land this ADR, glossary, invariants, and acceptance
   scenarios before changing production behavior.
2. **Extract the pure kernel.** Introduce cohesive model, intent, effect, and
   exhaustive update types behind unit tests. Translate existing behavior
   without adding performance grammar.
3. **Introduce the runtime adapter.** Negotiate key phases, normalize raw
   events, bound input work, and enforce the frame deadline with fake-clock
   scheduler tests.
4. **Centralize effects and view derivation.** Replace the independent
   controls, automation, and runtime-state publication boundaries with one
   atomically swapped aggregate live-session snapshot. Route every writer
   through its transaction API while keeping audio reads lock-free. Make
   rendering consume one immutable view model. The effect and view boundaries
   can be developed independently after the kernel contract exists.
5. **Build production-faithful replay.** Exercise the real normalizer, runtime,
   kernel, effect boundary, view derivation, and Ratatui test backend together.
6. **Migrate production and delete legacy paths.** Prove behavioral parity,
   switch the live terminal loop, and remove raw-key mutation, independent
   modal flags, the unbounded drain, and the many-argument render interface.
7. **Rebuild performance interaction.** Sequence retains one-shot edits.
   Normal-browsing gestures replace the persistent Deck workflow.

Audio DSP, audio routing, registry value semantics, automation math, and song
serialization are outside this refactor. For unchanged seeds and session
edits, existing headless render checksums must remain byte-for-byte identical.
If a later product change intentionally changes audio, it requires its own
explicitly reviewed checksum update and is not architecture-migration parity.

## Architecture-level acceptance scenarios

The replay test system must make these scenarios deterministic and retain the
intent, transition, effect, and frame trace on failure.

### Normal browsing gesture lifecycle

Given Browsing, a gesture Press starts its envelope on the current page's
layer. Repeat leaves it untouched; navigation remains available. A matching
Release, including one with changed modifiers, starts the return on the
original layer. Opening another mode releases it, and a held key cannot
reactivate it on return to Browsing until a physical release occurs.

Escape releases held or restored gestures before leaving a drill. Focus loss
and shutdown release them through the same ordered effect path. A frame
reflecting each accepted change completes within the 50 ms bound.

With press-only capabilities, gesture keys are inert and the help states why.
Rendered audio must prove a swell, a continuous partial release, and surviving
Bloom/Echo tails; input and footer checks alone do not establish this behavior.

### Duplicate Performance Sequence leader (double Space)

Given ordinary browsing and two Space events available before the next poll,
the Sequence leader enters `ChooseInstrument` once. Repeat is ignored and a
second distinct activation is idempotent; Space does not cancel the sequence.
A frame showing `ChooseInstrument` completes within 50 ms before any later
instrument intent is processed past a due render boundary. No control or
automation effect is emitted.

### Sustained repeat under a permanently ready source

Given a repeatable adjustment held while the scripted event source always has
another repeat available, only its initial `Press` and allowed `Repeat` phases
emit adjustment intents. The runtime continues to complete frames no more than
50 ms apart, preserves the ordered adjustment result, and processes a
subsequent `Release` and Escape without starvation. Repeating any
edge-triggered leader, save, quit, confirmation, or cancellation command emits
no duplicate semantic action.

### Capability fallback

Replay each scenario with full press/repeat/release support and with press-only
capabilities. Press-only fallback retains arrow-key + Tab navigation and all
one-shot editing. It never enables a hold-only gesture, fabricates a release,
or relies on a debounce timeout.

### Deterministic replay and effect failure

Replaying the same initial session, capability set, raw-input trace, and fake
clock produces an equal intent trace, model sequence, ordered effect trace,
and frame buffers. Injecting failure at any effect leaves later effects
unexecuted until the failure result is handled and never exposes a partially
published live-session transaction.

## Consequences

- The compiler can enforce exclusive modal ownership and exhaustive
  transitions once the kernel types replace independent loop variables.
- Scheduler liveness remains a tested temporal property instead of a type
  property, but it has one runtime owner, a numeric deadline, and a
  production-faithful deterministic test seam.
- Coupled live-session state has one observable generation. The refactor keeps
  lock-free `ArcSwap` audio reads while replacing the old independent
  publication shape.
- Some current functions will move or disappear, and temporary migration code
  is expected. Behavioral parity is defined by semantic traces, view frames,
  and unchanged audio checksums, not by preserving internal call shape.
- Advanced key grammar can evolve without exposing terminal phase quirks to
  domain logic or destabilizing base navigation.
