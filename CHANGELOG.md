# Changelog

Newest releases appear first.
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

