# Experiment: LFO shapes + lightweight macros

Branch `exp-envelopes-mod`. Round two: the per-control envelope submenu from
round one is gone — envelopes now live only on macro sliders. What remains is
the LFO **shape** parameter (the round-one winner) plus a lightweight macro
system and a batch of ergonomics fixes.

Everything the UI draws still comes from the same `modulated_control_value_full`
path the engine hears. All new routes start audible-neutral (amount 0).

## How to try it

`cargo run`, then:

- Arrow/`jk` select a control, `h/l` adjust, type a number + `Enter` to set.
- `f` opens the **LFO** submenu (amount / interval / offset / shape). On
  amount, interval, or offset, `v` stacks a macro onto that specific field —
  nothing shows until you press it, and it's pruned back out on close if you
  leave it neutral, same as every other route. `x` on the expanded rows
  removes just that stacked macro; `x` elsewhere still removes the whole
  route. Macro sliders' own LFOs never take a stacked macro.
- `v` opens the **macro** submenu on any regular control: four independent
  bipolar amount rows, one per macro slider. There's no target picker — set
  any subset of the four directly and a control can ride several at once.
- The **MACROS** tab (last tab) holds four bare sliders. On a macro slider,
  `f` adds an LFO and `e` adds a one-shot envelope; `e` is refused elsewhere,
  and `v` is refused on macro rows.
- `f`/`e`/`v` toggle their editor open and closed; settings are kept either
  way. `x` removes: with an editor open it deletes that route, on a bare
  control it strips every modulator at once. `Esc` also closes-and-keeps, one
  level at a time — it never quits the app, even at the root.
- `T` flips the selected time field between beats and ms (per field, not
  global). Flipping to ms keeps the exact equivalent and then h/l moves on a
  10 ms grid (typed ms is exact too); flipping back to beats rounds onto the
  beat grid.
- `Enter` on a cross-tab row (Master voice levels) expands into that voice's
  tab.
- `r` re-rolls the seed of a random-shape LFO.

## What changed this round

1. **Macros.** A macro assignment holds one bipolar amount per macro slider,
   not a single target — effective = base + LFO + sum(amount_i x macro_i
   value), summed and clamped once. A control can ride several macros
   simultaneously, each set and adjusted independently. Macro sliders are
   ordinary registry controls, so the engine applies automation in two passes
   (macros first) and targets follow macros that are themselves LFO- or
   envelope-driven. A closed assignment shows a compact amber chip line
   listing every non-zero slot under its control.
2. **Envelope demoted.** The AD sweep + trigger machinery (every-N / on-kick /
   once) survives only on macro sliders, where slow blooms actually made
   sense. Regular controls no longer take `e`.
3. **Markers.** The bright diamond is the effective (summed) value; each
   contributing source draws a dim ghost diamond at base+that-source-alone:
   pink = LFO, green = envelope, amber = macro. While any editor is open on a
   control, a faint band also shades the full reach of every active source —
   how far the value could swing, not just where it sits this instant.
4. **Baseline field behaviour.** Discrete fields (shape, trigger) clamp at
   their ends instead of wrapping; all submenu rows render through one
   shared field-row component.
5. **Resolution, centralized.** Every interval- and offset-like field (voice
   intervals, tonal rate/cycle, LFO interval, LFO offset, and all per-voice
   offsets) shares one beat-grid rule: 0.125 survives as a reachable floor
   (or a field's own lower minimum, e.g. offset's true 0 = no shift), then
   locks to sixteenths (0.25 grid) above it.
6. **Persistence.** Song-code automation payload v5 carries LFO seeds, macro
   routes (now one amount per slider), envelope routes, and field macros.
   v2/v3/v4 codes still decode, folding a legacy single-target macro into
   just that one slot of the new representation.
7. **Louder chords.** Pad voice output gain 0.58 → 0.72.
8. **Macros stack onto any LFO field.** `v` on the amount, interval, or
   offset row of an open LFO editor stacks a macro onto that one field —
   off by default, created only on that keypress, pruned back out if left
   neutral. Never on the field itself unless you opened it. Macro sliders'
   own LFOs are excluded (no macro chasing a macro). Like the top-level
   macro submenu, a field macro is four independent amount rows, not a
   single target.
9. **Reach shadow.** While a control's editor is open, its bar shades the
   full range every active source could push it to, distinct from the ghost
   diamonds (which show only the current instant).
10. **Esc never quits.** Esc only ever drills out one level (a nested field
    macro, then the modulator editor, then nothing) — only `q` or Ctrl+C
    exit the app now, so backing out of a deep edit can't accidentally kill
    the process. `v` on a nested field-macro row closes just that row the
    same way, instead of hijacking the parent LFO editor into swapping to a
    top-level Macro editor.

## Feedback questions

1. Is the amber chip line under an assigned control enough visibility, or do
   macro assignments need a dedicated overview (e.g. on the MACROS tab)?
2. Ghost diamonds plus the new reach shadow: clarifying or too much ink once
   two or three sources overlap?
3. Envelopes now only shape macros. Do you miss them on regular controls,
   or is routing a control through a macro the better gesture anyway?
4. Now that any LFO field can take a stacked macro, does macro-as-own-source
   still earn its place, or would you rather it collapse into always going
   through the per-field path?
