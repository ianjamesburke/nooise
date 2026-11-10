# Purpose

`show/default.json` is the editable default live-rig declaration. It names local audio routes, the optional Take 5 MIDI mapping, the default song, and OBS/Foorm identifiers. It is launch configuration, never portable musical state or a song-code payload.

## Ownership

- `default.json` owns the default show profile consumed by `nooise-show config` and `nooise-show`.

## Local Contracts

- Keep device names as they appear in Mixer’s device list and channels as one-based strings such as `"1-2"`.
- Each `audio.tracks[].name` is unique and becomes the recorded stem name.
- `baton.take5.enabled` chooses the default; `nooise-show --take5 off|pad` can override it for one launch.
- Audio master and stems are always recorded by the show launcher. `recording.events` only controls the optional Baton event journal.
- Do not put song codes, captured audio, credentials, or machine-specific paths here.

## Verification

- Run `nooise-show --dry-run` after edits. It must print the intended Nooise, Mixer, and Baton commands without starting the rig.

## Child DOX Index

- None.
