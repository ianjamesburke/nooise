# LFO Ergonomics Rework

Date: 2026-07-02
Status: approved
Replaces the interaction model shipped in unmerged stints 0003–0005.

## Problem

- `l` opens the LFO editor, but `hjkl` are arrow keys; `l` must mean "adjust up".
- The LFO "editor" is a footer readout only — amount/interval/offset cannot be edited from the UI at all.
- `LfoRoute` splits depth into `target_depth_ratio` / `effective_depth_ratio`; effective defaults to 0 and nothing ever ramps it, so every LFO is silent.
- The per-slider visual is a static `▁▂▄▆█▆▄▂` character strip: no animation, no relation to real phase, depth, or interval.

## Design

### Keybindings

- `f` on a selected slider toggles its LFO submenu. Opening creates a default route if none exists.
- `f` again or `Esc` closes the submenu. `Esc` with no submenu open still quits.
- `l` becomes "adjust up" (vim-right), symmetric with `h`. `Right` unchanged.
- No hold-detection: terminals cannot reliably report key release; tap-toggle is equivalent muscle memory and repeated presses are harmless.

### Data model

`LfoRoute` becomes:

```rust
struct LfoRoute {
    depth_ratio: f32,          // 0.0..=1.0, "amount"
    cycle_beats: f32,          // "interval", discrete ladder
    phase_offset_cycles: f32,  // 0.0..=1.0, "offset"
    shape: LfoShape,           // Sine only for now
}
```

The `target_depth_ratio` / `effective_depth_ratio` split is deleted. Default route: depth 25%, interval 2 beats, offset 0. Song-code format (stint 0005, unmerged) is rewritten for this shape; no compatibility with the unshipped layout.

### Submenu

When open, three indented rows render under the parent slider and join the normal selection flow (`jk` moves through them, `h`/`l`/arrows adjust, typed value + Enter sets, `H` resets):

- **amount** — 0–100%, 5% steps.
- **interval** — discrete ladder: 1/4, 1/2, 1, 2, 4, 8, 16 beats.
- **offset** — 0–1 cycles, 1/8 steps.

Closing the submenu with amount at 0 deletes the route (song codes never carry dead LFOs). While a submenu is open, selection is confined to the parent slider and its three rows.

### Animated visual

- Engine publishes its beat position each block through `FluidTelemetry` (`AtomicU64` storing `f64::to_bits`, same lock-free store/load pattern as `kick_pulse`).
- The per-slider LFO lane renders a live sine across its width: one full cycle window, phase-locked to the published beat plus route offset. Height scales with amount; the wave scrolls at the true interval rate.
- Color: panel pink/purple palette, per-column brightness peaking at the current phase head so the sweep reads as motion.
- The parent slider bar shows a bright marker at the live modulated value (base ± depth·sin), so the parameter visibly breathes even with the submenu closed.

### Testing

- Key handling: `f` toggle, submenu row navigation/adjust/type-set, route deletion on close at amount 0.
- Automation engine: modulation math against the simplified route (existing stint-0004 tests adapted).
- Song codes: round-trip encode/decode of the new route layout; reject-garbage paths preserved.
- Rendering: lane snapshot at fixed beat values (phase determinism), modulated-value marker position.
