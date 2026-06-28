---
id: "0001"
title: "Add perc beat control with continuous-noise endpoint"
status: backlog
priority: p2
estimate: "2h"
blocked_by: []
gh_issue: []
area:
  - "audio/perc"
tags:
  - "v1"
  - "audio"
  - "ui"
  - "testing"
---

Add a perc beat/subdivision control so the noise rhythm is not hard-wired to `0.25` beat hits. One end of the range should bypass discrete hits entirely and produce a full continuous noise stream.

## Scope

- Add a perc control for beat value or subdivision and wire it into the terminal UI.
- Replace the fixed `GridTrigger` interval in `PercEngine` with the selected beat value.
- Define a clear continuous mode at one control extreme where no `NoiseHit`s fire.
- Verify the continuous mode against an audio-level signal, not only by checking internal hit counts.
- Delete or shrink the related `GOTCHAS.md` entry after the task is implemented so the repo does not keep stale failed-approach notes around.

## Non-Scope

- Do not retry the crossfade or analytical RMS switch approaches.
- Do not redesign kick, clap, pad, tonal, or master-bus controls unless the audio-level test proves they cause the reported pulse.

## Why

Full decay still produced syncopated pulses after two failed transition approaches, so the rhythm needs its own control instead of hiding fixed 16th-note triggering behind decay.

## References

- `GOTCHAS.md` - failed crossfade and analytical-switch attempts, plus the beat-control idea.
- `src/fluid.rs` - `PercControls`, terminal control wiring, and `PercEngine` live here.
