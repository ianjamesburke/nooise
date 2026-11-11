# V3 performance checkpoint

Updated: 2026-11-06

## Playable Pad rhythm experiment

- `/pad.level`, then `/chop`, then Shift+Right on Amount gives a full chop
  over the existing swell. Eight quarter-beat steps, zero offset, soft edges.
  In the ordinary Steps editor, 0% retains the sound and -100% ducks it.
- Uses the existing shared LFO editor, save codec, and clock. No new song IDs,
  envelope mode, or separate UI. Chord Slur and harmonic followers remain intact.
- The separate web audition was discarded after user feedback. The user wants
  a playable in-app iteration first; melodic consolidation remains deferred.
- Build, all 997 tests (9 ignored), strict all-target Clippy, and formatting pass.
  Keyboard replay covers Chop insertion and save. A separate live PTY probe
  timed out during shutdown; no live listening acceptance is claimed.
- DOX updated in source, performance guide, and the owning proposal. Parent
  indexes unchanged because ownership and folder boundaries did not change.

## November 6 playing-feedback corrections

- Perc starts at zero. Its meter display floor is -66 dB instead of -42 dB;
  audio gain and telemetry remain unchanged.
- Clap Steady starts at Rate 0.25 beats, eight steps, zero Offset, only step 5
  active. The loop hits beats 2 and 4. Snapshot defaults remain fixed, so saved
  Clap patterns retain their original timing and steps; the current default
  round-trips explicitly and stays under 200 characters with every gesture on.
- The `b` hold is Beat Repeat: one quarter note of live Master, captured on the
  grid and looped in time. Loop length, grid start, seam, release and
  section-hold tests pass. Its musical feel still needs audition.
- Automated verification: 995 tests passed, 9 ignored; strict all-target Clippy,
  formatting, build, and diff checks passed. The unchanged golden DSP fixture
  explicitly raises Perc to its prior level, retaining its existing checksum.
- Release headroom probe: all 21 built-in songs and 38 fresh progressions, 354
  gesture renders plus dry references, zero clamp hits.
- Muted real-app PTY smoke displayed Clap Rate 0.25, stored/reopened A/B sections,
  exercised the `b` hold and release, and exited cleanly twice. Evidence lives under
  `target/playing-feedback/`; no listening or physical-keyboard acceptance claimed.
- The show plan records proposed independent Pad articulation rhythm and three
  independent melodic parts with shared machinery. The shared melodic redesign is not implemented; the small Pad rhythm shortcut is described above.
- DOX: source and engine contracts, performance guide, and owning proposal updated.
  Root/docs parent instructions and child indexes remain unchanged: no ownership
  boundary or directory changed.

## Stint 0007 completion on v3

- Implementation completed in this worktree on top of `7ba8a96`; the older `8573732` candidate was selectively ported, preserving subsequent v3 and show work.
- User confirmed v3 Slur remains: chord steps advance their own harmony without restarting the swell. Literal chord Tie in the original stint is superseded; Lead and Tonal retain literal note ties.
- Shared sixteen-step windows; Pad Rate reaches 0.125 beats. Lead and every Tonal character sustain through the complete final Tie and loop wrap. Tonal evolution preserves rests/ties, and stop, sequence edits, and section restoration release outgoing holds. Chord rests suppress new chord-following triggers; Scale remains playable.
- All 21 built-in songs re-encoded through the current encoder at range epoch 15. Older modulation on the expanded Pad Rate dial is refused as StaleRange.
- Verification: 991 Rust tests passed, 9 ignored; 6 Python authoring tests passed; format, strict all-target Clippy, build, and diff checks passed. New tests cover short-decay ties, loop wrap, initial source-less ties, evolution, follower rests, and section restoration.
- `src/AGENTS.md` updated for the shared sequence primitive, timing, sustain and current Slur contract. Root and product DOX documents and child indexes remain unchanged because ownership and workflow did not change.
- Muted real-app PTY smoke: the Rate row displayed 0.125 beats after keyboard reset; A/B sections stored, recalled, reopened, and both app launches exited 0. Evidence: `target/stint-0007/live-smoke.json` and `.ansi`. This does not establish listening or physical-keyboard acceptance.
- Hands-on listening remains pending. No PR, merge, release, or publication; run `cargo run` here to audition.

## Previous show checkpoint

- Worktree: `worktrees/v3`.
- Verified source commit: `54f9968` (`feat: add slapback delay preset`).
- Starting code: `aa47af3b48ce50ec7bd072397e3abad33407fcba`.
- Status: ready for the next 15–20 minute rehearsal; musical acceptance pending.
- No release PR, tag, or publication.

## Included

- Fresh Perc at 0.50 and Bass on Saw. Existing saved songs retain authored levels and sounds, including omitted historical defaults.
- Bass Offbeat has regular eighth-note offbeats instead of irregular pickups.
- Stereo feedback-delay-network reverb replaces Freeverb.
- Hold `b` or use `/repeat` for a quarter-note beat repeat of the live mix.
- `VAMP` and `VAMP ENDING` appear beside the chord.
- Tap `q` for a saved Kick+Bass latch; hold for a temporary gate. Individual mutes remain independent.
- Nine numbered sections (1-9): `"` then a digit stores (overwriting), `'` then a digit queues for the end of the phrase (the header counts down the bars; the queued digit again cancels, `''` queues the previous section), all also reachable through `/`; `--set PATH` persistence. The key header shows the current and queued sections. Recall retains performed holds, restores the destination snapshot, and lands at its authored tempo and beat zero.
- `/slapback` sets Delay Amount to 40%, both synchronized times to 0.25 beats, and Feedback to 0%. It also reapplies these values to an existing Delay.

The [performance guide](docs/PERFORMANCE.md) owns the controls and terminal-capability contracts.
The [show plan](docs/proposals/2026-11-02-show-readiness.md) owns the rehearsal loop.
Ian deferred Melody 1/2/3 consolidation and longer sequences to the following checkpoint.
Named songs, `/songs`, transition previews, and session/Baton joining remain proposed follow-on work.
Microphone work stays separate. The [October gate](docs/proposals/2026-10-05-stabilization-gate.md) retains historical evidence.

## Verification

- Final build, format, strict all-target Clippy, and diff checks: PASS.
- Full final suite: 981 passed, 0 failed, 9 ignored. Baseline: 955 passed, 0 failed, 8 ignored.
- Production keyboard/session/audio coverage: A → B → C → A, queue cancel/replace, incoming tempos, bank reopen, invalid code refusal, saved Tonal evolution and module Motion, physical holds across recall, and cancellation during the transition fade.
- Populated render inspection at 46×11 and 80×24: current/queued sections, Vamp, q latch and the `b` hold remain visible together. `target/show-checkpoint/combined-frames.txt` records the combined frame.
- Actual app/PTY smoke with Master at zero: stored A/B, recalled them, used `/slapback` and the `b` hold command, quit, reopened the bank and recalled again. Both launches exited 0. Evidence: `target/show-checkpoint/live-smoke.json` and `.ansi`. This was not listening or physical keyboard acceptance.
- Two final seed-21, 30-second renders match SHA-256 `9da9d0579325c538c1b8779c637eb4f4c75454d84bd4beab67c4c091cab4280b`. Files: `target/show-checkpoint/after.wav` and `after-repeat.wav`.
- Broad DSP probe after sound/gesture integration: 59 built-in songs/fresh progressions, 354 wet gesture cases, zero clamp hits; highest dry peak 0.341 and wet peak 0.335. This diagnostic preceded section/preset integration. The final full suite reran the populated-hold combined loud-song headroom gate. Full table: `target/show-checkpoint/all-song-levels.log`.
- Final checks: `target/show-checkpoint/final-gates.log`. Baseline binary/render and candidate binary are retained in the same local artifact directory.

## Listening and performance gate

The new reverb is deliberately a candidate for audition. Its six-second wet
impulse RMS is 0.00105 versus the old 0.00737; peak is 0.063 versus 0.307.
A fourfold gain increase failed sustained-signal and transport checks and was
rejected. Do not equate safe signal measurements with a better-sounding reverb.
Compare `target/show-checkpoint/reverb-before.wav` and `reverb-after.wav`, then
listen on actual show material. Captured audio is not part of saved songs.

1. Launch with `cargo run -- --set show-rehearsal.set`. Prepare contrasting
   snapshots with `"1`, `"2`, `"3` (or `/store section 1`). Storing into an
   occupied slot replaces it at once, with no prompt.
2. Record 15–20 uninterrupted minutes. Play 1 → 2 → 3 → 1 with `'1`, `'2`, `'3`
   or `/queue next section`; cancel one queued destination with the same digit, use
   `''` to go back, and recover from one wrong section. Include a different-tempo destination.
3. Play q tap/hold, Vamp ending, Beat Repeat release, Offbeat and Slapback. Listen for
   useful balance, clicks, dropouts and a clean ending on the intended audio route.
   Physical holds need a terminal reporting key releases; fallback palette paths
   remain available.
4. Quit and reopen the same bank. Confirm the intended snapshots, then timestamp
   the three largest interruptions and use them to choose the next batch.

The set-duration rig rehearsal, subjective reverb choice, physical key-release
behavior and real-device dropout acceptance remain open. The section publication
uses a bounded CAS attempt but still clones the prepared snapshot on audio;
no worst-case populated landing-latency benchmark has been claimed.
