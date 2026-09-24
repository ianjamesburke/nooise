# Domain context

## Interaction glossary

- **Raw input:** an operating-system or terminal event as received by the
  adapter. It can contain terminal-specific key codes, modifiers, resize
  events, and incomplete phase information. Raw input never mutates
  application state.
- **Input phase:** the lifecycle position of a physical key event: `Press`,
  `Repeat`, or `Release`. Phase is explicit even when the terminal cannot
  report every phase; a capability records that limitation. The kernel does
  not guess phases from timing.
- **Intent:** a terminal-independent statement of what the user asked nooise
  to do, such as move selection, begin numeric entry, adjust a control, or
  cancel the current interaction. The interaction kernel consumes intents.
- **Interaction mode:** the one state that currently owns keyboard input and
  defines which intents are legal. Modes are mutually exclusive variants, not
  independent flags or optional fields.
- **Effect:** an ordered request from the pure interaction kernel to the
  outside world, such as atomically publishing session edits, copying a song
  code, or quitting. Effects cannot mutate the interaction model directly.
- **Effect module:** an addable, slot-addressed musical processor on a voice
  layer or Master. Its stored catalog kind selects shared pre-trigger or
  post-synthesis behavior; this is distinct from an interaction **Effect**.
- **Automation lane:** one LFO or triggered envelope applied to a slider. A
  lane owns its curve, timing, amount, and editor state.
- **Automation stack:** every automation lane on one slider. The engine sums
  the stack in dial-position space, clamps and snaps once, then de-clicks the
  combined audio-rate movement.
- **Live-session snapshot:** one immutable aggregate containing controls,
  automation, and user-audible runtime session state that must change
  coherently. Writers publish the aggregate with one atomic `ArcSwap`
  replacement. Audio readers load that same aggregate without locks.
- **Transport:** whether the beat clock runs (`Transport::Playing` or
  `Stopped`). Stopped holds the beat and fires no grid hit while audio keeps
  running, so tails ring out. Live-session state, never in a song code.
- **Frame:** one immutable view-model snapshot passed to the renderer and one
  completed terminal draw from that snapshot. A state change is not visibly
  complete until a corresponding frame has been drawn.
- **Capability:** an explicit fact negotiated with the terminal adapter,
  including whether distinct repeat and release phases are available.
  Capabilities select safe interaction semantics; they are never inferred by
  a timeout inside the interaction kernel.

## Music glossary

- **Progression:** eight chords the Pad, Bass, Arp, and Lead all follow. A
  built-in one is named by key and mood ("Am · Drift"); Custom is built from
  eight user-authored chord slots.
- **Chord window:** the part of a progression that loops: Chord Count chords
  starting at Chord Offset, wrapping past the eighth. Count 4, Offset 4 plays
  chords 5–8.
- **Phrase:** one run of the chord window at one Chord Length. Changing
  Chord Length, Count, Progression, or Offset waits for the chord sounding
  now to end, then starts a new phrase from the new window's first chord.
- **Song value:** what a song code stores for a table-indexed control. It is
  permanent and separate from the control's dial position, so a table can
  grow without changing what a saved code means.

See [ADR 0001](docs/adr/0001-unidirectional-interaction-architecture.md) for
the contracts that connect these concepts.
