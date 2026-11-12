# v4: Push the Harmony and the v4 key map

Status: playable on branch `temp-v4-harmony` (to be renamed `v4`), cut from
the v3 show build `temp-show-marks` and merged with it at aea412b. Design and
key map approved by Ian; exploration, not the show build. Listening and
hands-on play are the open gate.

## Idea

Ambient harmony lives in smooth single-note shifts. You never name a chord:
each key moves the harmony from where it is now with minimal voice movement,
using the neo-Riemannian P, L and R swaps plus diatonic steps and fifths. You
steer by feel, it is impossible to play wrong, and it works with eyes closed.

## Against the North Star posture

The owner's "video game engine of music generators" posture
(`docs/NORTH_STAR.md`, 19d527d) names Push the Harmony as its example, and the
v4 key map follows the owner's columnar keyboard (a ZSA Voyager: keys directly
above and below a home-row key are sizes or variants of the same move).

- **Moves, not set values or prepared material.** Every key is relative to
  the chord sounding now; nothing is named or chosen from a list. There is no
  key back to the authored progression: once pushed, the home row is the
  instrument, and the authored progression is only the starting point.
- **Cannot be played wrong.** Every move key always produces a chord: the
  diminished P/L/R neighbours exist so no key is a dead end, and every chord
  stays inside MIDI 40-79. Undo with nothing to undo says so and changes
  nothing.
- **Rewards practice.** On the default beat grid (or off), every key is its own
  change, so chords are played live; bigger jumps are their own keys (`u`,
  `m`, `i`, `,`) rather than repeated taps. Quantized, moves chain before the
  line (`l l`, `f d`). Voice leading is deterministic: the same chain from
  the same chord always sounds the same, which is what lets it become muscle
  memory. Undo makes a wrong chain cheap to recover from.
- **Eyes-closed, each key always means the same thing.** The thumb on Space
  says "harmony"; the fingers stay on the home row. The letters still mean
  other things in browsing (`hjkl` move and adjust, `a` vamps), but the held
  thumb makes the mode physical rather than remembered: there is no latch to
  forget.
- **Prepare more, play less?** No setup is needed: no chord bank, no key
  choice, no mode to keep track of.

## Keys

Hold Space: taps chain while it is down, each landing per the quantize
setting, and letting go returns to browsing. Space alone never edits
anything. A terminal without key-release reporting gets Space then one key
as one move. Focus loss ends a held push. Ctrl+C still quits.

| key (Space held) | move | from A minor |
| --- | --- | --- |
| `j` / `u` / `m` | down 1 / 2 / 3 scale steps | G / F / Em |
| `k` / `i` / `,` | up 1 / 2 / 3 scale steps | Bdim / C / Dm |
| `h` | down a fifth (fall; it resolves only from a dominant) | Dm |
| `l` | up a fifth (lift) | Em |
| `f` | R, relative swap | C |
| `d` | P, parallel major/minor flip | A |
| `s` | L, leading-tone swap | F |
| `a` | home: the key's tonic triad | Am |
| Backspace | undo the last move (16 deep) | |

The rows above and below the other move columns stay unbound for now.

Where a move lands is the chord quantize setting,
`/harmony quantize beat|1|2|4|off` (palette rows `Harmony quantize 1 beat /
1 bar / 2 bars / 4 bars / off`). It defaults to `beat`: a move lands on the
next beat line of the engine clock, live but in time. `1`, `2` or `4` wait
for that bar line; `off` lands on the next sample. Before the line, a second
move composes from the queued chord and keeps the line. Changing the setting re-times a waiting move
from now. The header shows the sounding chord and the queued one, with a bar
countdown only on a bar grid (`CHORDS  C → G in 1 bar  PUSH`). The overlay
shows now/next and the quantize setting, the two step columns laid out as on
the keyboard (`u j m`, `i k ,`), and the other moves, each with the chord it
would produce: the eyes-open learning aid. Every move and undo is also a `/`
operation (`/harmony lift`, alias `/push l`).

Physical holds keep running through the mode, as with the other performance
leaders.

### The rest of the v4 key map

- `'` is go: `'1`-`'9` queue states and `''` the previous one, as before;
  `'a s d f q w e r` open layers and `'j`/`'k` reach Level/Filter on the page
  in front of you (moved from Space). Digits, `'`, layer and parameter keys
  are disjoint sets, so nothing collides inside the leader.
- `"` is keep: `"1`-`"9` store states (unchanged).
- `g` is grab: `g1 g2 g4 g8` grab Motion on the selected knob, or on the
  selected field in an automation editor (moved from Space 1/2/4/8). `G1 G2
  G4 G8` loop the harmony heard over the last N bars (see Harmony loops).
- Planned mute (`Space m`) is retired with its palette row and OSC surfaces;
  a song code carrying one is refused as `RetiredControl("planned mute")`.

## Harmony model (`src/fluid/voice/triad.rs`)

- A chord is a `Triad`: a root pitch class and a quality (major, minor,
  diminished), in the progression's own coordinates before the Master Key
  shift.
- The key is the Master Key and Mode (`HomeKey::of`), in the progression's
  coordinates.
- Steps take the next scale degree strictly below or above the root (so an
  off-scale root steps onto the scale) and the mode's own triad there; a 2 or
  3 step is that many single steps (test-enforced).
- Fifths are perfect fifths. In the scale they take the diatonic quality;
  outside it they keep the current quality.
- P, L and R are the textbook swaps on major and minor triads. Each is an
  involution (P(P(x)) = x) and moves exactly one voice (both test-enforced).
  A diminished triad has no textbook P/L/R, so each moves one voice to a
  consonant neighbour: P raises the fifth (Bdim to Bm), R lowers the root a
  tone (Bdim to Dm), L raises the fifth a tone (Bdim to G).
- The first move reads the progression chord sounding now as a triad (its
  root, and the quality its third and fifth spell; sus and power chords take
  the mode's own triad on their root).

## Voicing and the engine seam

- At the landing sample the audio clock voices the triad from the four notes
  the pad is sounding (`voice_lead`): each voice moves to a nearby tone of the
  new triad, all three tones sound, no two voices share a note, the total
  movement is minimal, and the voicing stays in MIDI 40 to 79 so long walks
  never drift. Ties resolve in a fixed order, so it is deterministic.
- While a chord is pushed it overrides the progression the way a held Vamp
  does: the engine narrows the chord window to the sounding slot and overlays
  the pushed chord on it (`PadControls::held_chord`, audio-only, like the
  existing automation overlay). Pad, Arp and Lead read the pushed notes through
  `pad_chord_tones`, and every chord follower treats it as sounding. Bass
  moves its authored line by the root's distance within a tritone
  (`root_near`), so it follows the new root in its own register.
- A pushed chord and an undo change exactly as an authored Sound step does:
  the incoming layer fades in from silence over the pad Attack, the outgoing
  one rings out over Release, and a held chord sustains across chord
  boundaries without re-striking. `a_pushed_change_sounds_like_an_authored_one`
  compares the envelopes. (The first mock slurred pushes in, so the new pitch
  arrived at about two thirds of full level at once: the jolt Ian heard.)
- Undo walks back through pushed chords and never past the first landed push.
- Without an audio clock (replay, tests) the production tick lands the queue,
  the same fallback the Vamp uses.

## Persistence

Song record 16 (version 1) carries the held chord (slot, root, quality, four
notes) and a queued triad: at most 12 bytes. A queued move lands on the loaded
song's first bar; a restart lands it on bar one. Undo history is live-only.
Malformed, inconsistent or trailing records are refused with
`SongCodeError::InvalidPushHarmony`, including tag 2, the retired queued
release. A state landing takes the destination's own push state.

The quantize setting is not saved in song codes. Commandment 4 says a code
saves the musical state a person chose. Quantize produces no sound on its own:
it only decides when the player's *next* key lands, the way key repeat or a
display choice would. It is a play-style preference of the player, not part
of the song, so sharing a code must not change how someone else's keys
respond. What it already affected is saved: the pushed chord and any queued
move. It lives in the live session (`PushedHarmony::quantize`), survives state
landings, resets to `beat` on launch, and needs no record or migration.

## Harmony loops (`G`)

Ian's model is loop + home + pushes + states: play chords, loop what you
played at any length, push over it, store the result as a state. `G1 G2 G4
G8` loop the chords heard over the last 1, 2, 4 or 8 bars before the nearest
bar line, with their real timing, and start the first pass on that line.

- **Heard log.** The audio clock appends every pad chord onset to a bounded
  live-only log (`HeardLog`, 64 entries) as a `HarmonyEvent { at, triad,
  notes }`: authored chords (heard as a triad on their slot, with their own
  notes, so colour is kept), pushes and loop chords alike. Re-strikes of the
  same chord add nothing.
- **Loop.** `HarmonyLoop { slot, bars, start_beat, events }` holds at most
  32 events (`LOOP_EVENT_LIMIT`), offsets from the loop's start. 8 bars on the
  default beat grid is at most 32 changes, so it always fits; a denser window
  (quantize off) keeps its first 32 changes and holds the last to the end.
  It sounds on the slot that was sounding, like a held push, through the same
  Attack/Release path, and its chord is the base pushes read.
- **Replace and take over.** A grab replaces the loop and withdraws a waiting
  push. A landed push ends the loop and holds the pushed chord; undo still
  walks back pushes.
- **Clear.** `G0` / `/loop clear` (`PushedHarmony::clear_loop`) sets
  `loop_release` to the quantize line, as a push lands; `settle` ends the
  loop there and the pad crossfades back to the held push or the progression.
  A code saved while it waits omits the loop (`saved_loop`). Undo does not
  restore a loop: the undo history is a chain of chords, and mixing loops
  into it would make Backspace's meaning depend on what was done last.
- **Persistence.** Song record 17 (version 1): slot, bars, the start beat
  rebased to song beat zero (so a state or code resumes at the same phase),
  and per event an offset in 1/1024 beat, root, quality and four notes, 8
  bytes each: at most 268 bytes. States store it through the same song
  snapshot.
- **How it maps to ADR 0002.** A loop is already the shape of the chord-lane
  redesign: a lane of timed cells whose harmony (`Triad`, the root and
  quality ADR 0002's `Harmony` names) is kept apart from its voicing (the
  four sounded notes, ADR 0002's `Voicing` realized). When the Cell lane
  lands, a loop event becomes a `Cell { harmony, voicing, gate: Sound }` at a
  beat offset, a loop becomes a progression lane with free timing, and `G`
  writes that lane instead of a separate one. The triad widens to the full
  `Harmony` (tensions, slash bass); nothing here needs replacing.

## Master Key and Mode

Ian (2026-11-11): derive and set the key and mode on the master; progressions
are relative values over them; Home is neither a key nor a mode.

- **Concrete.** `master.key` is C through B and `master.key_mode` one of
  Major, Minor, Dorian, Phrygian, Mixolydian (`MasterKey { pitch, mode }`).
  Launch (`FluidControls::derive_key`) and every song or state load derive
  them from the progression's authored key, so nothing sounds different until
  they move. Choosing another progression keeps them.
- **Progressions are relative** (`voice/mode_map.rs`, `ModeMap`). The Key
  transposes (`MasterKey::shift`, as before). The Mode re-reads each
  *diatonic* chord, one with a major or minor third whose every note is in
  the authored scale: root, third, fifth and seventh move to the new mode's
  notes on the same degrees, so its quality follows; an upper extension (9th,
  11th, 13th, 6th, add tones) keeps its interval while the new scale has the
  note and is dropped otherwise, its voice taking the nearest free chord
  tone. Sus, power and borrowed chords keep their notes (transposed only).
  The Bass under a re-read chord moves by degree with it. Built-in data is
  never modified; Custom and edited slots are re-read the same way.
- **One home.** Pushes, `a`, Roman numerals, Lead/Arp/Bass Scale follow and
  the `G` loop all read the Master Key and Mode (`HomeKey::of`).
- **Mode change mid-play.** An authored chord re-voices at once through the
  pad's Attack/Release (`PadEngine::last_mode_map`), like a chord edit. A
  pushed chord or a loop keeps the notes it was given; only later moves read
  the new mode.
- **Song codes.** The key keeps its id and wire values (1..12 = C..B, so an old
  chosen key means the same pitch; 0, the retired Home, is refused as
  `UnknownValue`); its dial lost a position (`MASTER_KEY_CONCRETE_EPOCH`).
  `master.mode`, whose values meant "pushes only", is retired
  (`RetiredControl("master.mode")`); `master.key_mode` replaces it. Both are
  written only when they differ from the progression's own and derived when
  absent, so every built-in loads in its own mode (test-enforced).
- **Octave.** `master.octave` (-2..+2, default 0) adds whole octaves to the
  same `MasterKey::tune` the Key shift goes through, so Pad, Bass, Arp, Lead
  and Tonal move together and drums never do. `/octave up 1`, `/octave 0`.
- The palette has `/mode <name>` and `/key <pitch>`; black keys are labelled
  as flats with sharp aliases because palette search reads `#` as a word
  break.

## What this build leaves out

- A harmony loop has no gate: a loop chord always sounds. Rests captured from
  an authored Rest step are not logged, so the loop holds the chord before.
- The heard log starts over when the clock restarts or a state lands, so a
  grab right after a landing reaches back only to the landing.
- Smear (glide a move over a whole bar) is not built: it needs per-voice
  pitch glides in `PadLayer`.
- Triads only: no sevenths, tensions or inversions on pushed chords (Ian:
  out of scope), and no augmented quality (no home mode produces one).
- With a sub-bar Chord Length, a chord boundary between a quantized press and
  its line is not re-read.
- OSC state publishing (`/nooise/state/*`) still reports the authored slot;
  OSC note output follows the pushed notes.
- A push does not exit auto morph, the same as the Vamp.

## What only listening can confirm

- Whether the authored crossfade (6 s attack, 8 s release by default) makes
  fast chained pushes feel responsive enough, or blur together.
- Whether minimal voice leading keeps the pad's colour when the authored
  chord was a wide voicing (Am7add11 collapses to a close triad).
- Whether the Bass register rule (within a tritone of the slot's root) sounds
  like it follows, especially on fifths and 3-step jumps.
- Whether the diminished P/L/R choices feel right.
