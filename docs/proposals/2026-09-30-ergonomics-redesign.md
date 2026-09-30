# Ergonomics redesign: operations, leader, and Motion

Status: Draft. This proposal defines the next experiments. It changes no shipped
input, automation, persistence, or song-code contract on its own.

## Decision

`/` is the complete searchable world for musical controls and operations.
Space is a visible, curated fast path into a small subset of that world.

Every operation has one typed definition. The palette can expose every valid
operation. A leader binding is optional. A binding appears only after it has a
short, playable sentence and a clear context.

This preserves the arrow and Tab floor. It also avoids two bad outcomes:
putting a hidden shortcut on every new feature, or turning Space into a second
text palette.

## The operation registry

Controls stay in the existing control registry. They own ranges, display,
persistence, and pages. The new operation registry owns named things a player
does: recipes, mix actions, lane actions, Motion actions, and future planned
layer actions.

An operation definition has:

- a closed, non-serialized operation id;
- label, aliases, and short palette description;
- target kind: none, selected control, current layer, or named layer;
- a typed command payload that resolves to existing typed interaction effects;
- availability and target-validation rules;
- an optional leader projection: route, display label, and context.

The registry projects into two surfaces.

- The palette lists every operation alongside controls and modules. Selection
  still freezes the relevant target and beat where the current action needs
  them. Execution continues to use typed effects. The executor never receives
  a string command or an operation id to interpret.
- The leader lists only bindings valid at the current step. It uses the same
  labels and command data as the palette, then emits the same typed intent or
  effect payload.

The palette remains a static deterministic list. Live topology or availability
is checked when an operation is confirmed, as it is today. That protects the
kernel and renderer from indexing different lists.

## Space leader

Space opens an immediate command map. It does not wait for a timeout.

The root map shows the layer targets and the valid current-page endings. A
layer selector completes immediately: it opens that page at the layer's
remembered row and gives the keyboard back to Browsing. Escape cancels an
unfinished sentence. A repeated Space is inert. Press and release capabilities
do not change the grammar.

The first code slice keeps every existing Jump result intact:

```
Space: a Pads  s Perc  d Bass  f Kick  q Tonal  w Clap  e Arp  r Master
Current page: j level  k filter
Esc cancel
```

`Space a` is enough to arrive at Pads, on the knob last active there. Normal
`h`/`j`/`k`/`l` then work immediately. `Space j` and `Space k` remain direct
current-page shortcuts when the player wants Level or Filter rather than the
remembered row.

The map replaces the control area in the same way as Help. The breadcrumb,
activity row, and stable footer remain visible. At the minimum 46x11 frame,
the root map uses two target rows and a compact current-page row.

Leader hints default to on. Hints may later be set to off as a local interface
preference. The leader still works when hints are off. Hint visibility never
enters the interaction model, live session, or a song code. Adding durable
local configuration is separate work; until it exists, the default map is the
honest product behavior.

Planned mute is the first candidate for a new leader operation:

```
m           mute the visible layer now
Space m     arm current layer mute at next bar
```

Jump to another layer first, then use `Space m`, rather than making a layer
selector wait for a possible third key. A later targeted-action grammar needs
its own explicit sentence if it earns one.

A small `m` means mute armed. `(M)` means muted. The planned action is
cancelable and persists because it changes the audible future of the session.
It is a later slice, not part of the leader-menu parity slice.

## Time has three meanings

The redesign must not hide different musical contracts behind one generic
scheduler.

| Feature | Contract |
| --- | --- |
| Planned mute and state recall | One action commits at a musical boundary. |
| Motion Grab | Make a loop from a finished interval of knob history. |
| Motion Record | Arm, record over a defined interval, close, then loop. |
| LFO, envelope, and Motion | Persistent automation lanes. |

These can share a small `MusicalBoundary` vocabulary, pending feedback, and
Escape cancellation. Their effects, state, and persistence remain distinct.

## Motion replaces Capture

Capture is retired as a public concept. Motion is a peer lane type alongside
LFO and envelope.

A first Motion lane has one target, one duration, sampled position values,
phase, enabled state, and a launch point. It keeps the current compact
resolution of eight samples per beat:

| Duration | Samples |
| --- | --- |
| 4 beats | 32 |
| 8 beats | 64 |
| 16 beats | 128 |

The first palette choices are:

- `Motion Grab 4`, `Motion Grab 8`, and `Motion Grab 16`: copy the
  finished interval ending now and launch its loop at the next bar.
- `Motion Record 4`, `Motion Record 8`, and `Motion Record 16`: arm at
  the next bar, record that exact interval, close, and start the loop.
- `Bypass`, `Resume`, and `Delete`: operate on the selected lane,
  including Motion.

Record has visible lifecycle: `ARMED NEXT BAR`, `REC 8`, then `LOOP`.
Escape before or during recording cancels it. A completed record atomically
replaces the old Motion on that knob.

Turning a knob while a Motion loop plays bypasses the loop and gives the
player the current value. It does not guess a new recording range or silently
replace the loop. A player chooses Record to replace it. During an armed or
recording Motion, those edits become the recorded movement.

The first version allows one Motion per target and no more than four live
Motion loops. It excludes overdub, relative/additive movement, multiple Motion
lanes on one knob, and a leader binding. Motion begins palette-only because its
duration choice and lifecycle need to be played before they deserve a compact
sentence.

## State and persistence

Completed and pending Motion belong in the aggregate live session with the
other automation state. A save captures a pending record's target, duration,
phase, held value, and events relative to its transport anchor. Loading resumes
the audible lifecycle instead of dropping it.

The current fixed Capture record cannot be silently reinterpreted as Motion.
A Motion format cut validates target, duration, sample count, phase, duplicate
targets, control eligibility, and range epochs. Old Capture payload semantics
are refused. Built-in states are re-authored through the current encoder if
they carry affected data.

The coordinator's existing production tick is the one place that advances and
closes an armed Motion. The executor does not depend on a later keypress to
finish recording.

## Contextual palette ranking

When a query is the canonical name of a module available on the current page,
that page's module outranks the same module elsewhere before fuzzy score is
considered. On Master, typing `swing` therefore starts at `Global Swing ·
Master · module`, not a Pads Swing entry whose shorter label happens to score
better. The same rule makes a voice page reach that voice's Swing first.

This is a narrow scope preference, not a general instruction to put every
current-page fuzzy match above a stronger result. Layer-name primary controls
and MRU behavior retain their existing deliberate ordering.

## Implementation order

1. Done: static operation metadata projects existing Capture, mix, and recipe
   palette actions through one closed vocabulary. Their behavior and typed
   effects are unchanged.
2. Done: Jump renders an immediate map from the existing layer and parameter
   tables. Its routes and typed effects are unchanged.
3. Play that leader before adding a new action. Add planned mute only if the
   map makes its target and cancellation obvious.
4. Replace Capture with palette-only Motion. Generalize lane lifecycle and
   song persistence together. Do not add overdub.
5. Only after a second musical action needs it, consider a shared internal
   boundary-action type. State recall is a likely test once marks exist.

## Verification

For the leader slice:

1. Run every existing Space Jump sequence, including current-page shorthand,
   full-capability input, press-only input, Repeat, and Escape.
2. Check the root and target maps at 46x11 and ordinary terminal sizes. The
   map must name the current target and leave activity/footer information
   visible.
3. Confirm arrows and Tab work unchanged before Space and after completed or
   cancelled sequences.
4. Run production replay, UI snapshots, format, Clippy, and the full test
   suite.

For Motion:

1. Grab a moving knob over 4, 8, and 16 beats and confirm each loop starts on
   the next bar with the right duration and phase.
2. Arm, record, cancel, and close each duration without another keypress.
3. Touch an active Motion and confirm bypass; touch during Record and confirm
   recorded movement.
4. Save and load an armed record, a mid-record state, a loop, and a bypassed
   loop. Verify compact payload bounds and refused invalid or retired data.
5. Render each lifecycle deterministically and check lane composition and
   de-clicking.

## Open decisions

- Whether local leader hints eventually offer only on/off or on, first-use,
  and off.
- Whether a planned layer action lands on the next bar or, for selected
  actions, a chord boundary. The first planned mute tests next bar only.
- Whether a later State Mark recall joins the leader as `Space 1` through
  `Space 9`. Do not reserve its keys before state marks are playable.
