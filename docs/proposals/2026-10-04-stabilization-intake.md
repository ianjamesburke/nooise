# Stabilization and next musical directions

Status: user intake, captured October 4, 2026. Ian resumed sprint work on October 5. Explicit requests below guide the plan; suggested defaults and open questions are not settled designs.

## Priority

Stabilize the current instrument and reconcile old stints before starting more hardware work. Ian is actively exploring microphone input in a separate worktree. APC40 support is wanted soon after stabilization. Preserve that exploration and the existing hardware branches.

Baseline inspected: main `f28f95d`, with 833 tests passing and five ignored in a fresh run. Automated evidence does not establish musical acceptance. The audit and the open-stint list must distinguish shipped work, unfinished acceptance, reported bugs, and new projects.

## Captured requests

| Topic | Requested behavior or direction | Existing work and next question |
| --- | --- | --- |
| Named songs and CLI cueing | Eventually pass a song name to the CLI and send songs into a running player. An optional server mode should document a default local port and permit an explicit override; optional hosted coordination may come later. Cue a next song and optionally morph/crossfade into it. A central mixer is exploratory. | Related: 0051 playlists, 0060 musical boundaries, 0074 library/session work. Start with one local running engine and one queued transition before choosing hosted coordination or a mixer. |
| Ableton Link | Resolve the existing worktree without forcing a CMake build on ordinary nooise users. Consider an opt-in live flag and capability check. | `feat/ableton-link` already declares optional `rusty_link` behind the `link` Cargo feature and accepts `--link`. Build capability and runtime participation are separate. Detecting Ableton Live installation does not supply the Link library; Link peers can be other applications. |
| OSC harmony and phrase timing | Send enough chord and phrase timing information for a consumer to follow the music, possibly a phrase count. | Main exports `/nooise/beat`, `/nooise/chord`, and `/nooise/chord/change`. Define missing phrase data, reset semantics, and whether chord notes/quality are needed rather than duplicating existing messages. The consumer's intended reaction is still unspecified. |
| Phrase restart | Ian confirmed October 5 that the reported Shift+P phrase restart issue is fixed. | No open defect or special sprint audition item. Existing restart behavior and regression tests remain in place. |
| Inert effect insertion | Adding a knob effect/module or modulation recipe must initially pass audio through unchanged; Sway and other LFO recipes should start at Amount 0%. After adding a recipe, focus Amount for immediate playing. | Main recipes start at 25%; Sidechain also lowers the authored base, and recipe application closes the editor. A zero-depth change alone would not make Sidechain inert. Ordinary empty modules and zero-amount lanes must retain their existing persistence/lifetime rules. |
| Drift shortcut | `/drift` creates RandomDrift with a four-beat cycle and 0% initial Amount, matching the general inert-insertion rule. | Decision confirmed October 5. Current source still uses sixteen beats and 25%; recipe insertion/focus work owns the correction. |
| Harmony and scale | Settle an explicit scale and progression model; consider applying progressions in different scales. The selected scale should meaningfully govern melodic note generation. | Related: 0064/0065. `feat/progression-mode` is a possible design source for this later project, not part of stabilization or the current push. The current Tonal investigation is narrower: decide key/root/mode, borrowed chords, chord-following versus scale-following, and how authored progressions transpose. A chosen scale is not automatically the union of progression chord tones. |
| Tonal pitch | Reported: Tonal often sounds out of key. Preserve its enjoyable way of discovering melodies. | Capture a concrete song/seed and sounding harmony, then diagnose current note choice and voice leading. Treat that reproduction separately from deciding a complete replacement architecture. |
| Common melodic layers | Tonal, Arp and Lead should become instances of a common melodic layer, perhaps added through `/melo`, with arp/sequencer/live-lead behaviors and note randomization available across instances. Start with two independently configurable leads and keep either only a few keys away. | Related: 0065 reusable tracks and 0071 independent layers. The command name and exact modes are proposals. Preserve per-instance evolution, modules, automation, mutes, identity and compact persistence; keyboard navigation remains primary. |
| Global Swing | Keep the rhythmic foundation steady. Ian does not want global Swing moving Kick; at minimum quarter/half-note and downbeat kick/snare events should stay anchored. | Resolve whether Kick is fully exempt or only anchor beats are protected. No dedicated snare layer is assumed: map the intended sound to current voices. Test actual hit times with global and local timing modules. |
| Global Drunken | Consider reducing the global Drunken influence on Kick by 50% to retain a solid foundation. | Proposed musical weighting, not yet an approved constant. Preserve other voices and determine how local Drunken combines with the global amount. Audition before treating the weight as settled. |
| LFO Amount during morph | LFO Amount should glide between states rather than switch abruptly. | Main already interpolates `depth_ratio` through `LfoRoute::morph`. Verify production transitions, including appearing/disappearing/bypassed lanes and module swaps, before deciding a code change is needed. Related: 0050. |

## Smallest song-cue experiment

Proposed first proof: one local nooise process receives a target built-in song number or current song code from a second CLI invocation, displays it as queued, and adopts it at a defined musical boundary. Demonstrate immediate change and one explicit morph duration using the current state-transition machinery. Define replacement/cancel behavior, acknowledge actual adoption, and preserve keyboard access to the same operation.

Names can initially map to existing codes in a small local catalog. Hosted coordination, multiple rendering engines, audio crossfading, and a central mixer are later decisions. A parameter morph inside one engine and an audio crossfade between two engines are different operations; audition the first proof before choosing the latter.

The experiment is a proposal for a later step. It should leave plain startup unchanged and perform all networking outside the audio callback.

## Sprint decisions still needed

- Clarify the microphone goal: synthesized voice from singing, live texture processing, or both. Keep its active branch independent until the desired integration point is known.
- Capture a concrete Tonal song code and note trace that sounds wrong.
- Select the intended OSC consumer response: chord color/notes, phrase-aligned scene changes, or another behavior.
- Settle harmony behavior before the shared-track rewrite; keep the current pitch bug actionable on its own.

## Verification and ownership

Runtime changes follow root/source DOX, existing Rust gates, production input replay, persistence checks, and deterministic audio evidence where relevant. Musical feel and live microphone/controller behavior require their own acceptance. No release, PR, merge, or existing-worktree cleanup follows from this intake.

Product rules belong in `../NORTH_STAR.md`; shipped input behavior belongs in `../PERFORMANCE.md`. Existing stints remain the owners of their scopes. New task creation and status reconciliation follow the completed audit and the revised plan.

Link reference: [Ableton Link concepts](https://ableton.github.io/link/) describes peer applications, beat/tempo/phase, and start/stop synchronization.
