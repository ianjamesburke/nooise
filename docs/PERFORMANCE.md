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

The footer names the target and amount: `↑` means rising, `↓` returning,
and `R` marks a restored hold loaded from a song code.

These are temporary effects over the playing song. User module slots remain
intact, automation and auto-morph continue, and edits made during a gesture
survive its return.

Escape releases held gestures before backing out of a browsing drill.
Opening a keyboard-owning editor or Lead play releases gestures; their tails
can finish in the background. A key held through that transition must be
released before it can start another gesture. Reported focus loss and shutdown
also release held gestures.

Hold gestures require negotiated key-release support. Without it, the footer
explains the requirement and gesture keys remain inactive. The runtime never
guesses release from a timeout or keyboard repeat.

A saved song carries active gesture amounts and envelope direction, with no
audio buffers. Loaded holds resume on their saved targets. Press the matching
gesture key to claim a loaded hold and release it, or use Escape to release
all loaded holds.

## Sequence

Space enters the one-shot Sequence owner. Repeated entry is inert while
choosing or performing. `a/s/d/f` select Pads/Bass/Kick/Perc and open the
corresponding page. `h/l` shorten/lengthen, `j/k` quieten/louden, and `u/i`
make the selected instrument sparser/denser.

Each action applies once on Press. With releases, Sequence consumes subsequent
Repeat events until the matching action Release returns to Browse. Unrelated
releases are inert. Selector Press selects and marks the instrument held;
its matching Release clears the hold. Selecting another instrument first
releases the old one.

Without releases, an action enters a visible completed stage. Autorepeat is
inert there; Space rearms Sequence and Escape returns to Browse.

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
6. Verify Space, `d`, `k` still makes one louder Kick edit and completes
   Sequence according to the terminal's release capability.

Acceptance requires rendered-audio checks as well as input replay and footer
checks. The minimum supported frame remains 46x10.
