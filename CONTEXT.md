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
- **Hub:** the Master page, where the app opens. Its first rows are the
  layers, each showing its level and opening that layer on Enter; Master's
  own controls follow. Tab reaches it after Lead, and Esc from a layer's
  root returns to it on that layer's row.
- **Layer:** one voice's page (Pads, Perc, Bass, Kick, Tonal, Clap, Arp,
  Lead), entered from the hub. It remembers its row for the session.
- **Breadcrumb:** the line naming the path from the hub to the open page,
  such as `Master › Pads › Progression`.
- **Capability:** an explicit fact negotiated with the terminal adapter,
  including whether distinct repeat and release phases are available.
  Capabilities select safe interaction semantics; they are never inferred by
  a timeout inside the interaction kernel.

## Music glossary

- **Home key:** The progression's tonic and mode. Lead's Scale reads its seven
  notes even when a sounding chord borrows a note outside them. Built-in
  progressions declare a mode; Custom currently has an A-minor home.
- **Progression:** eight chords the Pad, Bass, Arp, and Lead all follow. A
  built-in one is named by key and mood ("Am · Drift"); Custom is built from
  eight user-authored chord slots.
- **Chord window:** the part of a progression that loops: Chord Count chords
  starting at Chord Offset, wrapping past the eighth. Count 4, Offset 4 plays
  chords 5–8.
- **Phrase:** one run of the chord window at one Chord Length. Manual
  window/length edits wait for the sounding chord to end, then preserve its
  successor where possible; selecting another progression starts at its first
  chord. An auto morph keeps the outgoing harmony through whole phrases
  with Kick and Bass silent, then starts the incoming window at its first
  chord on landing.
- **Morph leg:** an outgoing song's hold followed by its crossing into the
  next song. `--bars` requests an approximate total length. Hold and crossing
  each round to whole outgoing phrases, at least one phrase per section.
  Progression, chord length/window, and Swing change on the landing; Kick
  and Bass return at their destination levels.
- **Tempo bridge:** a transition tempo near the outgoing BPM that is half or
  double the incoming BPM. The crossing glides to this bridge, then switches
  to the incoming song's authored BPM on landing. Nearby tempos glide
  directly; large gaps without a close bridge jump on landing.
- **Song value:** what a song code stores for a table-indexed control. It is
  permanent and separate from the control's dial position, so a table can
  grow without changing what a saved code means.

See [ADR 0001](docs/adr/0001-unidirectional-interaction-architecture.md) for
the contracts that connect these concepts.
