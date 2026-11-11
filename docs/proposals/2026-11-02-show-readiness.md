# November 22 show readiness

Status: the November 3 performance batch is integrated into V3. The show date,
V3 target, and rehearsal/post-mortem loop are confirmed. Listening and the
15–20 minute performance gate remain pending. Melody consolidation and longer
sequences stay at the following checkpoint, as confirmed by Ian.
[STATE.md](../../STATE.md) owns candidate provenance, checks, and remaining acceptance.

## Outcome

Play a prepared set on November 22, 2026 (5:30 to 6:30 pm, one hour), with room to improvise, moving between
musical states and songs inside one running nooise instance. Develop on `v3`
or worktrees based on it. Each batch ends with a recorded performance and a short
post-mortem that determines the next batch.

Set duration, other performers, venue audio connection, and required external
gear remain to be confirmed. Until then, use a 20-minute rehearsal as a planning
placeholder; the final rehearsals must run for the actual booked duration.

## Work order

The current batch covers Perc startup level, Bass's Saw startup sound, Bass Offbeat, replacement reverb,
temporary freeze/smear, Vamp visibility, q tap/hold, and live state recall.
The follow-up `/slapback` preset request also belongs to this batch: Delay
Amount 40%, left/right time 0.25, Feedback 0%.
Its next human gate is a 15–20 minute set with state recall and recovery.
The melodic spike follows that rehearsal.

1. Record a short baseline set on current V3. Prepare three contrasting states
   for one piece and write down exactly where current controls interrupt playing.
2. Build the smallest usable state bank and live recall path. Play an intro,
   main state, breakdown, and return without restarting the process.
3. Resolve melodic behavior through a bounded spike and playing comparison.
4. Assemble the full set; fix the problems heard and encountered in rehearsal.
5. Freeze the show build and its song material; rehearse the actual rig twice.

## State recall and song structure

The first deliverable is a named bank of states with a manual next-state
operation. The performer can stay in a state indefinitely, choose another,
see which state is queued, and cancel or replace it before it lands. Every
operation must be reachable through normal browsing or `/`.

Timing rule: changes land at the end of the sounding phrase, with an explicit
cancel path. The first spike landed on the next bar; after playing it the owner
chose the phrase boundary on 2026-11-10, because a bar landing cut the chord
progression mid-phrase. The bar-versus-phrase comparison is settled.
It must settle BPM changes, destination phrase position, active automation,
evolved notes, pending mutes, vamp, gestures, and the fate of unsaved live edits.
Suggested first policy: recall restores the authored snapshot; saving a new
version is explicit. Avoid accidentally overwriting a prepared state.

Start with an audible, click-free state change. Try parameter morphing only if
the rehearsal needs it. An audio crossfade between two engines has different
costs and behavior and requires its own decision. Validate a destination before
adopting it; an invalid or retired code must leave the current music playing.

A compact song code remains one musical snapshot. Names, ordering, and transition
instructions belong in a separate set/session document. Loading a set must not
require gear configuration, and ordinary startup must remain immediately playable.
Do not build a timed arrangement editor unless manual state recall proves
insufficient in rehearsal.

Acceptance: play A → B → C → A without a restart or unintended silence; cancel
and replace a queued transition; load an invalid code without disrupting playback;
save/reopen the bank and hear the chosen melody/evolution/automation state return.
Test both a same-tempo state change and a change between songs at different tempos.

## Melodic spike

Explore Melody 1, 2, and 3 as independent instances with a shared editing model.
Separate how notes are produced (Arp, authored Sequence, evolving Sequence, live
playing) from sound choice, Chord/Scale following, effects, and automation.
The exact labels, mode boundaries, and number of instances remain open.

Use one piece to compare today's Tonal/Lead/Arp with the proposed model. Preserve
their distinct musical behaviors; common controls must not make every part sound
the same. Prove independent notes, sound, evolution, mute, effects, and saved state
in two instances before expanding the design.

Test a recognizable 4- or 8-bar melody. A slower 16-step loop provides duration
but may not provide enough note events. Compare that with a short chain of
16-step patterns before choosing a larger sequence/editor rewrite. Keep the
feature only if the phrase improves the set and remains easy to edit by keyboard.
Timebox the exploration to two working days, then choose a show-sized slice or defer.

Any retired controls or changed ranges follow the existing refusal/re-authoring
policy. Settle those changes before authoring the final show library.

## q tap and hold

Implemented: tapping `q` latches Kick+Bass mute; tapping again clears it.
Holding for at least 250 ms mutes temporarily. Individual layer mutes remain
independent. The saved latch persists; physical holds stay live-only.
[Performance](../PERFORMANCE.md) owns the input, focus, fallback, and recall
contracts. Here “cue” means the q key, not queuing a song transition.

## November 3 playing notes

These observations drove the implemented batch. The proofs below remain part of the listening and rehearsal gate.

| Topic | Captured request | Next proof |
| --- | --- | --- |
| Perc startup level | November 6 revision: start at 0%. | Verify a silent Perc on fresh launch; saved songs retain their authored level. |
| Bass startup sound | Default Bass to Saw on a fresh launch. | Verify Saw at startup while saved songs retain their authored sound, including an omitted historical Sub default. |
| Bass Off Beat | It sounds like a broken beat rather than an offbeat bass pattern. Reported behavior, cause unverified. | Reproduce against a straight kick, inspect onset timing, and audition the corrected offbeat feel. |
| Reverb | The current sound is unsatisfactory; a new algorithm is wanted. | Prioritize the reverb slice of 0011 and compare replacements on the same show material by ear. |
| Temporary freeze/smear | Explore an auto-freeze gesture that stretches and repeats grains into a smeared texture. | Spike capture, sustain/repetition, and return to the playing mix; choose the binding, timing, and release behavior through audition. Exact meaning of auto-freeze remains to be settled. |
| Vamp visibility | Active Vamp and its pending ending need to be obvious. The bottom text is crowded and confusing. Suggested placement: near the chord at the top, with `VAMP` and `VAMP ENDING`. | Check active, latched, and release-pending states while playing and browsing, including the minimum terminal size. Placement and wording are proposals; immediate status recognition is the requirement. |

Perc and Bass defaults, Bass Offbeat, the replacement reverb, freeze/smear,
Vamp visibility, and `/slapback` are in the candidate. Freeze uses `b` or
`/freeze`; its captured audio remains transient and stays out of song codes.
The replacement reverb is quieter than the old algorithm and needs a musical
listening decision; automated signal checks do not settle that choice.

## November 6 playing feedback and design recommendation

Confirmed defaults: Perc starts at zero. Clap Steady uses Rate 0.25 beats,
eight steps, zero Offset, and only step 5 active: beats 2 and 4 in a repeated
4/4 bar. Perc needs a more sensitive display without extra audio gain.
Freeze must sustain captured pitch; the half-speed grain playback caused an
octave drop. These corrections require audition after automated checks.

**First playable Pad rhythm experiment:** `/chop` on Pad Level seeds the
existing Steps LFO as an eight-step, quarter-beat duck pattern, initially at
zero Amount. Raise Amount to audition chopping without restarting the swell.
The user requested the experiment directly in nooise; a separate web audition
was discarded. No new saved IDs or parallel sequencer are introduced.

**Further Pad articulation proposal, not implemented:** separate the progression clock
(which chord, including a 64-bar phrase) from a repeating articulation pattern
(when that chord sounds). Start with a bypassed pattern, editable Rate, Steps,
Offset, and Hit/Hold/Rest cells in an ordinary keyboard drill. Hit retriggers the
current harmony; Hold sustains it without an attack; Rest closes the Pad envelope
with a short release. A chord change updates harmony under an open gate without
restarting the rhythmic phase. Preserve v3 Slur. Keep rhythmic rests local to
Pads: they must not silence Chord-following melodic parts. Existing progression
Rest still means no harmonic source under the accepted chord contract.

The first slice uses amplitude chopping through the shared Steps lane. Envelope
retriggering remains a separate note-generation proposal: a six-second Pad
attack makes sixteenth-note retriggers nearly inaudible. Audition the in-app
shortcut before adding another sequencer or a dedicated gate module. Never reuse the retired Pad Stabs IDs. Persist the
chosen pattern and phase/anchor compactly; check recall, tempo changes, Vamp,
transport restart, and chord/chop boundary coincidences.

**Melodic recommendation, not implemented:** keep three independent parts with
one common editing model. Separate note source (live keys, authored sequence,
arpeggiator), sound, articulation/voice allocation, and effects. Tonal and Arp
already share `TonalVoice`; Lead differs materially through monophonic glide,
held keyboard input, and phrase capture. Preserve those capabilities explicitly
instead of flattening all voices to one behavior. Evolution modifies a sequence;
it need not become a fourth channel or an exclusive note-source mode. Live keys
should play over the focused part even while its sequence runs, and capture
into its sequence.

First share note events and sequence semantics internally with current labels
and saved IDs intact. Then audition two independent instances, including a
monophonic sliding part against overlapping piano notes, before a three-part UI
change. Compare keyboard reach, arpeggiator behavior, saved evolving state,
mutes, effects, and CPU. A rename alone does not resolve the current duplication.

## November 10 requests

| Topic | Captured request | Next proof |
| --- | --- | --- |
| Startup drums | Clap read Rate 2 in the default set; it should start at 0.25 with step 5, and Kick at 0.25 with steps 1 and 5, "across the board for start up". | Fresh launch and every built-in song (including `nooise 1`) open with both on the sixteenth grid and the same hits as before; confirm by ear that nothing moved, then edit a step. |
| Downbeat reset | "Need a way to reset the down beat cleanly"; when the felt bar drifts from the grid, re-anchor rather than change the song. Implemented as `d` / `/downbeat`, quantized so the next beat line becomes beat one and the beat phase never moves ([Performance](../PERFORMANCE.md#downbeat-reset)). | Tap `d` just before a felt beat one during a set with Kick and a queued state; confirm the pulse never stumbles and the chord rings on. |

## Named songs and transition previews

November 3 follow-up idea, proposed for a later slice: `nooise moss` resolves a
named full song snapshot rather than only a chord progression. With a session
already running, offer to queue that song there; `-y` could accept that offer.
How a running nooise or Baton session is discovered and selected remains open.
Joining a clock, loading a song, and opening another audio engine are distinct
operations and need explicit meanings.

The proposed primary performance path is `/songs`: browse saved snapshots,
preview the destination, then queue it at a musical boundary. A preview shows
current and authored tempo, the required change, key, and compact before/after
bars for important controls. Nearby-song browsing can start with tempo/key and
authored tags. A one-action recall moves the controls; manually matching their
positions is optional. A later transition control could perform a continuous
move while discrete changes wait for their musical boundary.

Muted preparation, tempo matching, tempo preservation, gradual morphing, and
headphone audition remain separate design choices. Muting an instance alone
does not provide headphone cueing. Baton clock/session ownership is unresolved.
The current numbered state-bank batch (slots 1-9) is the foundation, not an implementation
of this library, preview, discovery, or multi-engine design.

## Rehearsal checkpoints

| Target | Work to bring | Performance gate |
| --- | --- | --- |
| Nov 2–4 | Baseline recording, three state snapshots, current task audit | Play 10 minutes; identify the three largest interruptions and choose one piece for the recall spike |
| Nov 5–8 | In-process state bank/recall and q tap/hold | Play 15–20 minutes with states 1/2/3/return, one song transition, and hands-free breakdowns; exercise cancel and recovery |
| Nov 9–12 | Chosen melodic slice, any required harmony fixes, longer phrase only if it earns its place | Play the draft set end to end; decide the final feature scope and song order |
| Nov 13–16 | Rehearsal fixes, balanced patches, complete set material and startup path | Full-duration recorded set on the intended rig, including setup, ending, and a deliberate wrong-state recovery |
| Nov 17–18 | Frozen candidate and matching saved material | Two full-duration passes with no blocking failure; verify restart/reload and backup material |
| Nov 19–20 | Only blocking fixes and regression rehearsal | Pack and check the rig; verify the exact show binary and material together |
| Nov 22 | Rehearsed build and set | Soundcheck routing, levels, first transition, and ending |

If recall slips, keep the existing melodic voices for the show. If a new feature
cannot be rehearsed by November 16, defer it. Any later blocking fix requires a
fresh full-set pass. Preserve the last successful build and its matching codes;
newer builds may intentionally refuse older codes.

For each rehearsal, record the build commit, set revision, terminal, audio route,
and recording. Play through mistakes unless continuing is impossible. Afterwards,
timestamp what happened, the intended musical action, the workaround, and whether
it blocked the set or merely interrupted flow. Choose at most three fixes for the
next batch. Re-test the failed moment, then play another uninterrupted set.

Show acceptance includes startup from a fresh launch, correct output routing,
usable level balance, no audible dropouts/clipping, planned transitions, recovery
from a wrong state, a deliberate ending, and a matching backup binary/material
pair. Automated gates support this evidence; they do not replace listening.

## Current work to reconcile

Inspected November 2 on `v3` at `aa47af3`. `stint list` reports 0007 active and
0008–0017 proposed. These statuses are task records, not proof of missing code.

| Work | Evidence and show disposition |
| --- | --- |
| 0007 shared 16-step sequencer | Candidate `8573732` exists on `feature/stint-0007-unify-lead-tonal-and-chords-around`, outside V3's ancestry. Task records automated checks and pending audition. Audit against V3's accepted Rest/Slur/Sound semantics before integrating its Tie-based work. It does not deliver Arp consolidation or longer pattern chains. |
| 0008 stdin song authoring | Explicitly excludes loading into a running instance. Useful authoring convenience; live state recall needs separate scope. |
| 0009 explicit home key/mode | Check Custom songs before writing show melodies; wrong scale-following would block prepared material. Verify current behavior before adopting the older task assumptions. |
| 0010 level curves; 0011 sound design | Prioritize audible balance, clipping, and broken automation found in the set. Defer the broad sound redesign unless a specific sound is unusable. |
| 0012 temporary effects | Reconcile against current q/w/e/r gestures. The task still discusses Bloom/Thin/Lift; new q tap/hold needs explicit coverage. |
| 0013 chord rework | Overlaps 0007 and ADR 0002. Separate already-landed gates/copy-paste from remaining lane/voicing work and show-blocking authoring bugs. |
| 0014 UI consistency; 0015 runtime | Prioritize unreadable performance state, device-change problems, startup, and recovery. Namespace changes and broad UI work can wait. |
| 0016 probabilistic harmony; 0017 knob package extraction | Defer from the show path unless rehearsal reveals a concrete dependency. |

`STATE.md` now tracks the integrated V3 performance candidate; `ROADMAP.md` is
absent here. The table above remains the November 2 historical stint audit. ADR 0002's detailed status lists
landed gate/copy-paste work and a remaining lane/voicing epic, while its header
still says Stages 2–3 pending. Reconcile those records during the integration audit.
Create focused stints after settling the first recall and melodic spike scopes;
do not treat completion of every old task as a prerequisite for the show.

## Ownership and verification

This document owns the November show plan, pending q interaction request, and
November 3 playing notes on Perc, Bass, reverb, freeze/smear, and Vamp visibility.
[North Star](../NORTH_STAR.md) owns musical state and keyboard priorities;
[Performance](../PERFORMANCE.md) owns shipped gestures;
[ADR 0002](../adr/0002-v3-chord-model.md) owns accepted chord semantics.
The [October intake](2026-10-04-stabilization-intake.md) supplies related proposals.
Microphone exploration stays separate until explicitly included in the show rig.

Runtime slices follow their source DOX chains, build/test/fmt/Clippy gates, input
replay, state round-trips, and rendered-audio checks. Record hands-on acceptance
separately. The current automated and live-smoke evidence is recorded in `STATE.md`; the full rehearsal is still a human acceptance gate.
