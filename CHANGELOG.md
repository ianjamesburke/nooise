# Changelog

Newest releases appear first.
## [1.1.0] — 2026-07-01

### Changes
- Bump version to 1.1.0
- Add test/check/render just recipes; update DOX and README
- Split fluid.rs into fluid/ modules
- Add headless render subcommand
- Replace four-way control match with declarative ControlSpec registry
- Add numeric value entry for control rows (apply_value/NumericEntry)
- Track wtp worktree tool config
- Initialize DOX AGENTS.md tree
- Add global Master Tune control, ±1 octave, default flat
- Give Bass its own authored line, decoupled from Pad chord roots
- Remove Bass Release control, fold into Decay
- Make Bass voice percussive (no sustain)
- Fix Bass Interval to crop the rhythm phrase, not stretch it
- Add Decay control to Bass voice
- Add Bass voice tracking the Pad chord root on a 4-way rhythm pattern
- Default chords to 8 bars/8s release; smooth Progression A voicings
- Lower Release default from inherited 20s to 1.5s
- Fix chord smearing: expose Release control, lower Attack floor, resequence progressions
- Add Progression (A/B/C/D) selector to the Chords tab UI
- Add MIDI-authored A/B/C/D chord progressions to the Pad voice
- Add FM synthesis to kick for fuller ambient techno sound
- Fix BPM control stepping by 2 instead of 1
- Fix perc continuous-mode filter being nearly non-op
- Standardize Level/Interval/Offset as first three controls across rhythmic tabs
- feat: wire perc interval/offset controls into terminal UI
- feat: bypass GridTrigger for continuous perc noise at interval >= 4.25
- feat: add perc interval/offset controls, lower kick interval floor to 0.25
## [1.0.4] — 2026-06-26

### Changes
- Retune nooise defaults
## [1.0.3] — 2026-06-26

### Changes
- Add nooise updater command
## [1.0.2] — 2026-06-26

### Changes
- Add nooise README preview
## [1.0.1] — 2026-06-26

### Changes
- Document nooise install
## [1.0.0] — 2026-06-26

### Changes
- Release nooise v1
- feat(t5e): fluid audio visualizer with kick ripples and control overlay
- feat: add multi-variant UI support for t5 experiment with navigation and layout abstractions
- t5: consistency pass -- uniform beats unit in TUI, explicit field naming
- add t5 experiment with ratatui/crossterm dependencies
- add 12 UI experiments: 7 Rust (ratatui) + 5 Python (Textual)
- init: nooise crate

