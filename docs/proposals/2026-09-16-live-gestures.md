# Live gestures from normal browsing

Status: Implemented in the `temp-live-gestures-design` worktree; awaiting
hands-on approval. Bindings, recipes, and timings below describe this first
playable version. Musical defaults remain subject to listening.

## Musical intent

Hold a key to swell into a temporary effect. Release it to let the song return.
Keep navigating and adjusting controls throughout.

For example: submerge the drums, bloom the pad into a larger space, then bring
the drums back while the pad's cloud fades. The existing arrangement gains a
build, a contrast, and a return through a few live gestures.

The user does not use Deck and wants these gestures available while browsing.
The design follows the arrow-key and Tab floor in the [North Star](../NORTH_STAR.md).
The gestures provide expression and temporary contrast; their defaults must
already sound balanced without corrective mixing.

## First playable version

Four gesture keys work directly in Browsing, including its ordinary detail
drills. Press starts a smooth rise; holding continues it to an authored peak;
release begins the return. Different gestures can overlap.

Unmodified bindings are `z`, `c`, `v`, and `b`. Their left-hand placement
leaves the right hand free for arrows.

| Key | Gesture | Increasing amount | Release character |
| --- | --- | --- | --- |
| `z` | Bloom | Send more sound into a broad reverb cloud; maintain a clear dry anchor | Stop feeding the cloud smoothly; let its tail finish |
| `c` | Submerge | Sweep a low-pass down until the layer feels distant | Open back toward the current sound |
| `v` | Echo | Send the phrase into repeats, with bounded feedback rising at deeper amounts | Stop new input to the repeats; let existing echoes decay |
| `b` | Thin | Reduce the target's contribution to make room around other layers | Bring the target back smoothly |

On Master, Thin withdraws the whole mix. On a voice page, it withdraws that
voice. It does not choose a hidden subset of instruments.

The first version uses live timing in seconds. Beat quantization, scheduled
starts, gesture recording, brake/depth/speed modifiers, and transfers between
named states are later experiments. Fray and Flutter remain possible future
gestures. Their addition should follow listening to the first four.

## Feel and timing

Each gesture has one continuous amount from 0 to 1. Its recipe maps that
amount into a musical range; 1 means the authored peak, not every processor
parameter at maximum.

Initial audition values:

| Gesture | Rise from zero to peak | Return from peak | Peak intention |
| --- | --- | --- | --- |
| Bloom | 1.5 s | 0.8 s send return, then natural tail | Spacious, with audible dry detail |
| Submerge | 1.2 s | 0.45 s | Clearly muffled, without a resonant whistle |
| Echo | 0.7 s | 0.15 s send return, then natural tail | A distinct phrase throw with decaying repeats |
| Thin | 1.0 s | 0.4 s | Roughly 12 dB less target contribution |

These values are listening hypotheses. Short touches must produce useful
accents. Begin responding immediately and use a gentle, continuous rise;
avoid a long easing dead zone that makes short taps feel ineffective.
Release from a partial amount must also feel responsive.

- Envelope progress follows audio elapsed time. OS autorepeat and UI frame
  rate never determine the amount.
- Releasing halfway returns from that exact amount.
- Pressing again during return reverses smoothly from the current amount.
- Repeating a held key neither restarts the rise nor adds another instance.
- Returning an effect preserves ongoing notes and the musical clock.
- At rest, with gesture tails drained, processing is an exact dry bypass.

## Targeting and navigation

The page at initial Press supplies the target: its voice, or the full mix on
Master. Fix that target for the duration of the press. Moving the selection,
entering a detail drill, or changing tabs never moves a held effect.

Hold Bloom on Pads, Tab to Perc, then hold Submerge: both continue on their
own targets. The active display names those targets even when their pages
are no longer visible.

There is at most one physically held instance per gesture key. A new press
on a different page starts a gesture on that page; any old return or tail
finishes on its original target. Returning instances need stable identities
so a late release cannot stop a newer press.

Arrows, `hjkl`, Tab, Shift+Tab, mute, save, and ordinary control edits keep
their existing meaning. Gesture keys do not become text shortcuts in the
palette, numeric entry, automation editor, or Lead play.

Opening one of those keyboard-owning modes begins the return of held
gestures. Audio tails can finish while the new mode owns input. This avoids
trapping a hold behind another keyboard owner. A physical key held across
that transition must be released before it can trigger again in Browsing.

In Browsing, Escape releases held gestures first and preserves navigation;
with only returns or tails remaining, it follows the existing drill/back
behavior. ADR 0001 records this cancellation contract. In other modes,
Escape keeps that mode's meaning.
Reported focus loss and orderly shutdown also release held gestures.

Only unmodified gesture presses start an effect. Ctrl+C remains quit;
Ctrl+B in the palette remains staged commit. A matching physical key release
still ends its gesture if modifiers changed while it was held.

## Temporary processing over a moving song

Controls, automation, progression, and auto-morph continue underneath a
gesture. Starting a gesture alone does not exit auto. Ordinary edits keep
their existing auto-exit behavior.

The gesture amount composes with the current audio and controls. Releasing
never restores an old control snapshot. An edit made during a swell remains
after the swell ends.

Reuse the shared Filter, Reverb, and Delay implementations and processing
path. Thin uses a smoothed gain multiplier. A gesture must work even if the
user has no matching module loaded or all eight module slots are occupied.
It must not insert, overwrite, reorder, or borrow a user's slot.

Routing feeds a bounded temporary stage from the target's existing
module-chain output. Thin and Submerge shape its direct signal and the input
sent to Bloom/Echo. Bloom and Echo are parallel returns, so they cannot feed
back into each other. Released returns continue without fresh input. Voice
gestures feed the normal Master chain; Master gestures remain inside the
engine's final output protection.

Use shared module execution for that temporary stage, with separate stable
state identities. Do not create gesture-specific copies of the effects or a
second effects registry. Processor storage must be prepared off the audio
callback, bounded, and tested under overlapping voices and Master.

A full slot bank is a required audition case. Each target has fixed processor
storage, independent of user slots. Bloom and Echo tails have eight- and
six-second budgets after their sends return to zero, followed by a short
smooth fade and incremental buffer clearing. Resting processors stop running;
rapid presses and target changes never grow storage.

## Input capability and feedback

Use the runtime's negotiated hold support. Full-capability terminals provide
the hold/release experience. On terminals without trustworthy releases, the
first version leaves gesture keys inactive and explains that hold gestures
require key-release support. Ordinary browsing remains available. Never
infer release from a timeout or convert ambiguous repeats into toggles.

The idle footer can show `z bloom  c submerge  v echo  b thin`. Active feedback
shows the target, amount, and direction, for example
`Pads Bloom 42% rising`. A released return can read `Pads Bloom tail`.
The existing view model renders this feedback without opening a dashboard
or moving the selected row. Keep cancel/help visible at the minimum supported
terminal width; shorten optional idle hints first.

## Saving during a gesture

The project requires song codes to preserve user-audible session state.
Temporary gestures still fall under [commandment 4](../NORTH_STAR.md).

Persist only active gesture recipe identity, target, current amount, and
held/returning phase. The fixed recipe rates reconstruct remaining timing.
Inactive gestures add no payload. Reverb/delay buffers and completed-envelope
tails remain excluded audio state.

A saved rising or held gesture resumes its audible trajectory toward its
peak, with a visible restored-hold marker. Physical keyboard ownership is
never serialized. Pressing that gesture key claims the restored instance on
its saved target; releasing returns it. Escape returns all restored holds.
A saved returning gesture continues its return automatically.

Normal help documents restored holds. Save/reload remains part of hands-on
acceptance; never discard active amounts or bake them into base controls.

## Replacing Deck

This worktree replaces Deck: its `p` entry, Deck-only selector/action handling,
help, fixtures, and documentation are retired together. `p` is unassigned.

Sequence on Space is outside this replacement's first scope. Its shared
instrument/action vocabulary must survive wherever Sequence still uses it.
Lead play also keeps its existing ownership. Removing either is a separate
product choice.

The [performance reference](../PERFORMANCE.md), source DOX rail, help, and
[ADR 0001](../adr/0001-unidirectional-interaction-architecture.md) describe
the implementation in this worktree.

## Verification before delivery

1. Audition short touches, long holds, partial releases, and catching a
   falling gesture. Listen for a useful range and absence of clicks.
2. Hold on Pads, navigate to Perc, and add a different gesture. Confirm
   target labels, audible separation, and uninterrupted arrow/Tab editing.
3. Keep automation or auto-morph running through a gesture. Make an ordinary
   edit during another. Verify return follows the current song and preserves
   the edit.
4. Replay Press/Repeat/Release bursts, Escape, mode transitions, modifier
   changes, and reported focus loss through the production input path. Check
   release ownership, no stuck effects, and the existing 50 ms frame bound.
5. Verify unavailable-release behavior and that typing in other modes never
   triggers gestures. Preserve exact save, quit, and palette bindings.
6. Render audible Bloom and Echo tails after release, a layered gesture
   overlap, and a zero-gesture parity case. Verify stable feedback, bounded
   output, and bounded storage under repeated target changes and full slots.
7. Save/reload rising, held, overlapping, and returning gestures. Compare
   resumed envelopes and base controls; exclude expected audio-buffer loss.
   Measure song-code growth and prove inactive gestures add nothing.
8. Run the repo's build, tests, fmt, and clippy checks for the eventual code
   change. Present a runnable worktree for hands-on approval before a PR.

Begin the hands-on audition with Submerge to hear the hold envelope and check
target ownership, then compare the tail-bearing recipes and overlaps.
