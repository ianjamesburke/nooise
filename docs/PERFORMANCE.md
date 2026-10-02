# Live performance

## Hub navigation

The app opens on Master, the hub. Its first rows list the layers in order
(Pads, Perc, Bass, Kick, Tonal, Clap, Arp, Lead). Each row shows the layer's
level and carries `›` to show that Enter opens the layer.
Master's own rows follow. On a layer row, `h`/`l` adjust its level, `m`
mutes that layer, and Enter goes into it; Shift+M mutes Master from any row.

Tab and Shift+Tab step through every layer and then the hub, in Master's
old last place: Pads … Lead, hub, then Pads again. From the hub, Tab enters
Pads and Shift+Tab enters Lead. The hub stays in the cycle because of the
arrow-and-Tab floor (`docs/NORTH_STAR.md`, ADR 0001 invariant 9): a player
who knows only arrows and Tab must still reach every page, and with Esc as
the only way up, Master's own rows (BPM, Tone, Drive) would be lost to them
after leaving the hub. Esc backs out one level at a time: the innermost drill
first, then from the layer's root to the hub with the cursor on that
layer's row. A held gesture is released by the first Esc instead, as
below. Each layer remembers the row it was left on for the session, however
it is re-entered. Esc on the hub does nothing.

The line above the rows is a breadcrumb: `Master` on the hub, `Master ›
Pads` in a layer, `Master › Pads › Progression` in a drill, `Master › Pads
› Reverb` in a module. A muted page's crumb and hub row read `(M)`, and a
playing chord slot or lane carries `♪`. Its longest paths fit the minimum
frame. Navigation is never saved in a song code.

## Normal browsing gestures

Hold `z` for Bloom, `c` for Submerge, `v` for Echo, or `x` for Lift.
The amount rises while held and returns smoothly on release. Bloom blends the
mix into a bright reverb cloud built from its mids and highs, Submerge darkens
it, Echo throws it into repeats, and Lift thins its low end. Every gesture
lets go within 10 ms of release, wet returns included, so no reverb or echo
rings on. The former Thin
gesture on `b` is retired; a song code carrying one is refused.

Every gesture plays over the whole mix, after Master's module chain, whatever
page is open. Arrows and Tab remain available while a key is down, and
different gestures can overlap. Pressing during a return catches it at its
current amount. Gestures are gain-staged so a full throw never raises the
master: Bloom and Echo trade dry level for their returns instead of stacking
on top of it.

The footer is two rows: a gesture-activity row above a stable exits/mode-help
row, so a held gesture never crowds out `Esc release · ^Q quit` or the row
below it. Idle, the activity row lists each hold's key and name (`z bloom
c submerge  v echo  x lift`); held or returning, it switches to a
bold readout of gesture and amount — `↑` means rising, `↓` returning, and `R`
marks a restored hold loaded from a song code. The row below stays a terse
`BROWSE · ? shortcuts   ^Q quit`; pressing `?` opens the full keyboard-shortcut
map (`InteractionMode::Help`), a static overlay covering the breadcrumb/control area
that leaves both footer rows visible beneath it. Esc closes it. Shift+/ is
matched two ways, since terminals disagree on how they report it: the
shifted glyph `?` alone (most terminals — unlike a shifted letter, no SHIFT
modifier accompanies shifted punctuation), or the base key `/` with an
explicit SHIFT modifier (the keyboard-enhancement protocol's report-base-key-
plus-modifier style). A plain, unshifted `/` still opens the palette.

These are temporary effects over the playing song. User module slots remain
intact, automation and auto-morph continue, and edits made during a gesture
survive its return.

Escape releases held gestures before backing out of a browsing drill.
Opening a keyboard-owning editor or Lead play releases gestures; Echo's
repeats can finish in the background. A key held through that transition must be
released before it can start another gesture. Reported focus loss and shutdown
also release held gestures.

Hold gestures require negotiated key-release support. Without it, the
activity row stays blank and gesture keys remain inactive — the shortcut map
(`?`) still lists them, since they are a capability gap, not a hidden feature.
The runtime never guesses release from a timeout or keyboard repeat.

A saved song carries active gesture amounts and envelope direction, with no
audio buffers. Loaded holds resume where they were. A code from a build with
per-layer gestures that carries a gesture aimed at a voice is refused as a
retired per-layer gesture. Press the matching
gesture key to claim a loaded hold and release it, or use Escape to release
all loaded holds.

## Clock stop

Shift+P stops the clock and Shift+P again starts it. It is shifted so a stray
keystroke cannot end the song; plain `p` does nothing. It works from browsing
and from an open LFO or envelope editor, on every terminal: a Press edge, no
key-release support needed, and autorepeat never flutters it.

Stopped, the beat holds where it was. No new note, hit, chord, arp, or lane
step fires, and sustaining Pad chords release into their tails. The audio
path keeps running, so note releases, reverb, and delay tails ring out
naturally. Automation and the auto morph wait on the held beat, and gestures
still play into the tails. Played Lead keys still sound. A palette edit
staged for the next bar waits for the clock to start.

Starting returns to bar 1. Every voice grid and phrase position restarts,
and Pads voice the first chord in the selected chord window. Captured knob
loops restart from their first sample, and auto morph restarts its first
endpoint. Pending palette edits land on the first new bar. Live capture
history clears; saved curves and Tonal's evolved notes remain. Existing
release envelopes and effect tails keep ringing. MIDI output sends Start.

While stopped the activity row leads with `■ STOPPED`, whoever owns the
keyboard. Stopping is not an edit: it leaves auto running and the MRU
untouched, like mute.

A song code never carries the stopped state. A stopped song is silence,
not a state worth sharing, so a loaded code always plays.

## Jump

Space is a leader key. A layer key opens that layer at the row last active
there and hands the keyboard straight back to browsing, where `h`/`l`
adjust it and `j`/`k` move as always. `Space j` and `Space k` put the
cursor on Level or Filter for the page already open. The leader changes
nothing by itself: it is an address, not an edit.

The layer keys read down the hub's layer rows, so their positions mirror
the layers on screen: `a` Pads, `s` Perc, `d` Bass, `f` Kick, `q` Tonal,
`w` Clap, `e` Arp, and `r` the hub itself. `j` is volume and `k` is filter.

Lead is the one page no selector key names. It already owns `i` for play
entry, and the shorthand below reaches it from its own page.

Skipping the layer key aims at the page you are already on, so `Space k` is
the filter on whatever is in front of you. That shorthand is the only way
into Lead, and stays the quickest route to the page already in front of
you.

`Space m` arms the visible layer's mute for the next bar. Its breadcrumb and
hub row show a small `m` until the boundary, where it becomes `(M)`. The audio
gate completes its click-free ramp before that downbeat, so its first onset is
already muted or audible. Repeating
`Space m` before that bar cancels the same planned mute. The action is part of
the saved session state, rebased to the next song's beat zero. Escape leaves a
pending leader; Space while it is pending is inert.

`Space 1`, `Space 2`, and `Space 4` grab Motion from the selected knob: the
last one, two, or four bars. A grab belongs to its nearest bar downbeat. When
that downbeat is ahead, the current value holds until it arrives and the loop
starts there. When it has passed, the loop joins immediately at the phase it
would already have reached from that downbeat. The palette keeps the same
Motion Grab choices.

Volume is the layer's own Level row. Filter is the shared filter module's
Cutoff, never its Amount: Amount is a detail-only wet/dry mix an added filter
pins fully wet, so Cutoff is the single knob the leader lands on and `h`/`l` sweep.
Bass, Kick, Perc and Clap ship with a filter in slot 1, so `k` lands on the cutoff
already in play. Pads gets one added into its first free slot at a
transparent 20 kHz cutoff, so arriving is silent and turning the cutoff down
is the first audible move. A layer whose chain is full says so and stays
put.

The leader only ever moves a cursor except for `Space m`, which is a press-only
boundary action and needs no key-release support. It renders a compact centered
Jump menu: layers form a two-column list, current-page Volume, Filter, and
Mute share a row, and `Esc Cancel` has the bottom row. The footer remains
visible.

Lead play retains its own `i` entry and existing bindings.

## Audio smoke

1. Run `cargo run` in this worktree, in a terminal reporting key releases.
   On any page, hold `c`: expect the whole mix to darken smoothly and a
   Submerge amount in the footer. Release: expect the original clarity to
   return.
2. Hold `z`, move with arrows and Tab, and release. The Bloom must keep
   playing over the mix while navigation continues. Overlap a different
   gesture and confirm both are named. Neither should make the mix louder.
3. Release `z` after a long hold: the wash must be gone at once (10 ms)
   with no click. Try short and long `v` presses, then `x`. Echoes stop on
   release; Lift should clear its low end while held and restore it at once
   on release. `b` does nothing in Browse.
4. Open the palette during a held gesture, return to Browse, and keep the
   physical key down. It must stay released until a fresh press after key-up.
   Escape must also release held or loaded gestures.
5. Save during a swell and load that code. Its amount should resume; the
   matching gesture key or Escape must let it return.
6. Move to a non-Level Bass row, then press Space, `d`: it returns to that
   remembered Bass row in Browse with nothing changed, and `h`/`j`/`k`/`l`
   work at once. From Bass press Space, `j`: the cursor reaches Bass Level.
   Press Space, `a`, then Space, `k`: a filter appears on the Pads chain,
   inaudible, cursor on its Cutoff row, and `h` sweeps it down. Repeat on
   Bass and confirm `k` reaches the filter already in slot 1 without
   resetting its cutoff or adding a second one. Press Space, `r`, then
   Space, `j` and confirm it reaches Master Level. Repeat the direct layer
   jump for Tonal, Clap, and Arp. On Lead, confirm Space, `j` still reaches
   its Level.
7. On Kick, press Space, `m`: its hub row and breadcrumb show `m` while the
   sound continues. On the next bar it reads `(M)` and the Kick drops out
   click-free with no first hit. Repeat Space, `m` before the boundary to cancel. Save while it
   is armed, load the code, and confirm it still lands at the loaded song's
   next bar.
8. Press `?` from Browsing: the shortcut map should open over the
   breadcrumb and control rows, leaving both footer rows visible beneath
   it. Esc returns to Browsing.
9. On Pads with Reverb loaded and Kick audible, press Shift+P: no new kick or
   chord arrives, the chord and reverb fade out naturally rather than
   cutting, and the activity row shows `■ STOPPED`. Hold `z` during the
   tail and hear it bloom. Press Shift+P again: the grid restarts at bar 1 and
   the first chord in the selected window swells back in.
10. Launch: the hub shows `Master` above one `›` row per layer. Press `m` on
   Kick: its row and nothing else reads `(M)`. Enter Bass, move down two
   rows, Tab to Kick and Esc: the cursor is on the hub's Kick row. Enter
   Bass again: the cursor is back where it was left. Shift+Tab to Lead, then
   Tab: the hub is back, with BPM reachable by arrows.

Acceptance requires rendered-audio checks as well as input replay and footer
checks. The minimum supported frame is 46x11 — the two-row footer costs the
control-row area one line versus the former 46x10.
