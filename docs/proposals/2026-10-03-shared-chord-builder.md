# Shared chord builder

Status: built in the October 3 sprint worktree; the two revoices await hands-on audition before publication.

## Editing a progression

On Pads, select Progression and press Enter. Every progression opens eight chords. Enter on a chord opens Root, Accidental, Quality, Extension, Bass, Voicing and Fifth. An authored second extension is visible; `/add extension` reveals an empty second row without changing the chord. Its first value edit saves it. The header names the four source tones that sound.

The first edit copies only that slot's authored controls. Other chords keep their presets. Switching progressions retains each one's edits. `/restore chord` clears the selected override, or returns a Custom slot to its default. The action checks the captured progression, slot and fields before publishing. It closes an open automation editor while retaining its lanes and applying normal Motion takeover.

All fields have stable `/` destinations, including Root and Extension 2. Browsing, opening a drill and saving never create edits. The minimum 46×11 screen and press-only terminals use the same production input path.

## Chord ingredients

Root and Accidental retain the existing A-minor editor coordinates: degree zero is A2; degrees move through the natural-note scale and Accidental transposes the chord by a semitone. These coordinates do not declare a song's home tonic or mode. Stint 0064 owns that separate harmony model; Lead and Tonal scale behavior is unchanged here.

Quality selects the scale third, a minor third, a major third, sus2, sus4 or power. The forced choices read `Min 3rd` and `Maj 3rd`: the fifth remains independent. Fifth `Scale` keeps the established natural-root fifth (diminished on B for the original third choices); the sus/power choices use a perfect fifth. `Perfect` explicitly uses seven semitones; `Omit` removes that role. B plus Min 3rd and Perfect therefore produces ordinary B minor directly.

Each Extension chooses Off, scale 7, scale 9, scale 11, b7, maj7, 9, 11, #11 or 13. Scale choices keep their old pitches. For example, B's scale 9 is C, so its actual name includes b9. Explicit intervals count from the altered root.

The four source tones use this order: root, third/suspension, first extension, second extension, fifth. Duplicate pitch classes are skipped; a conflicting second seventh is skipped; selection stops at four distinct classes. Thus two distinct extensions normally displace the fifth. Spare positions double the lowest selected tones by octaves. Names use the selected roles and actual notes: an omitted fifth plus #11 does not become a diminished fifth, and Close's added inversion tones appear in the name.

## Bass and spacing

The Bass row's first four values retain the original root position and three inversions. Its four added values put a bass at 2, 4, #4 or b7 above the root. These slash choices place the first three selected roles above it, starting at least six semitones higher; a second extension can be omitted by that three-role upper limit.

| Voicing | Four-tone construction |
| --- | --- |
| Close | Double before inversion; resolve unisons by climbing the natural-note scale, preserving existing Custom sound. |
| Stack | Invert distinct selected tones before octave doubling; resolve unisons by octaves. |
| Open | Start from Stack and raise the second tone one octave. |
| Wide | Keep the bass; arrange the other distinct pitch classes from an octave above it, doubling if needed. |
| Drop-3 | Start from Stack and raise the second and third tones one octave. |

Close and Stack differ audibly on inverted triads. Existing Close C/E produces E–G–C–D and now reads `Cadd9/E`; Stack keeps its doubling within the selected chord tones. Open, Wide and Drop-3 preserve pitch classes when unisons collide by moving a duplicate up an octave.

The Bass instrument retains each built-in's authored register and pedal. Only editing Root/Accidental transposes that note by the same root delta. Extension, Quality, Fifth, Bass and Voicing edits affect Pad harmony without jumping the Bass instrument. Custom Bass follows its root as before.

## Two through five output voices

Pad audio and MIDI share `pad_voicing`. It preserves the existing saved count meanings after the four source tones have been built and spaced:

| Count | Output |
| --- | --- |
| 2 | Source positions 1 and 3. |
| 3 | Source positions 1, 2 and 3. |
| 4 | All four source positions. |
| 5 | All four plus source position 1 two octaves higher. |

These are positional choices. Two voices can omit a third; three can omit an extension. Every count keeps the lowest source tone, including slash bass. Five adds a high doubling and does not recover an omitted fifth or extension. Changing that established meaning would require a separate saved-control decision. Bass, Arp and Lead continue to read the underlying harmony through their existing paths.

## Song codes and automation

Custom's eight slots remain in ordinary control snapshot records. Built-in overrides use sparse record 13: version, count, then permanent progression song value, slot and eight signed control values. All banks persist, including inactive progressions. Raw snapshot access is separate from effective editing, so record order cannot seed or overwrite another bank.

Decode validates version, count, key uniqueness, supported progression/slot and every field range before publishing; truncated and trailing bytes are refused. Encode rejects nonfinite, fractional or invalid override fields. New control IDs append to `SONG_ID_TABLE`. Range epoch 6 refuses older modulation of every chord-slot field, including Root/Accidental because those routes previously had no effect on built-ins. Current built-in songs have no such stale routes and need no re-authoring.

Container v2 readers skip unknown records. Shared edited progressions therefore require the current build: an older binary can ignore record 13 and lose those edits. Stints 0065/0066 own the broader future format and launcher boundary.

Automation resolves progression before slot fields, including the release of a removed progression route. It modifies a fixed eight-slot scratch bank in the audio working copy; authored banks are shared through `Arc` and stay fixed. UI edits copy the authored bank on write. Audio setters never copy or allocate a bank. Encoding evaluated controls is refused, and history/session snapshots use authored state. Pad caches resolved notes until a chord field changes. Morph holds the complete outgoing Custom and override banks, with their chord automation, until the existing harmony landing transfers the incoming banks together.

## Preserved notes and audition

The catalog contains 120 chords. The Rust audit preserves 118 exact four-note voicings, including all eight Hosking chords. It also matches all 2,160 archived Custom settings. Required common-tone loops still pass. The two candidates use the same public controls:

| Chord | Before MIDI notes | After MIDI notes |
| --- | --- | --- |
| Ache 8, Em7/G | 43, 50, 55, 64 | 43, 52, 59, 62 |
| Float 7, F | 41, 57, 60, 65 | 53, 57, 60, 65 |

Float's authored Bass still plays F2. Neither candidate occurs in the current 21 built-in songs; all 21 seeded renders match the pre-change PCM exactly.

Local audition artifacts are in `target/oct3-chords`: `before/ache.wav`, `after/ache.wav`, `before/float.wav`, `after/float.wav`, hashes/settings, `presets.json`, and the 46×11/86×30 screenshots. Baseline commit: `ca1ac04`. Each render uses seed 551003, 24 kHz stereo float PCM, 22 seconds, 180 BPM, one bar per chord, all eight slots from offset zero. This covers two complete loops. Listen through each changed chord and its return to chord one; automated equivalence does not decide musical preference.

Reproduce the current engine exports from the worktree:

```sh
RUSTC_WRAPPER= NOOISE_CHORD_EVIDENCE=target/oct3-chords/current cargo test --release --locked chord_builder_audio_evidence -- --ignored --nocapture
```

The ignored exporter derives the built-in song count from `AUTO_STATES`, records its settings beside the hashes, and never rewrites expected fixtures. A paired 30-second default render check retains identical PCM and measures within 2% of the baseline after this ownership change (`target/oct3-chords/scratch-performance.json`). The normal suite covers catalog source notes, legacy Custom notes, common tones, independent edit/restore/switch, malformed song codes, automation and morph, audio/MIDI voice counts, and production input replays on both terminal profiles.
