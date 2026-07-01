# Chord progressions (A/B/C/D) for the Chords (Pad) voice

## Problem

`PadEngine` cycles through one fixed 5-chord Hz table (`pad_chord`, `src/fluid.rs:1706-1715`) forever, with no way to pick a different harmonic mood. The user wants four selectable 8-chord progressions of increasing emotional intensity, authored via MIDI note numbers instead of hand-picked Hz values, with instant switching between them.

## Scope

- Replace the single 5-chord Hz table with four 8-chord progressions, authored as MIDI note numbers, converted to Hz at lookup time.
- Add a `progression: f32` field to `PadControls` (stored as `0.0..=3.0`, stepped by `1.0`), displayed as `"A"`/`"B"`/`"C"`/`"D"` on a new "Progression" row in the Chords tab.
- Change `chord_bars` default from `4.0` to `1.0` (min stays `1.0`, max `64.0`, same doubling/halving control) so the default full 8-chord progression plays over 8 bars.
- On the normal chord-advance trigger, cycle `step_index` `0..8` (instead of today's unbounded `chord_index`).
- On changing `progression` (not on every frame — only when the value actually changes), immediately release current pad layers and push a new layer for `(progression, step_index)` at the *current* `step_index`, without waiting for the next scheduled chord-advance trigger.

## Non-scope

- No chromatic/dissonant harmony — all four progressions use only the natural-note pitch classes A/B/C/D/E/F/G (no sharps/flats), so switching never introduces tension beyond added color tones (7ths/9ths) and register spread.
- No change to voice count (still 4 notes/chord), panning, gains, attack, reverb, stereo width, detune, or octave-mix behavior.
- No redesign of other tabs/voices.
- No persistence of `progression` selection beyond the existing controls save/load path (it's just another `f32` field, follows whatever `FluidControls` already does).

## Design

### MIDI note authoring

```rust
fn midi_to_hz(note: i32) -> f32 {
    440.0 * 2f32.powf((note as f32 - 69.0) / 12.0)
}
```

Each chord is a `[i32; 4]` of MIDI note numbers. Four progressions of eight chords:

```rust
const PROGRESSIONS: [[[i32; 4]; 8]; 4] = [
    // A: today's loop + 3 more in the same open/spread style
    [
        [45, 50, 55, 60], // Am open   (A2 D3 G3 C4)
        [45, 52, 55, 62], // Am open   (A2 E3 G3 D4)
        [43, 50, 57, 60], // G open    (G2 D3 A3 C4)
        [47, 52, 55, 62], // B open    (B2 E3 G3 D4)
        [45, 50, 57, 64], // Am open   (A2 D3 A3 E4)
        [48, 55, 60, 64], // C open    (C3 G3 C4 E4)
        [43, 50, 55, 59], // G open    (G2 D3 G3 B3)
        [45, 52, 57, 60], // Am open, resolve (A2 E3 A3 C4)
    ],
    // B: gentle diatonic movement, still plain open voicings
    [
        [45, 50, 57, 60], // Am open   (A2 D3 A3 C4)
        [50, 53, 57, 62], // Dm open   (D3 F3 A3 D4)
        [48, 55, 60, 64], // C open    (C3 G3 C4 E4)
        [43, 50, 55, 59], // G open    (G2 D3 G3 B3)
        [41, 48, 53, 57], // F open    (F2 C3 F3 A3)
        [45, 52, 57, 60], // Am open   (A2 E3 A3 C4)
        [52, 59, 64, 67], // Em open   (E3 B3 E4 G4)
        [45, 50, 57, 60], // Am open, close (A2 D3 A3 C4)
    ],
    // C: adds 7ths for warmth (Am-F-C-G with color)
    [
        [45, 48, 52, 55], // Am7    (A2 C3 E3 G3)
        [41, 45, 48, 52], // Fmaj7  (F2 A2 C3 E3)
        [48, 52, 55, 59], // Cmaj7  (C3 E3 G3 B3)
        [43, 47, 50, 53], // G7     (G2 B2 D3 F3)
        [50, 53, 57, 60], // Dm7    (D3 F3 A3 C4)
        [45, 48, 52, 55], // Am7    (A2 C3 E3 G3)
        [52, 55, 59, 62], // Em7    (E3 G3 B3 D4)
        [41, 45, 48, 52], // Fmaj7  (F2 A2 C3 E3)
    ],
    // D: same harmony as C, voiced wider/higher with 9ths for maximum swell
    [
        [45, 52, 57, 60], // Am, wide      (A2 E3 A3 C4)
        [41, 45, 48, 55], // Fmaj9-flavor  (F2 A2 C3 G3)
        [48, 55, 59, 62], // Cmaj9-flavor  (C3 G3 B3 D4)
        [43, 50, 53, 57], // G9-flavor     (G2 D3 F3 A3)
        [50, 57, 60, 64], // Dm9-flavor    (D3 A3 C4 E4)
        [45, 52, 55, 60], // Am7, wide     (A2 E3 G3 C4)
        [52, 55, 59, 64], // Em open       (E3 G3 B3 E4)
        [45, 52, 57, 60], // Am, wide, resolve (A2 E3 A3 C4)
    ],
];

fn pad_chord(progression: usize, step: usize) -> [f32; 4] {
    PROGRESSIONS[progression % 4][step % 8].map(midi_to_hz)
}
```

Every note across all 32 chords is drawn only from pitch classes A/B/C/D/E/F/G at various octaves — no accidentals, so there is no chromatic dissonance in any progression, including D.

### `PadControls`

```rust
pub(crate) struct PadControls {
    pub level: f32,
    pub chord_bars: f32,     // default changes from 4.0 to 1.0
    pub progression: f32,    // new: 0.0..=3.0, step 1.0, default 0.0 ("A")
    pub reverb_mix: f32,
    pub stereo_width: f32,
    pub detune: f32,
    pub octave_mix: f32,
    pub attack_time: f32,
}
```

### `PadEngine`

```rust
struct PadEngine {
    sample_rate: f32,
    layers: Vec<PadLayer>,
    chord_trigger: GridTrigger,
    step_index: usize,        // renamed from chord_index; now 0..8
    last_progression: usize,  // new: detects progression-selector changes
    reverb: Freeverb,
    depth_lfo: DriftingLfo,
    width_lfo: DriftingLfo,
    air: WhiteNoise,
    rng: StdRng,
    telemetry: Arc<FluidTelemetry>,
}
```

`PadEngine::new` initializes `step_index: 0`, `last_progression: 0`, and the initial layer via `PadLayer::new(0, 0, sample_rate, c.attack_time)` (progression 0, step 0).

`PadEngine::next`:

```rust
fn next(&mut self, c: &PadControls, timing: TimingContext) -> (f32, f32) {
    let progression = (c.progression.round() as usize) % 4;
    let progression_changed = progression != self.last_progression;
    self.last_progression = progression;

    let advance = self.chord_trigger.pop(timing, c.chord_bars * 4.0, 0.0);

    if advance || progression_changed {
        for layer in &mut self.layers {
            layer.release();
        }
        if advance {
            self.step_index = (self.step_index + 1) % 8;
        }
        self.telemetry
            .chord_index
            .store(self.step_index as u64, Ordering::Relaxed);
        if self.layers.len() >= MAX_PAD_LAYERS {
            let remove_count = self.layers.len() + 1 - MAX_PAD_LAYERS;
            self.layers.drain(0..remove_count);
        }
        self.layers.push(PadLayer::new(
            progression,
            self.step_index,
            self.sample_rate,
            c.attack_time,
        ));
    }

    // ...rest of the function (mixing, reverb) is unchanged.
}
```

`progression_changed` fires the same release/push/crossfade path as a normal chord-advance trigger, but does **not** advance `step_index` — it re-renders the *same* chord slot using the new progression's voicing, giving the instant A2-vs-B2-style alternation the user described. If both `advance` and `progression_changed` happen on the same sample (rare edge case: a chord-advance trigger and a progression switch land in the same buffer), the code above advances `step_index` first and always uses the freshest `progression`/`step_index` pair — one layer push, not two.

`PadLayer::new` and `pad_tones` gain a `progression: usize` parameter threaded through to `pad_chord(progression, step)`.

### Terminal UI (`tab_controls`, `apply_delta`, `apply_min`)

New `ControlItem` inserted into `Tab::Chords` right after "Chord Length" (so the row order becomes Level, Chord Length, Progression, Reverb Mix, Stereo Width, Detune, Octave Mix, Attack — shifting Reverb Mix through Attack down by one index from today):

```rust
ControlItem {
    label: "Progression".to_string(),
    value: c.pad.progression,
    min: 0.0,
    max: 3.0,
    display: ["A", "B", "C", "D"][c.pad.progression.round() as usize % 4].to_string(),
}
```

`apply_delta`'s `Tab::Chords` arm gets a new index-2 case (existing indices 2-6 shift to 3-7):

```rust
2 => c.pad.progression = (c.pad.progression + dir).clamp(0.0, 3.0),
```

`apply_min`'s `Tab::Chords` arm gets a matching index-2 case:

```rust
2 => c.pad.progression = 0.0,
```

All existing index-based `apply_delta`/`apply_min`/`tab_controls` tests for `Tab::Chords` (indices 2-6 today) need their indices bumped by 1 to 3-7.

### Testing

- `PadControls::default()` yields `chord_bars == 1.0`, `progression == 0.0` (update `defaults_match_current_mix`).
- `tab_controls(Tab::Chords, ...)` row 2 is `"Progression"` with display `"A"` at default; setting `c.pad.progression = 2.0` yields display `"C"`.
- `apply_delta(Tab::Chords, 2, 1.0, ...)` increments `progression` from `0.0` to `1.0`; clamps at `3.0`; `apply_min` resets to `0.0`.
- `pad_chord(progression, step)` returns the expected Hz values (spot-check a couple of chords per progression against `midi_to_hz` by hand) and wraps both `progression % 4` and `step % 8`.
- `PadEngine`: render a few chord-advance cycles at `chord_bars = 1.0`, assert `step_index` cycles `0..8` and wraps.
- `PadEngine`: render at a fixed `step_index`, then flip `c.progression` (e.g. `0.0 -> 1.0`) between calls to `next()` without any elapsed chord-advance trigger, and assert a new layer was pushed immediately (layer count increases / oldest layer starts releasing) rather than waiting for the next `chord_trigger.pop()` to return true.
- Existing `pad_engine_caps_released_layers` test continues to pass unmodified (progression stays at default `0.0` throughout).

## Out of scope follow-ups

None identified.
