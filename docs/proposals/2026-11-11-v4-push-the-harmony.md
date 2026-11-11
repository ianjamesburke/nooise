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

Moves land on the next bar. A second move before that bar composes from the
queued chord and keeps the same bar, so `l l` is two fifths up. The header
shows the sounding chord, the queued chord and the bar countdown
(`CHORDS  C → G in 1 bar  PUSH`). The overlay lists the eight moves with the
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
  (`root_near`), so it follows the new root in its own register. A pushed chord slurs in and sustains across chord boundaries.
- Release (`g`, or undo back past the first move) lands on the next bar like a
  move: the held slot's authored chord sounds again and the progression moves
  on at its next chord boundary. This is the clean way back; the Vamp (`a` in
  Browsing) stays independent.
- Without an audio clock (replay, tests) the production tick lands the queue,
  the same fallback the Vamp and planned mute use.

## Persistence

Song record 16 (version 1) carries the held chord (slot, root, quality, four
notes) and a queued target (chord or release): at most 12 bytes. A queued move
lands on the loaded song's first bar; a restart lands it on bar one. Undo
history is live-only. Malformed, inconsistent or trailing records are refused
with `SongCodeError::InvalidPushHarmony`. A section landing takes the
destination's own push state.

## What the mock leaves out

- Smear (hold Space to glide a move over a whole bar) is not built. A real
  glide needs per-voice pitch glides in `PadLayer`, which does not exist; the
  mock lands every move with the existing slur entry. Space keeps its Jump
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

- Whether slurring each landing (no re-attack) reads as a smooth shift or as
  a smear, with the default 6 s attack and 8 s release.
- Whether minimal voice leading keeps the pad's colour when the authored
  chord was a wide voicing (Am11 collapses to a close triad).
- Whether the Bass register rule (within a tritone of the slot's root) sounds
  like it follows, especially on fifths.
- Whether the diminished P/L/R choices feel right.
