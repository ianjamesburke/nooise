# Experiment: Envelopes and richer modulation

Branch `exp-envelopes-mod`. Theme: every control can already have a sine LFO —
this branch explores what *else* a control can have. It extends the existing
`f`-submenu + animated-lane paradigm rather than replacing it: select a slider,
press a key, tweak an inline submenu, watch a live lane that draws exactly what
the engine plays.

All new routes/fields start **audible-neutral** (amount 0). Everything the UI
draws comes from the same `modulated_control_value_full` path the engine hears.

## How to try it

`cargo run`, then on any tab:

- Arrow/`jk` select a control, `h/l` adjust, type a number + `Enter` to set.
- `f` opens the **LFO** submenu (as before) — now with a **shape** field.
- `e` opens the **envelope** submenu (new) — a one-shot AD sweep.
- Inside a submenu: `jk` move between rows, `h/l` change a field, `H` reset a
  field, type a number + `Enter` to set, `Esc` closes.
- `r` re-rolls the seed of the selected LFO when its shape is random.
- A control can carry **both** an LFO and an envelope at once; the marker and
  audio sum them and clamp to the control's range.

### 1. Modulator SHAPE (LFO submenu)

The LFO submenu gains a `shape` row (last row, so `f`→`l` still nudges amount
first). Cycle with `h/l` through:

- `sine` (unchanged default), `triangle`, `ramp up`, `ramp down`, `square`
  (smoothed), `random drift` (smoothed noise), `sample & hold` (stepped random,
  snapped to the LFO's cycle grid).

The animated lane draws the actual selected shape. Periodic shapes stay
phase-locked (one cycle across the width, bright head at the current phase).
Random shapes instead scroll the **real generated trajectory** right-to-left
(head = now), so what you see is the sequence you hear. Random values are a pure
hash of `(seed, cycle index)` — no RNG state — so offline `render --seed` stays
byte-identical. `r` re-rolls the seed to a new but repeatable pattern.

### 2. One-shot ENVELOPE (`e` submenu)

`e` opens a sibling editor with four rows: `amount` (bipolar −100%..100%),
`attack` (beats), `decay` (beats; `0` = hold at peak), and `trigger`. The
trigger row cycles through `every 1/2/4/8/16/32 beats`, `on kick`, and
`once (macro)` — folding the every-N interval choices and the macro one-shot
into one discrete field. The envelope re-triggers and sweeps the control's
modulation marker just like the LFO diamond, with its own lane (a rising/falling
ramp with a phase head; green = positive amount, amber = negative).

`on kick` reconstructs the kick grid from the live `kick.interval`/`kick.offset`
controls, so it fires with the kick without threading kick-engine state into
automation.

### 3. Slow MACRO envelope

Implemented as the `once (macro)` trigger plus a long `attack` and `decay = 0`
(hold). Example: select Reverb Mix, `e`, set trigger `once (macro)`, amount
`+60%`, attack `256 beats`, decay `hold` — the mix blooms over the next few
minutes and stays. `attack`/`decay` reach 512 beats (~6 min at 82 BPM, ~12 min
at 40 BPM). It is the same envelope system, not a separate one.

### 4. LFO + envelope on the same control

Cheap and kept: `AutomationState` holds independent `routes` (LFO) and
`envelopes` maps. `modulated_control_value_full` sums both contributions and
clamps/snaps once. Add an LFO with `f`, an envelope with `e`, on the same
slider; the marker reflects the sum.

## Tradeoffs / notes

- **Persistence:** LFO shape now survives the song code (still payload v2, new
  shape tags). Envelope routes and random seeds are **not** serialized on this
  branch — export silently skips envelopes, and decoding old codes still works.
  A saved random LFO reloads at the default seed (id-derived), so a manually
  re-rolled pattern does not survive save/load.
- **On-kick approximation:** the envelope's `on kick` trigger follows the kick
  *grid*, not the kick engine's exact latching/pull-earlier behaviour. Close in
  practice, deterministic, and it avoids coupling automation to the voice.
- **Attack/decay granularity:** 0.5-beat steps (numeric entry for large macro
  values). Fine for slow evolution; a fast pluck-tight attack is coarse.

## Feedback questions

1. Is `e` for envelope, sitting right next to `f` for LFO, discoverable? Or
   should the submenu advertise both keys inline?
2. Does putting `shape` as the **last** LFO row (so `f`→`l` still hits amount)
   feel right, or should shape lead the submenu as the headline feature?
3. Do the random lanes (scrolling oscilloscope) read clearly next to the
   phase-locked periodic lanes, or is the mixed metaphor confusing?
4. Is folding interval + macro into one `trigger` field intuitive, or would you
   expect a separate "interval" row and/or a dedicated macro key?
5. Is `r` to re-roll a random LFO seed findable, and do you want reseeded
   patterns to persist in the song code?
