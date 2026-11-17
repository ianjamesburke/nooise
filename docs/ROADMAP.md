# Roadmap

Written 2026-11-17. Show: Salt Lake City, 2026-11-22, 5:30-6:30 pm. This file
holds direction and order; designs live in `docs/proposals/`, rulings in
`docs/adr/`.

## Lines

- **main (release line).** What `cargo install nooise` gets; crates.io is at
  2.9.1 (2026-10-25). Roadmap work branches off main and lands back on it
  through PRs; each release is a crates.io version.
- **v3-show (show line).** Branched from `temp-show-marks`, 69 commits ahead of
  main: the instrument without custom chord playing, plus the show fixes
  below. Pinned for Nov 22. Merges into main right after the show and ships as
  the next crates.io release.
- **v4 (chord performance mode).** `v4` (stable) and `v4-alpha` (work): Space
  harmony pushes, `G` harmony loops, Master Key/Mode, Shift+D, resume. Stays
  a long-lived branch off the release line, rebased onto main after the v3
  merge, and lands as a major version when chord mode is ready.

## Now: before Nov 22 (show line only)

Only small, rehearsed changes. The freeze is the last rehearsal day before
travel; after it, only show-stopper fixes, each re-rehearsed.

Landed on `v3-show` (unheard until rehearsed):
- `t` held: eighth-note beat repeat; unit flip is Shift+T only.
- Beat repeat release lands a full downbeat.
- Vamp holds through progression changes; release starts the new progression
  on its first chord.
- `z` mutes Kick alone, Shift+Q mutes Bass alone, `q` mutes both.
- Six Salt Lake progressions (First Light, Salt Flats, Wasatch, Great Salt
  Lake, Night Drive, Last Train Home) and starter bank `docs/show/salt-lake.set`.
- From v4: +6 dB master makeup with -1 dBFS limiter (ADR 0004), audio callback
  overrun timer.

Open:
- Robot voice (`v3-show-robot`): plays the show only if rehearsals say so; off
  by default.
- One full logged 60-minute rehearsal on the show profile.
- Venue profile (direct audio, no mixer/BlackHole) with the real output device
  name; `idle_stop_minutes: null`, `recording.events: false`.

## Next: right after the show

1. **Merge v3-show into main** and publish the next crates.io version; rebase
   v4 onto it.
2. **Flip** ([proposal](proposals/2026-11-17-flip.md)): one bar-quantized,
   undoable multi-layer change that keeps the clock and chord running.
   `/flip` and `/flip back` first. Recommended rulings, pending Ian: Robot
   offers only on request; Flip never changes chords; arrangement
   deterministic, dice only on entering layers; auto-morph frozen, later
   rebuilt as timed flips.
3. **Robot** ([proposal](proposals/2026-11-17-robot.md)): tune by ear, OSC
   events and visuals, answer what Ian plays, chord requests (`/chords
   new|longer|shorter`) landing at phrase end. Fit its style settings from
   Ian's journals.
4. **Step sequencer**: level, decay and pattern on one screen; a jump from a
   layer to its pattern. Consider unifying patterns with LFOs (step-and-hold
   with pitch) so patterns, LFOs and the Robot share one modulation engine.
5. **Motion looping**: loop LFO amounts as well as knob positions.

## Later

- Sound world: more wavetables, percussion families (Perc to hats), textures,
  more progressions.
- More left-hand temporary effects.
- Gesture broadcast: Rise, Kick+Bass Mute and Perc Roll amounts over OSC, so
  visuals can react to them.
- Remote visuals: second-computer foorm and Clem's client over baton
  (`baton` repo, `docs/VISUALS-PROTOCOL.md`).
- v4 chord lane (ADR 0003 steps 1-6), patch-style song codes, retiring
  built-ins and auto-morph once Flip covers them.
- Ableton Link for syncing with other players.
- Performance: allocation at state landing and plan rebuild; per-note
  allocations.
