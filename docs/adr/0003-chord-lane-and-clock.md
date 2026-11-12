# ADR 0003: One chord lane, one chord clock

- Status: Accepted 2026-11-11 (design and rulings by Ian); not implemented,
  lands in the build order below
- Date: 2026-11-11
- Amends: [ADR 0002](0002-v3-chord-model.md). Its `[Cell; 16]` plan
  (Remaining) and its `ProgressionFollower`/`ProgressionCursor` carry-forward
  (invariant 5, Carried forward) are replaced by this lane and clock. Its
  `Harmony`/`Voicing`/`Gate` split stands.
- Scope: chord timing, the downbeat reset on harmony, Vamp, Push the Harmony,
  harmony loops (`G`), and how chord-following voices and the UI find the
  sounding chord. Paths below are `src/fluid/` unless stated.

## Context

- **Shift+D must rotate the song, not restart a chord.** Pressing the
  downbeat (`d` today, `EffectExecutor::reset_downbeat`; the Shift+D rebind is
  separate show work) should make the chord pressed on become chord 1, with
  the song rotated underneath and no audible break. Today the sounding chord
  restarts its count instead (`docs/PERFORMANCE.md:304-307`, `CONTEXT.md:37-40`).
- **Every chord is the same length.** One `pad.chord_bars` per phrase
  (`controls.rs:277`, `registry.rs:1810-1832`), so a change cannot fall on
  beat 3.
- **Five stateful clocks for one harmony.** `ProgressionCursor`
  (`voice/pad.rs:1863-2041`) is held by the Pad and, through
  `ProgressionFollower` (`voice/pad.rs:2047-2069`), by Bass, Tonal, Arp and
  Lead, each reset by hand (`engine.rs:882-885`). The UI keeps a sixth
  estimate (`sounding_chord`, `effect.rs:2690-2701`). The "what plays next
  after a window edit" rule exists twice: `step_after_boundary`
  (`voice/pad.rs:1948-1960`) and `pad_landing_slot_at` (`engine.rs:1550-1580`).
- **Step sequencers are already stateless.** Their step index is a pure
  function of the beat (`lane_step_at`, `voice/mod.rs:40-45`; used at
  `voice/kick.rs:146`, `voice/tonal.rs:453`, `voice/arp.rs:379`);
  `GridTrigger` (`engine.rs:2551-2645`) only schedules hits.
- **Vamp pauses the progression.** It narrows the window to its slot
  (`engine.rs:1204-1208`) and on release re-anchors the phrase
  (`engine.rs:1630-1647`, `planned.rs:3-12`), so release cannot rejoin where
  the song would be.
- **A `G` loop is a second timed harmony system** (`HarmonyLoop`,
  `push.rs:190-293`; a landed push ends it, `push.rs:431-442`): 1/2/4/8 bars
  only, `LOOP_EVENT_LIMIT = 32` (`push.rs:118`).
- **Pushes and loops store absolute chords** (root pitch class, quality, four
  MIDI notes; records 16 and 17, `song.rs:66-75`), outside Master Key/Mode.

## Decision

One timed-event lane, one clock (design C).

1. **`HarmonyLane`**: a length in whole beats, a rotation in beats, and sparse
   events (beat offset + cell), each sounding until the next. A grid is only
   an editing and quantize view, never the storage.
2. **Stateless position.** `phase(beat) = (beat - origin + rotation) mod length`,
   the same shape as `lane_step_at`. One `LaneClock` in the engine owns
   origin and the lanes; every voice reads its result the way `held_chord` is
   injected today (`engine.rs:1209-1216`). The UI calls the same pure
   function. The five cursors and the UI estimate are deleted.
3. **Edits are one queued lane swap.** Window, length and progression edits
   that wait for the boundary today become a single lane swap the clock
   applies at the sounding event's end, keeping the current successor rule
   in one place. Chord Length is LFO-modulatable (`registry.rs:1832`,
   `LfoSnap::Step`); a stateless phase jumps when length changes, so modulated
   length also goes through the queued swap, never a live re-read.
4. **Relative events.** An event stores degree + accidental + quality against
   the concrete Master Key and Mode (ADR 0002 `Harmony`, 0002:60-63) and its
   voicing as chord-tone index + octave (0002:65-68). Diatonic events
   (accidental 0) re-derive on a Mode change; borrowed events keep their
   accidental.
5. **Three layers, each a pure function of the beat:**
   1. base lane, compiled from the progression;
   2. optional loop lane, written by `G` at any whole-beat length; `G0` clears
      it;
   3. hold overlay: a chord held from a beat to an optional end. Vamp and a
      landed push are the same thing. The clock keeps running under a hold,
      so release rejoins where the song would be.
6. **States snapshot all three layers.**

### Rulings (Ian, 2026-11-11)

1. **Shift+D is a pure rotation.** Each lane's rotation becomes its phase at
   the nearest beat line; the transport rewinds by whole beats as today. No
   event moves; no chord is cut or extended. The pressed chord draws first
   with its remaining beats and its head draws at the end. Seam, ribbon start
   and phrase end read the rotation.
2. **Rotation is saved** in states and song codes: a state landing restarts
   the clock at beat 0 (`session.rs:352`), so it must start at the seam. The
   transport stays live-only.
3. **Vamp keeps the clock running** on the plain progression too, one rule
   for both; release rejoins at the next chord boundary.
4. **Loop lengths are any whole-beat length**, not just 1/2/4/8 bars.
5. **Finest authored resolution is a sixteenth** (0.25 beat), the same grid
   the drum step sequencers use.

Adopted defaults: a push over a loop ends the loop and holds; Shift+D moves
the loop lane's seam too; v3 song-code breakage at step 6 is accepted after
the show.

## Rejected alternatives

- **Keep `[Cell; 16]` and add a rotation to `ProgressionCursor`.** Five
  counters each need it, chords stay equal-length, and the loop stays a
  second system.
- **One stateful cursor instead of five.** Removes the duplication but keeps
  hand resets on every restart and rewind; a pure function of the beat
  cannot drift from the step sequencers.
- **Rotate the stored slots on Shift+D.** Rewrites the song the player chose
  and cannot put a seam inside a chord.
- **Unify the clock but keep `HarmonyLoop` separate.** Two harmony timelines
  and two push-over rules remain.
- **Keep absolute chords in events.** Pushes and loops would not follow
  Master Key or Mode.

## Consequences

Build order (each step its own change; gate on audio fixtures where marked):

| # | Step | Size | Breaks |
| --- | --- | --- | --- |
| 0 | This ADR | S | nothing |
| 1 | One chord clock; lane compiled from existing slots; five cursors and the UI estimate deleted; no behaviour change, gated on existing audio fixtures staying identical | L | tests reading `cursor.step`; `src/AGENTS.md:72,74,160` wording |
| 2 | Shift+D rotation | M | `engine/downbeat_tests.rs:229,262,283`, `push.rs:689`, `CONTEXT.md:37-40`, `docs/PERFORMANCE.md:286-315`; one new record (next free: 18) omitted at zero, so old codes still load |
| 3 | Loop is a lane, any beat length, `G0` | M | record 17 to version 2; saved loop states refused |
| 4 | Hold overlay unifies Vamp and push | M | Vamp release behaviour, records 14 and 16, `docs/PERFORMANCE.md:172-184`; `vamp_landed_*` logic goes |
| 5 | Relative harmony in events; depends on concrete Key/Mode landing | M | records 16/17 again (batch with 3 and 4); the "Mode never rewrites" contract (`CONTEXT.md:82-87`, v4 proposal Master Mode) |
| 6 | Beat-level lane editor; progressions stored as lanes | XL | container v3, every song code, built-ins re-authored; pairs with the song-code/patch rework |

Risks:

- LFO-modulated Chord Length (decision 3).
- A pure rotation leaves changes on count 3 of every bar while drums stay
  offset. Intended.
- Beat-level changes barely register on the Pad (6 s attack, 8 s release,
  `docs/proposals/2026-11-11-v4-push-the-harmony.md:228-229`); they are
  heard mostly in Bass, Arp and Lead.
- Auto-morph legs round to whole phrases (`CONTEXT.md:113-117`); a rotated
  seam changes leg lengths. Moot if the song-code rework deletes auto-morph.
- A backwards beat already upsets the baton hub (external).
- `LOOP_EVENT_LIMIT` (32) must rise for long any-length loops.

Schedule: only the Shift+D rebind and the lag fix ship before the
2026-11-22 show (freeze Nov 17-18,
`docs/proposals/2026-11-02-show-readiness.md:202`). Steps 1-6 wait. The show
runs a pinned binary, so branch work cannot reach it.

## Verification scenarios

- Step 1: every existing audio fixture renders identically; Pad, Bass, Tonal,
  Arp, Lead and the UI read one `LaneClock` result at every beat.
- Shift+D mid-chord: no onset, release or pitch change at the press; the
  pressed chord draws first; a state stored after it reloads at the seam.
- Chord Length swept by an LFO never moves the phase inside a chord.
- Vamp or push held for N beats then released lands on the chord the
  unheld lane sounds at that beat's next boundary.
- A 6-beat `G` loop round-trips through a state; `G0` returns to the base
  lane.
- Step 5: a Mode change re-derives diatonic events and keeps borrowed ones.
