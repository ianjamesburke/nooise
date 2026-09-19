# Live performance

## Normal browsing gestures

Hold `z` for Bloom, `c` for Submerge, `v` for Echo, `b` for Thin, or `x` for
Lift.
The amount rises while held and returns smoothly on release. Bloom adds a
reverb cloud, Submerge darkens the sound, Echo throws the phrase into repeats,
Thin lowers its contribution, and Lift sweeps its low end away. The gesture
input returns in 50 ms; reverb and echo tails finish after release.

The current page supplies the target. Master affects the whole mix. The target
stays fixed while the key is down, so arrows and Tab remain available and
different gestures can overlap on different layers. Pressing during a return
on the same layer catches it at its current amount.

The footer is two rows: a gesture-activity row above a stable exits/mode-help
row, so a held gesture never crowds out `Esc release · ^Q quit` or the row
below it. Idle, the activity row lists each hold's key and name (`z bloom
c submerge  v echo  b thin  x lift`); held or returning, it switches to a
bold readout of target and amount — `↑` means rising, `↓` returning, and `R`
marks a restored hold loaded from a song code. The row below stays a terse
`BROWSE · ? shortcuts   ^Q quit`; pressing `?` opens the full keyboard-shortcut
map (`InteractionMode::Help`), a static overlay covering the tab/control area
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
Opening a keyboard-owning editor or Lead play releases gestures; their tails
can finish in the background. A key held through that transition must be
released before it can start another gesture. Reported focus loss and shutdown
also release held gestures.

Hold gestures require negotiated key-release support. Without it, the
activity row stays blank and gesture keys remain inactive — the shortcut map
(`?`) still lists them, since they are a capability gap, not a hidden feature.
The runtime never guesses release from a timeout or keyboard repeat.

A saved song carries active gesture amounts and envelope direction, with no
audio buffers. Loaded holds resume on their saved targets. Press the matching
gesture key to claim a loaded hold and release it, or use Escape to release
all loaded holds.

## Jump

Space is a leader key. A layer key then a parameter key puts the cursor on
that control and hands the keyboard straight back to browsing, where `h`/`l`
adjust it and `j`/`k` move as always. The leader changes nothing by itself:
it is an address, not an edit.

`a`/`s`/`d`/`f` choose Pads/Bass/Kick/Perc and open that page. `j` is volume
and `k` is filter.

Skipping the layer key aims at the page you are already on, so `Space k` is
the filter on whatever is in front of you. That shorthand is also the only
way into the five layers no selector key names: Tonal, Clap, Arp, Lead and
Master.

A second layer key re-aims a jump that has not completed, so a mistyped
layer costs one key rather than an Escape and a restart. Escape leaves the
leader; Space while it is pending is inert.

Volume is the layer's own Level row. Filter is the shared filter module's
Cutoff, never its Amount: Amount is a detail-only wet/dry mix pinned fully
wet, so Cutoff is the single knob the leader lands on and `h`/`l` sweep.
Bass, Kick and Perc ship with a filter in slot 1, so `k` lands on the cutoff
already in play. Pads gets one added into its first free slot at a
transparent 8 kHz cutoff, so arriving is silent and turning the cutoff down
is the first audible move. A layer whose chain is full says so and stays
put.

The leader only ever moves a cursor, so it needs no key-release support and
behaves identically on every terminal. It renders as a footer line naming
the keys it is waiting for; the page it is aiming at stays on screen beneath
it.

The former Deck entry on `p` is retired. Lead play retains its own `i` entry
and existing bindings.

## Audio smoke

1. Run `cargo run` in this worktree, in a terminal reporting key releases.
   On Pads, hold `c`: expect a smooth darkening and a Submerge amount in the
   footer. Release: expect the original clarity to return.
2. Hold `z`, move with arrows and Tab, and release. The effect must stay on
   Pads while navigation continues. On another audible layer, overlap a
   different gesture and confirm each target is named.
3. On Master, try short and long `v` presses, then `b` and `x`. Echoes should
   finish after release; Thin should smoothly lower and restore the mix; Lift
   should clear its low end while held and restore it quickly on release.
4. Open the palette during a held gesture, return to Browse, and keep the
   physical key down. It must stay released until a fresh press after key-up.
   Escape must also release held or loaded gestures.
5. Save during a swell and load that code. Its amount should resume; the
   matching gesture key or Escape must let it return.
6. Press Space, `s`, `j`: the cursor lands on Bass Level in Browse with
   nothing changed, and `h`/`l` then move it. From that page press Space,
   `j` again: the same row, two keys, no layer key. Press Space, `a`, `k`: a
   filter appears on the Pads chain, inaudible, cursor on its Cutoff row,
   and `h` sweeps it down. Repeat on Bass and confirm `k` reaches the filter
   already in slot 1 without resetting its cutoff or adding a second one.
   On Master, confirm Space, `j` reaches Master Level.
7. Press `?` from Browsing: the shortcut map should open over the tab and
   control rows, leaving both footer rows visible beneath it. Esc returns
   to Browsing.

Acceptance requires rendered-audio checks as well as input replay and footer
checks. The minimum supported frame is 46x11 — the two-row footer costs the
control-row area one line versus the former 46x10.
