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
- `f` opens the **LFO** submenu (amount / interval / offset / shape).
- `v` opens the **macro** submenu (macro none/1-4, bipolar amount) on any
  regular control.
- The **MACROS** tab (last tab) holds four bare sliders. On a macro slider,
  `f` adds an LFO and `e` adds a one-shot envelope; `e` is refused elsewhere,
  and `v` is refused on macro rows.
- `f`/`e`/`v` toggle their editor open and closed; settings are kept either
  way. `x` removes: with an editor open it deletes that route, on a bare
  control it strips every modulator at once. `Esc` also closes-and-keeps.
- `T` flips the selected time field between beats and ms (per field, not
  global); display and numeric entry convert at the current BPM.
- `Enter` on a cross-tab row (Master voice levels) expands into that voice's
  tab.
- `r` re-rolls the seed of a random-shape LFO.

## What changed this round

1. **Macros.** A macro is its own modulation source: effective = base + LFO +
   amount x macro value, summed and clamped once. Macro sliders are ordinary
   registry controls, so the engine applies automation in two passes (macros
   first) and targets follow a macro that is itself LFO- or envelope-driven.
   A closed assignment shows a compact amber chip line under its control.
2. **Envelope demoted.** The AD sweep + trigger machinery (every-N / on-kick /
   once) survives only on macro sliders, where slow blooms actually made
   sense. Regular controls no longer take `e`.
3. **Markers.** The bright diamond is the effective (summed) value; each
   contributing source draws a dim ghost diamond at base+that-source-alone:
   pink = LFO, green = envelope, amber = macro.
4. **Baseline field behaviour.** Discrete fields (shape, trigger, macro
   target) clamp at their ends instead of wrapping; all submenu rows render
   through one shared field-row component.
5. **Resolution.** Every 0.25-beat grid halved to 0.125 (32nd notes), floors
   included.
6. **Persistence.** Song-code automation payload v3 carries LFO seeds, macro
   routes, and envelope routes. v2 codes still decode.
7. **Louder chords.** Pad voice output gain 0.58 → 0.72.

## Feedback questions

1. Does macro-as-own-source feel right, or did you expect the macro to scale
   the LFO amount instead?
2. Is the amber chip line under an assigned control enough visibility, or do
   macro assignments need a dedicated overview (e.g. on the MACROS tab)?
3. Ghost diamonds: clarifying or cluttering once two sources overlap?
4. Envelopes now only shape macros. Do you miss them on regular controls,
   or is routing a control through a macro the better gesture anyway?
5. Would a `macro` row inside the f-submenu (macro scales the LFO's amount)
   earn its place next to macro-as-own-source, or is one mechanism enough?
