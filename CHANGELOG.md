# Changelog

Newest releases appear first.
## [1.4.0] — 2026-07-14

### Added
- feat: add custom chord progression builder
- feat: add pad chord type character variants
- feat: add bass type character variants
- feat: add arp voice following the pad chord progression
- feat: add attack/release controls to tonal
- feat: add four new chord progressions (two dark modal, two major)
- feat: beats-based chord length entry, eased gain ramps, macro LFO field guard

### Fixed
- fix: rebuild audio stream when the default output device changes
## [1.3.0] — 2026-07-07

### Added
- feat: macro routes become 4 independent amount sliders, drop target picker
- feat: gate macro-on-field behind v (off by default), add reach-shadow marker
- feat: centralize beat grid for offsets too, keeping true zero reachable below the 0.125 floor
- feat: stack a macro onto LFO depth via indented amount rows
- feat: flipped time fields step and type in their display unit, snap on return to beats
- feat: interval grids lock to sixteenths above the 0.125 floor
- feat: x removes automation; same-key tap just toggles the editor
- feat: T flips units per selected field instead of globally
- feat: v double-tap hides a macro assignment; amount row leads the macro submenu
- feat: Enter expands a row into its owning tab; louder chords voice
- feat: song code v3 — persist LFO seeds, macro routes, and envelopes
- feat: effective marker + per-source ghost diamonds on sliders
- feat: T cycles a global beats/ms unit mode
- feat: lightweight macro system — 4 sliders, v-assignment, two-pass automation
- feat: double-tap f/e disables the modulator
- feat: baseline field behaviour — discrete fields clamp, shared field row renderer
- feat: halve all 0.25-beat grids to 0.125 (32nd notes)
- feat: add modulator shapes, envelopes, and combined LFO+envelope routes

### Fixed
- fix: percent entry always means percent, v on Shape no-ops, macro-driven LFO amount survives hide
- fix: Esc never quits, unify one-level-at-a-time editor close

### Performance
- perf: allocation-free audio hot path, opt-level 1 dev builds
- perf: instant-feel input and audio response
## [1.2.3] — 2026-07-05

### Changed
- Revert "Merge branch 'exp-audio-visuals'"
## [1.2.2] — 2026-07-05

### Added
- feat(fluid): move focus-mode hint to discoverability cue, purify focus view
- feat: give hit ripples identity colours and impact cores so they read through the kick
- feat: unify everything into one fluid; chord tones become vibrating center-column nodes
- feat: chord character shapes the pad's flowing waves
- feat: kick wave radiates from a bottom point, pushed upward
- feat: spark brightness tracks each voice's live decay envelope
- feat: crisp surface layer for tonal/perc/clap over the fluid field
- feat: level-gated fluid field with coherent kick wavefront and blended hue
- feat: node-based audio-reactive field visualizer
- feat: publish per-voice telemetry for visualizer

### Fixed
- fix: capture trigger peaks past the level-publish race; anchor kick and tonal placement
## [1.2.1] — 2026-07-04

### Added
- feat: add ambient techno tonal voices
- feat: retune tonal piano variations
- feat: add tonal synth type variations
- feat: add piano tonal synth type
## [1.2.0] — 2026-07-03

### Added
- feat: refine fluid modulation and ambient reverb
- feat: redesign tonal phrases
- feat: lfo interval on 0.25-beat grid, offset in beats (#5)
- feat: lfo automation with f-key submenu and animated lane (#3)
- feat: add song snapshot codes (stint 0002) (#4)

### Fixed
- fix: tighten save toast and lfo amount step
- fix: remove fluid control foot guns
## [1.1.2] — 2026-07-02

### Fixed
- fix: skip nooise reinstall when current
## [1.1.1] — 2026-07-02

### Added
- feat: use clap for nooise cli
## [1.1.0] — 2026-07-01

### Added
- Add test/check/render just recipes; update DOX and README
- Add headless render subcommand
- Add numeric value entry for control rows (apply_value/NumericEntry)
- Add global Master Tune control, ±1 octave, default flat
- Add Decay control to Bass voice
- Add Bass voice tracking the Pad chord root on a 4-way rhythm pattern
- Add Progression (A/B/C/D) selector to the Chords tab UI
- Add MIDI-authored A/B/C/D chord progressions to the Pad voice
- Add FM synthesis to kick for fuller ambient techno sound
- feat: wire perc interval/offset controls into terminal UI
- feat: bypass GridTrigger for continuous perc noise at interval >= 4.25
- feat: add perc interval/offset controls, lower kick interval floor to 0.25

### Changed
- Bump version to 1.1.0
- Split fluid.rs into fluid/ modules
- Replace four-way control match with declarative ControlSpec registry
- Track wtp worktree tool config
- Initialize DOX AGENTS.md tree
- Give Bass its own authored line, decoupled from Pad chord roots
- Make Bass voice percussive (no sustain)
- Default chords to 8 bars/8s release; smooth Progression A voicings
- Lower Release default from inherited 20s to 1.5s
- Standardize Level/Interval/Offset as first three controls across rhythmic tabs

### Fixed
- Fix Bass Interval to crop the rhythm phrase, not stretch it
- Fix chord smearing: expose Release control, lower Attack floor, resequence progressions
- Fix BPM control stepping by 2 instead of 1
- Fix perc continuous-mode filter being nearly non-op

### Removed
- Remove Bass Release control, fold into Decay
## [1.0.4] — 2026-06-26

### Changed
- Retune nooise defaults
## [1.0.3] — 2026-06-26

### Added
- Add nooise updater command
## [1.0.2] — 2026-06-26

### Added
- Add nooise README preview
## [1.0.1] — 2026-06-26

### Changed
- Document nooise install
## [1.0.0] — 2026-06-26

### Added
- feat(t5e): fluid audio visualizer with kick ripples and control overlay
- feat: add multi-variant UI support for t5 experiment with navigation and layout abstractions
- add t5 experiment with ratatui/crossterm dependencies
- add 12 UI experiments: 7 Rust (ratatui) + 5 Python (Textual)

### Changed
- Release nooise v1
- t5: consistency pass -- uniform beats unit in TUI, explicit field naming
- init: nooise crate

