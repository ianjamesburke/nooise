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
- **Downbeat reset:** `d` or `/downbeat` makes the next beat line beat one of
  bar one. The beat phase never moves; bars rotate by whole beats. The
  sounding chord restarts its count instead of changing, and every grid
  restarts at step 1. It is live-only and never in a song code.
- **Frame:** one immutable view-model snapshot passed to the renderer and one
  completed terminal draw from that snapshot. A state change is not visibly
  complete until a corresponding frame has been drawn.
- **Hub:** the Master page, where the app opens. Its first rows are the
  layers, each showing its level and opening that layer on Enter; Master's
  own controls follow. Tab reaches it after Lead, and Esc from a layer's
  root returns to it on that layer's row.
- **State leader:** a pending `'` (go) or `"` (keep). Go: a digit `1`-`9` queues that stored state (a second `'` queues the previous one), a layer key opens that layer, and `j`/`k` reach Level/Filter on the open page. Keep: a digit stores the current song there, replacing any stored one. It opens only from browsing, acts on key presses, and leaves physical holds running.
- **Grab leader:** a pending `g` waiting for `1`/`2`/`4`/`8` bars of the selected knob's or editor field's movement to keep as Motion; `G` loops the harmony instead.
- **Loop:** a harmony loop: the chords heard over the last 1, 2, 4 or 8 bars (authored, pushed or looped), replayed with their real timing as a lane separate from the progression's slots. A new grab replaces it; a landed push ends it.
- **State (stored state):** one numbered slot, 1-9, in the state bank. It
  holds a whole song snapshot (controls and automation) that the performer
  stored from the live song and recalls with the State leader or `/queue state
  N`. Stored states live only in the `--set` file and are never in a song
  code. The word "state" alone means this in the UI, the palette, and the
  performance docs; say "stored state" where it could be misread.
- **Auto state:** one of the built-in song endpoints (`AUTO_STATES`) that
  `nooise auto` morphs between, also reachable as built-in song numbers. It
  is authored in the source, never stored in the bank, and unrelated to the
  1-9 slots. Both a stored state and an auto state are a `SongState` value in
  code, which only means "a complete snapshot". The bank is `StateBank` and
  its recall types are `State*`; the morph endpoints are `AUTO_STATES`.
  "Section" is retired for both.
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
- **Master Key:** The one key every progression plays in, or Home, where each
  keeps its authored key. Choosing a pitch keeps each progression's mode and
  moves its tonic there, so a morph or progression change never leaves the key.
  Letter names and the header follow it; Roman numerals do not.
- **Master Mode:** Home (each progression's own mode) or Major, Minor,
  Dorian, Phrygian or Mixolydian. It sets the **home** pushes move in: the
  progression's tonic in that mode, which scale steps walk, step chords take
  their quality from, and `a` returns to. It never rewrites a progression,
  a loop or the sounding chord; to hear material in a new mode, push it
  there and loop it. Saved in song codes.
- **Chord names:** Roman numerals ("i7", the default) or letters ("Am7"), a
  display choice made from `/` that never changes sound or a song code. Roman
  reads each chord against the home: the progression's tonic in the Master
  Mode, or the progression's own mode under Home.
- **Key label:** the header writes the home as a chord symbol for major and
  minor (`A`, `Am`) and by name for the other modes (`A Dorian`).
- **Progression:** eight chords the Pad, Bass, Arp, and Lead all follow. A
  built-in one is named by key and mood ("Am · Drift"); Custom is built from
  eight user-authored chord slots.
- **Chord window:** the part of a progression that loops: Chord Count chords
  starting at Chord Offset, wrapping past the eighth. Count 4, Offset 4 plays
  chords 5–8.
- **Phrase:** one run of the chord window at one Chord Length. A queued state lands where the phrase ends, when the window wraps to its first chord. Manual
  window/length edits wait for the sounding chord to end, then preserve its
  successor where possible; selecting another progression starts at its first
  chord. An auto morph keeps the outgoing harmony through whole phrases
  with Kick and Bass silent, then starts the incoming window at its first
  chord on landing.
- **Push the Harmony:** steering the harmony by moves from the chord
  sounding now (P, L, R, scale steps of 1-3, fifths, home) while Space is
  held, never by naming a chord. A **move** lands on the next beat line by default
  (**chord quantize** can make it the next 1, 2 or 4 bar line, or at once),
  composing with one already queued. The **pushed chord** is the triad a landed move sounds; it holds
  the progression on its slot from then on (there is no way back but a
  state or song change).
- **Morph leg:** an outgoing song's hold followed by its crossing into the
  next song. `--bars` requests an approximate total length. Hold and crossing
  each round to whole outgoing phrases, at least one phrase each.
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
