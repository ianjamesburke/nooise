# v4 mock-up: Push the Harmony

Status: playable mock-up on branch `temp-v4-harmony` (cut from the v3 show
build `temp-show-marks`). Design approved by Ian; exploration, not the show
build. Listening and hands-on play are the open gate.

## Idea

Ambient harmony lives in smooth single-note shifts. You never name a chord:
each key moves the harmony from where it is now with minimal voice movement,
using the neo-Riemannian P, L and R swaps plus diatonic steps and fifths. You
steer by feel, it is impossible to play wrong, every key sits on the home row,
and it works with eyes closed.

## Against the North Star posture

The owner's "video game engine of music generators" posture (`docs/NORTH_STAR.md`,
committed as 19d527d on `temp-show-marks` after this branch was cut, so not in
this branch's copy) names Push the Harmony as its example. How this mock-up
scores against each preference:

- **Moves, not set values or prepared material.** Every key is relative to
  the chord sounding now; nothing is named, chosen from a list, or authored
  ahead. Release returns to the prepared progression, but only as an exit.
- **Cannot be played wrong.** Every move key always produces a chord: the
  diminished P/L/R neighbours exist so no key is a dead end, and every chord
  stays inside MIDI 40-79. `u` with nothing to undo and `g` with nothing pushed
  say so and change nothing.
- **Rewards practice.** Unquantized (the default), every key is its own
  change, so chords are played live. Quantized, moves chain before the line
  (`l l`, `f d`), so speed buys bigger moves per line. Voice leading is deterministic: the same chain
  from the same chord always sounds the same, which is what lets it become
  muscle memory. Undo makes a wrong chain cheap to recover from.
- **Eyes-closed, home row, each key always means the same thing.** All eight
  moves sit on the home row and mean the same thing every time inside the
  mode, and `c` then a key is always the same two-key move. Open tension: the
  same letters mean other things in browsing (`hjkl` move and adjust, `a`
  vamps, `u` randomizes the selected knob), so the meaning depends on the
  mode. The approved design accepts this. The latched mode is where it
  matters most, since you have to remember you are in it.
- **Prepare more, play less?** No setup is needed: no chord bank, no key
  choice. The latch adds one mode to keep track of; the one-shot `c` does not.

## Keys

`c` in Browsing opens the mode for one move, then returns to Browsing.
Shift+`C` latches it until Esc. A terminal that reports the bare glyph `C`
latches it too. Ctrl+C still quits. Inside the mode only:

| key | move | from A minor |
| --- | --- | --- |
| `j` | step down the home scale | G |
| `k` | step up the home scale | B dim |
| `h` | down a fifth (resolve) | D minor |
| `l` | up a fifth (lift) | E minor |
| `f` | R, relative swap | C major |
| `d` | P, parallel major/minor flip | A major |
| `s` | L, leading-tone swap | F major |
| `a` | home: the key's tonic triad | A minor |
| `u` | undo the last move (16 deep) | |
| `g` | release: back to the authored progression | |

Where a move lands is the chord quantize setting. It defaults to off: a move
lands on the next sample, so chords can be played live, and each key is its
own change. `/harmony quantize 1`, `2` or `4` (palette rows `Harmony quantize
off/1 bar/2 bars/4 bars`) make moves wait for the next 1, 2 or 4 bar line;
then a second move before that line composes from the queued chord and keeps
the same line, so `l l` is two fifths up. Changing the setting re-times a
waiting move from now (off lands it at once). The header shows the sounding
chord and the queued one, with a bar countdown only when quantized
(`CHORDS  C → G in 1 bar  PUSH`); the overlay's first line shows the setting
(`now C → G   quantize 1 bar`). The overlay lists the eight moves with the
chord each would produce from the current base, which is the eyes-open
learning aid. Every move, undo and release is also a `/` operation (`/harmony
lift`, alias `/push l`, and so on).

Physical holds keep running through the mode, as with the section leaders.

## Harmony model (`src/fluid/voice/triad.rs`)

- A chord is a `Triad`: a root pitch class and a quality (major, minor,
  diminished), in the progression's own coordinates before the Master Key
  shift.
- The key is the progression's home (`progression_home`: tonic and one of the
  five home modes). Custom uses its A-minor home.
- Steps take the next scale degree strictly below or above the root (so an
  off-scale root steps onto the scale) and the mode's own triad there.
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
  `pad_chord_tones`, and every chord follower treats it as sounding (never a
  Rest). Bass moves its authored line by the root's distance within a tritone
  (`root_near`), so it follows the new root in its own register. A pushed chord, its release and an undo change exactly as an authored Sound
  step does (the incoming layer fades in from silence over the pad Attack, the
  outgoing one rings out over Release), and a held chord sustains across
  chord boundaries without re-striking. `a_pushed_change_sounds_like_an_authored_one`
  compares the envelopes. The first mock slurred pushes in (the incoming layer
  started at the level the pad held), which put the new pitch in at about
  two thirds of full level at once: the jolt Ian heard.
- Release (`g`, or undo back past the first move) lands like a move: the held slot's authored chord sounds again and the progression moves
  on at its next chord boundary. This is the clean way back; the Vamp (`a` in
  Browsing) stays independent.
- Without an audio clock (replay, tests) the production tick lands the queue,
  the same fallback the Vamp and planned mute use.

## Persistence

Song record 16 (version 1) carries the held chord (slot, root, quality, four
notes) and a queued target (chord or release): at most 12 bytes. A queued move
lands on the loaded song's first bar; a restart lands it on bar one. Undo
history is live-only.

The quantize setting is not saved in song codes. Commandment 4 says a code
saves the musical state a person chose. Quantize produces no sound on its own:
it only decides when the player's *next* key lands, the way key repeat or a
display choice would. It is a play-style preference of the player, not part
of the song, so sharing a code must not change how someone else's keys
respond. What it already affected is saved: the pushed chord and any queued
move. It lives in the live session (`PushedHarmony::quantize`), survives
section landings, resets to off on launch, and needs no record or migration. Malformed, inconsistent or trailing records are refused
with `SongCodeError::InvalidPushHarmony`. A section landing takes the
destination's own push state.

## What the mock leaves out

- Smear (hold Space to glide a move over a whole bar) is not built. A real
  glide needs per-voice pitch glides in `PadLayer`, which does not exist; the
  mock lands every move through the authored chord-change path. Space keeps its Jump
  meaning.
- Triads only: no sevenths or tensions on pushed chords, and no augmented
  quality (no home mode produces one).
- The move composes from the chord sounding when the key is pressed. With a
  sub-bar Chord Length, a chord boundary between the press and the bar is not
  re-read.
- OSC state publishing (`/nooise/state/*`) still reports the authored slot;
  OSC note output follows the pushed notes.
- Arrows and Tab do nothing while the latched mode is open. The one-shot `c`
  entry keeps the arrows-and-Tab floor intact; the latch is the opt-in
  exception Ian approved. Revisit if the latch feels like a Deck mode.
- A push does not exit auto morph, the same as the Vamp.

## What only listening can confirm

- Whether the authored crossfade (6 s attack, 8 s release by default) makes
  fast chained pushes feel responsive enough, or blur together.
- Whether minimal voice leading keeps the pad's colour when the authored
  chord was a wide voicing (Am11 collapses to a close triad).
- Whether the Bass register rule (within a tritone of the slot's root) sounds
  like it follows, especially on fifths.
- Whether the diminished P/L/R choices feel right.
