# Changelog

Newest releases appear first.
## [2.6.0] — 2026-09-30

### Added
- feat: shape drunken timing into per-layer waves
- feat: add global swing and drunken timing modules
- feat: add muted startup flag
- feat: add midrange kick characters for exploration
- feat: optional Pad Offset shifts the stab lane
- feat: open on a Master hub and enter layers instead of paging a tab strip

### Fixed
- fix: reshape exploratory kicks around 909 reference
- fix: keep the hub in the Tab cycle so arrows and Tab reach every page
## [2.5.5] — 2026-09-28

### Fixed
- fix: step the Swing dial one readout point per press
- fix: pad stabs follow attack/release and swing through the Swing module
- fix: retire MIDI Trigger from Pad Trigger drill, share Stabs envelope with Hold
## [2.5.4] — 2026-09-27

### Added
- feat: add auto-morph state
- feat: Shift+L reaches every control and modulator field's ceiling
- feat: bypass and resume selected automation lanes
- feat: capture completed sixteen bar phrases
- feat: restart performance sequences and balance percussion
- feat: capture sixteen beats of knob movement into looping automation
- feat: add Mute Kick palette action
- feat: add Kick Only palette action and honor mutes in MIDI output
- feat: add Pulse Drift and Rise modulation recipes
- feat: add focused-knob modulation recipes to the palette
- feat: preserve chord flow across window edits and add MIDI rows on demand
- feat: route MIDI input through Pads Arp and Lead
- feat: add fresh-session MIDI output setup and lower MIDI controls
- feat: let Pad voice two to five chord notes
- feat: add independent Pad and Lead MIDI output controls
- feat: add Arp MIDI output mode
- feat: make Pad stab gate adjustable
- feat: add swung Pad stab pattern for audio and MIDI
- feat: send Pad chords and transport clock to MIDI output
- feat: emit OSC musical hit events
- feat: emit OSC musical hit events
- feat: mirror live gesture amounts in the OSC feed
- feat: per-voice levels in the OSC feed and a song level profile
- feat: carry kick level, pad envelope, and master level in the OSC feed
- feat: bare --osc targets foorm's default port
- feat: mirror live telemetry as OSC with --osc
- feat: let every live gesture go within 10 ms of release
- feat: retire the Thin gesture
- feat: play live gestures over Master only, gain-staged under the clamp
- feat: refuse old codes that sweep the grown Progression dial
- feat: restart the chord phrase at the next chord change
- feat: add Chord Offset, name progressions by key and mood, add six progressions
- feat: replace built-in song 2 with Ian's brighter perc take
- feat: start the clap's factory filter fully wet
- feat: refuse old codes whose sweeps target a moved dial
- feat: move clap's filter into the shared Filter module
- feat: open the filter dial to 20 kHz and mirror its cutoff on a type flip
- feat: LFO steps run 0 to 100%
- feat: Shift+R inside an LFO's steps rerolls only the step values
- feat: move the clock stop to Shift+P so a stray key cannot end the song
- feat: add a clock stop on p so a song can end on its tails
- feat: read the Jump leader's layer keys off the tab strip
- feat: let a Jump parameter key aim at the page already open
- feat: replace the Sequence mode with a Jump leader on Space

### Fixed
- fix: read swing amount rows on the 50-75% dial scale
- fix: integrate main controls and preserve morph balance
- fix: use sixteen beats for capture blocks and playback
- fix: capture timing knobs and explain pending phrases
- fix: use a beat ramp LFO for the sidechain recipe
- fix: start fresh LFOs at one beat
- fix: preserve silent automation until explicit deletion
- fix: hide Pad rhythm controls on every fresh start
- fix: show active Pad MIDI switch on MIDI launch
- fix: default duplex MIDI sessions to Pad output
- fix: keep MIDI track directions exclusive at startup and edit time
- fix: let Bloom go in 50 ms on release
- fix: brighten Bloom and measure gesture headroom at real playback level
- fix: keep palette-added effects on their page and mark drillable rows
- fix: plain r in an automation editor rolls the row under the cursor
- fix: Shift+R rolls automation fields across their whole dial
## [2.5.3] — 2026-09-20

### Added
- feat: replace built-in song 2 and swap songs 2 and 3
## [2.5.2] — 2026-09-19

### Fixed
- fix: stop Windows key releases from firing every binding twice (#40)
## [2.5.1] — 2026-09-19

### Added
- feat: add Shift+? shortcut map, shrink footer to a terse hint

### Fixed
- fix: also accept Shift+/ reported as base key '/' plus modifier
- fix: bind ? on its own character instead of a SHIFT-gated chord
- fix: show gesture key hints on the activity row when idle
- fix: restore two-layer footer with general shortcuts on bottom row
## [2.5.0] — 2026-09-17

### Added
- feat: add Lift gesture and fast releases
- feat: add normal-mode live effect gestures
## [2.4.0] — 2026-09-16

### Added
- feat: show step pattern followers
- feat: adjust controls while playing lead
## [2.3.1] — 2026-09-16

### Added
- feat: randomize visible control sets
## [2.3.0] — 2026-09-14

### Added
- feat: i enters Lead play mode from any page, top row nudges level/decay/glide
- feat: lead Pattern row sits under Level
- feat: keep a played Lead phrase with r instead of arming a Record state
- feat: lead Pattern transport (Off/Play/Record) with step recording from play mode
## [2.2.0] — 2026-09-13

### Added
- feat: hold a Lead key to sustain it where the terminal reports releases
- feat: lead Follow (Chord/Scale), Ctrl+Q from play and deck, lighter factory drive
- feat: lead feel: click-free retrigger, tighter glide, 256-frame audio buffer, steps behind a pattern drill
## [2.1.0] — 2026-09-13

### Added
- feat: lead types, calibrated level, r to randomize a slider, row scrolling
- feat: play the Lead from the letter row with Enter on its page
- feat: add the Lead voice with a chord-tone step lane
- feat: add the driven 75 BPM song as number 17, after its sibling 16
- feat: restore the sparse 145 BPM song as number 14
## [2.0.1] — 2026-09-13

### Fixed
- fix: lift song 12's kick to match song 11
- fix: lift song 11's kick out of the mix
- fix: split the golden render's two promises so both profiles pass
## [2.0.0] — 2026-09-13

### Added
- feat: one CLI grammar — a song is a number or a code
- feat: make morph state 10 a variant of 9 instead of a tempo jump
- feat: replace morph state 9 with the captured live track
- feat: move kick's filter into the shared module chain

### Fixed
- fix: give morph state 8 its own kick back
- fix: the AUTO footer names the song playing, not a morph that has not started
- fix: give the morph states back their bass drive, perc filter LFOs, and a similar-sounding tail order
- fix: restore the filters the morph states lost
## [1.10.0] — 2026-08-28

### Added
- feat: add filter module

### Fixed
- fix: filter's collapsed knob labels itself Filter, not Slot N Time
## [1.9.2] — 2026-08-12

### Fixed
- fix: make Tonal/Arp level live and tune the pad's opening chord
## [1.9.1] — 2026-08-12

### Added
- feat: add multi-operator FM primitive and level-match kick types

### Fixed
- fix: fade module processors out instead of dropping them
- fix: swap pad character in place instead of retriggering the chord
- fix: retune default mix values
## [1.9.0] — 2026-08-12

### Added
- feat: randomize fresh progression and expand pads
- feat: add stackable automation lanes
- feat: remove macro controls
- feat: recalibrate Drive's knob range and add volume compensation
- feat: add compression, delay, and drive as new module slot types
- feat: add modules from the palette without breaking kernel purity
- feat: fold per-voice effect sliders into module slots
- feat: add per-layer module slots with value-addressed identity (#32)
- feat: unify sliders on a Dial, taper modulation, stop mute ending auto (#31)
- feat: pin a layer's level control when the palette query is its name (#30)
- feat: container-v2 song codes with interned control ids (#29)
- feat: visualize and chord performance layers (#27)
- feat: collect local morph states and Pads naming (#26)
- feat: rebuild performance interaction on kernel (#25)

### Changed
- merge: stackable automation lanes
- merge: crate-wide conventions sweep
- merge: ui four-way split
- merge: automation submodule split
- merge: interaction kernel split
- merge: fx params convention
- merge: runtime key tables
- merge: voice dedup
- merge: registry table dedup

### Fixed
- fix: apply pad type changes immediately
- fix: gate mute after automation
- fix: make tonal offset ride the phrase
- fix: keep envelope animations green
- fix: close neutral lfo editor cleanly
- fix: stage pad progression changes
- fix: draw modulation markers on the contextual mapping
- fix: keep FX tails out of song codes
- fix: apply Chord Count to built-in progressions, not just Custom
- fix: stop module-slot rows on Chords from landing inside the chord drill
- fix: keep the open editor drawn during numeric entry (#28)
- fix: make add-morph transactional (#17)
## [1.8.5] — 2026-07-27

### Added
- feat: just add-morph auto-commits its own change
- feat: add just add-morph tool and new auto-morph states
## [1.8.4] — 2026-07-27

### Fixed
- fix: show song indexes during auto morphs
## [1.8.3] — 2026-07-27

### Added
- feat: add control palette
- feat: prioritize global recent controls and copy raw song codes
- feat: add / control palette: fuzzy-find any control, inline value entry, staged batch edits with next-bar commit
- feat: add another auto morph target state
## [1.8.2] — 2026-07-24

### Added
- feat: add per-chord major/minor quality override to custom chord slots
## [1.8.1] — 2026-07-21

### Added
- feat: add a low-tempo auto morph state
- feat: persist evolved tonal sessions (#15)

### Changed
- Cut drum exits on morph downbeats (#16)
## [1.8.0] — 2026-07-20

### Fixed
- Fix LFO navigation and timing behavior (#14)
## [1.7.2] — 2026-07-20

### Added
- feat: add two new auto-morph target states
## [1.7.1] — 2026-07-20

### Added
- feat: custom Steps LFO shape (automation step sequencer) (#13)
- feat: add 4 selectable kick drum types (#12)
- feat: reorder tabs to Chords-first, Master-last
## [1.7.0] — 2026-07-19

### Added
- feat: add two new morph targets to auto-morph cycle
## [1.6.1] — 2026-07-19

### Added
- feat: badge the currently-sounding chord in the custom progression UI

### Fixed
- fix: ease ramp LFO cycle-wrap discontinuity to stop level/cutoff clicks
## [1.6.0] — 2026-07-18

### Added
- Add full-band build state to the auto-morph rotation
- Add tapered dial mapping; unify tonal/arp envelope to attack+decay
- Add release-build render benchmark script
- feat: auto-morph slow evolution between song states (stint 0024) (#6)
- feat: add m/M mute keybindings for track and master (stint 0027)
- feat: add instrument type selector to ARP (stint 0026)

### Changed
- Symlink CLAUDE.md to AGENTS.md
- Halve startup fade-in from 8s to 4s
- Lower TUI redraw pacing from 60fps to 30fps

### Fixed
- fix: default bass decay to 300ms
- fix: morph LFO/envelope/macro automation through auto-mode
- fix: guard GridTrigger against double-hit on offset/rate edits (stint 0025)

### Removed
- Remove kick echo/delay engine and its sliders
## [1.5.2] — 2026-07-15

### Added
- feat: mirror arp level onto the Master tab mixer
- feat: add arp.offset_beats control
- feat: add drill-down navigation to the Chords tab
- feat: add arp.reverb_mix control, replacing fixed ambient reverb mix
- feat: add tonal.octave whole-octave transpose control
- feat: add bass.cutoff one-pole lowpass filter
- feat: add custom chord progression builder
- feat: add pad chord type character variants
- feat: add bass type character variants
- feat: add arp voice following the pad chord progression
- feat: add attack/release controls to tonal
- feat: add four new chord progressions (two dark modal, two major)
- feat: beats-based chord length entry, eased gain ramps, macro LFO field guard
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

### Changed
- Update README with playlist link

### Fixed
- fix: lower clap filter default to 75%
- fix: make bass monophonic with hard-cut retrigger
- fix: rebuild audio stream when the default output device changes
- fix: percent entry always means percent, v on Shape no-ops, macro-driven LFO amount survives hide
- fix: Esc never quits, unify one-level-at-a-time editor close

### Performance
- perf: cut wasted work from the per-sample audio path
- perf: allocation-free audio hot path, opt-level 1 dev builds
- perf: instant-feel input and audio response
## [1.5.1] — 2026-07-15

### Added
- feat: mirror arp level onto the Master tab mixer
- feat: add arp.offset_beats control
- feat: add drill-down navigation to the Chords tab

### Fixed
- fix: lower clap filter default to 75%

### Performance
- perf: cut wasted work from the per-sample audio path
## [1.5.0] — 2026-07-14

### Added
- feat: add arp.reverb_mix control, replacing fixed ambient reverb mix
- feat: add tonal.octave whole-octave transpose control
- feat: add bass.cutoff one-pole lowpass filter

### Fixed
- fix: make bass monophonic with hard-cut retrigger
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

