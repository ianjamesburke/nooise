# Venue rig and performance budget for November 22

Status: research report with proposed defaults. Nothing here is built or ruled on.
Read-only investigation on 2026-11-12; no repo, profile or launcher file was changed.
Measured on the show laptop (`MacBookPro18,1`, Apple M1 Pro, 8 performance + 2 efficiency cores, 16 GB, macOS 14.5) while unrelated agents kept the load average at 16 to 18.

Sources are cited inline. "Launcher" means the installed copy in `~/.local/share/nooise-show/launcher/` (source: `baton/worktrees/fix/backward-beat-timeline/scripts/show/`, doc: `baton/worktrees/fix/backward-beat-timeline/docs/SHOW-LAUNCHER.md`). "nooise" means `nooise/worktrees/v4` at `43817fb`, one commit past the pinned show snapshot `v4-32bb4f6`.

## Recommendations first

1. Turn Low Power Mode off on the show laptop. `pmset -g` reports `lowpowermode 1` today, on AC power. Zero code, largest free headroom gain.
2. Play the venue with nooise straight into the audio device. BlackHole and the mixer leave the audio path; baton and foorm stay as a side chain that cannot stop audio. This needs one small launcher change (direct audio mode). Until that is built and rehearsed, the bare one-line command in section 5 is the fallback and works today.
3. Fix the visual bridge before promising a second computer: the bridge binds `127.0.0.1` and so cannot send to another machine (reproduced, section 1D).
4. Replace the pinned foorm with a release build. The installed one is a debug build and costs 28% of a core against 8.5% for release.
5. Run the first full 60 minute rehearsal now. The longest recorded session to date is 5.8 minutes (`~/Music/Nooise Sessions/*/audio/manifest.json`). Everything else in this document is smaller than that gap.
6. Set `safety.idle_stop_minutes` to `null` in every venue profile. At 30 it stops the whole rig after 30 minutes of stopped transport and silence, which is exactly what a rig launched early for soundcheck looks like.

## 1. The venue profile and its tiers

### What the launcher can select today

Profile schema 2 (`launcher/config.mjs:66-117`):

| Want | Field today | Result |
| --- | --- | --- |
| No foorm, no bridge | `visuals.enabled: false` (then `visuals.obs` must be `false`) | Works |
| foorm on another machine | `visuals.foorm_listen` and `visuals.foorm_target` on different hosts | Launcher skips local foorm and starts the bridge (`config.mjs:194-195`), but the bridge cannot reach the target; see 1D |
| No recording | `recording.audio: false`, `recording.events: false`, `safety.idle_stop_minutes: null` | Accepted, but mixer readiness then hangs unless the routes are BlackHole (`nooise-show.mjs:110` hardcodes `BlackHole 16` and `BlackHole 2ch`) |
| No mixer | none | Not possible. `audio.monitor_output`, `audio.stream_output` and a non-empty `audio.tracks` are required (`config.mjs:71-72`) and the mixer is always launched (`config.mjs:190`) |
| No baton hub | none | Not possible. The hub is always launched (`config.mjs:191`) |
| Starting state | `nooise.song` accepts an `n1_` code | Works |
| Start stopped | always passed (`config.mjs:189`) | Works |
| State bank | `nooise.set` | Works |

So today the launcher can only run the mixer chain. A venue profile inside the current schema is: nooise to `BlackHole 2ch`, mixer reads it and plays to the venue device as `audio.monitor_output`, `audio.stream_output` on `BlackHole 16ch`, recording on.

### Tiers

| Tier | Chain | Launcher today | Needs adding |
| --- | --- | --- | --- |
| A bare | nooise to device | No. Run the command in section 5 by hand | Nothing; keep it as the written-down fallback |
| B record | A plus a recording | Only as the mixer chain (nooise to BlackHole to mixer to device). Recording is the mixer's master and stems | For a direct chain: a recorder that is not in the audio path (see below) |
| C visuals, one machine | A or B plus hub, bridge, foorm | Yes, with the mixer chain | Direct audio mode |
| D visuals, second machine | C with foorm remote | Schema accepts it; bridge fails to send | Bridge bind fix; foorm built on the second machine |

Recommended venue set, as separate profiles so the picker is the tier switch (lowest `order` is preselected):

- `venue` (order 1): direct audio, hub + bridge + local foorm, no recording by the rig.
- `venue-remote-visuals` (order 2): same, foorm on the second computer.
- `venue-mixer` (order 3): the chain that exists today, venue device as the mixer monitor output, mixer records. This is the only one launchable before any launcher work.
- Tier A stays a command on paper, outside the launcher, because the reason to use it is that something in the launcher path failed.

### Launcher changes needed for the direct tiers

Concrete user-facing problem: at the venue the mixer and BlackHole are two extra real-time processes and a second clock domain between nooise and the PA, and neither does anything a nooise-only set needs except recording.

1. `audio` becomes optional. Absent means direct: no mixer is started, the hardware check lists devices with `nooise --audio-devices` (`nooise/worktrees/v4/src/main.rs:41-47`) instead of `mixer devices`, and `recording.audio` must be `false`. Present keeps today's behaviour.
2. Bridge gets `--listen 0.0.0.0:0` when foorm is remote (1D).
3. `mixerReady` stops matching literal BlackHole names in the non-recording path (`nooise-show.mjs:110`).
4. Pin a release foorm (section 3).

Not recommended: making the hub optional. It is 1% of a core, is outside the audio path, and the idle watchdog and journal already depend on it.

### Recording without the mixer (tier B on a direct chain)

| Option | Why | Why not |
| --- | --- | --- |
| Keep the mixer chain for recording | Works and is the only recorded path so far; master plus dry stem, gap counters | Puts BlackHole, a resampler and two more real-time callbacks between nooise and the PA |
| OBS records program audio directly | No extra audio process in the path; OBS 30.2.3 on macOS 14.5 has ScreenCaptureKit audio capture | Not verified here. The launcher's OBS setup only wires `BlackHole 16ch` (`config.mjs:9`, `obs.mjs`), so this is manual scene work and a rehearsal item |
| macOS Multi-Output device (interface + BlackHole) as nooise's output | Recording tap in parallel, not in series | Blocked: aggregate output `SYS OUT BlackHole and Headphone` exists in `system_profiler SPAudioDataType` but is missing from `nooise --audio-devices` and `mixer devices` (same open bug as TRUTH.md "Mixer QA" item 2). Cause unknown |
| Recording from the venue desk, or a handheld recorder on the second output pair | Cannot affect the set | Depends on the venue |
| nooise records its own master (`--record-dir`) | Right long-term shape: removes BlackHole and the mixer from every nooise-only set | Does not exist (only `render` writes WAV). Touches the audio callback; after the show |

Call: for November 22, do not let recording decide the audio path. Use the direct chain and take the recording from OBS if the ScreenCaptureKit source passes rehearsal, otherwise from the venue desk or a handheld recorder.

### 1D. foorm on a second computer

What exists:

- foorm takes `--listen ADDR` (`foorm/src/main.rs:19-21`) and consumes plain nooise OSC over UDP (`foorm/src/osc.rs:119-180`).
- `baton foorm` joins the hub, filters to the visual stream and forwards it to `--target` (`baton/.../src/adapters/foorm.rs:28-41, 47-90`).
- The launcher skips local foorm when the two visual addresses are on different hosts and prints where the bridge sends (`config.mjs:194`, `nooise-show.mjs:209`).

What is broken: the bridge's `--listen` defaults to `127.0.0.1:0` (`baton/.../src/main.rs:147-148`) and the launcher does not override it (`config.mjs:195`). The same socket is used to send to the target (`foorm.rs:48, 83`). A socket bound to loopback cannot send off the machine on macOS. Reproduced on this laptop:

```
127.0.0.1 -> remote FAIL [Errno 49] Can't assign requested address
0.0.0.0 -> remote OK
```

(Python `socket.bind((b, 0)); sendto(b"", ("192.0.2.1", 9002))`.) The launcher tests that cover "remote Foorm" are mocked (`SHOW-LAUNCHER.md` Verification), so they do not see this.

What the network needs:

- Unicast UDP in one direction, laptop to second computer. No multicast, no broadcast, no discovery, no return path. foorm never talks back.
- A wired link: Ethernet or a Thunderbolt cable between the two Macs. Give both ends static addresses (for example `10.22.0.1` and `10.22.0.2`, mask `255.255.255.0`) so the profile can hold a fixed `foorm_target`. Do not depend on venue Wi-Fi or an ad hoc Wi-Fi network.
- On the second computer: `foorm --listen 0.0.0.0:9002` (or its static address), a full-screen terminal, and the macOS firewall allowing incoming connections for the foorm binary.
- Profile: `"foorm_listen": "10.22.0.2:9002"`, `"foorm_target": "10.22.0.2:9002"` would be read as local (hosts equal, `config.mjs:87`). Use `"foorm_listen": "127.0.0.1:9002"` with `"foorm_target": "10.22.0.2:9002"`; only the hosts differing matters, and `visuals.obs` must be `false`.
- Packet load is small: beat at up to 200 Hz (5 ms poll, `nooise/.../osc.rs:196`), meters at 30 Hz (`osc.rs:199`). A lost packet is harmless because `/nooise/beat` carries the absolute beat.

Simpler alternative, rejected: `nooise --osc=10.22.0.2:9002` straight to the remote foorm (nooise binds `0.0.0.0`, `osc.rs:991-996`, so this works). It removes the hub's only input, which breaks the idle watchdog and any baton client.

Not verified: an actual two-machine run. The second laptop's model and macOS version are still an open "Needs Ian" item in TRUTH.md.

### Device names at the venue

- nooise selects by exact name (`--output-device`, `main.rs:78-85`). Today's list: `BlackHole 16ch`, `BlackHole 2ch`, `External Headphones`, `FaceTime Screen Cap`, `Immersed`, `QT IN Laptop Mic and Blackhole`, `Reincubate`, `poop`.
- `MacBook Pro Speakers` is absent from that list while headphones are plugged in, although macOS lists it. Inference, not tested: `External Headphones` likewise only exists while a plug is in the jack, so a profile naming it fails to launch with nothing plugged in.
- The Scarlett 2i2's exact name is unknown until it is connected. Run `nooise --audio-devices` with it plugged in and copy the line.
- With an explicit device, a lost interface is retried by name and never falls back to the system default (`src/audio.rs:330-338`). Omitting `--output-device` follows the system default and rebuilds onto whatever macOS switches to, for example a newly plugged HDMI display. Always name the device at the venue.

## 2. Latency

Not relevant to the music (nothing is triggered against an external clock), relevant only if Ian monitors through a different path than the audience.

From code, at 48 kHz:

| Stage | Frames | ms | Source |
| --- | --- | --- | --- |
| nooise output buffer | 256 | 5.3 | `nooise/.../src/audio.rs:19, 495-500` |
| nooise master limiter lookahead | 72 | 1.5 | `src/fx/limiter.rs:15`, ADR 0004 |
| BlackHole driver | 0 advertised | 0 | [BlackHole README](https://github.com/ExistentialAudio/BlackHole); not measured here |
| mixer input callback buffer | 256 | 5.3 | `mixer/.../src/audio.rs:20, 792` |
| mixer input queue, held at target | 512 | 10.7 | `PREFILL_FRAMES`, `TARGET_QUEUE_FRAMES`, `mixer/.../src/audio.rs:22-24, 453-459` |
| mixer drift resampler chunk | 64 | 1.3 | `RESAMPLER_CHUNK_OUT`, `audio.rs:27` |
| mixer limiter | 3 sample history, no lookahead | ~0 | `mixer/.../src/lib.rs:464-500` |
| mixer output buffer | 256 | 5.3 | `audio.rs:928` |

- Direct chain: about 6.8 ms plus the interface's own hardware latency.
- BlackHole plus mixer adds about 23 ms on top (5.3 + 10.7 + 1.3 + 5.3), so roughly 30 ms total before hardware latency. At home today it is slightly more, because both BlackHole devices run at 44.1 kHz (`mixer devices`), which makes the 256 and 512 frame stages 5.8 and 11.6 ms.
- Not measured end to end. That needs a physical loopback (output to input) and was out of scope. The interface's safety offset and converter delay are unknown until the venue device is chosen.

Side finding: because BlackHole is at 44.1 kHz, the home rig runs the nooise engine at 44.1 kHz, while the built-in output is 48 kHz. A direct venue chain will run the engine at a sample rate it has not been rehearsed at. Set both BlackHole devices to 48 kHz in Audio MIDI Setup (already listed as Hardware QA item 1 in TRUTH.md and still not done) or rehearse on the direct chain.

## 3. CPU and dropout budget

### Measured

Method: each process started on a private pseudo-terminal and private ports (9790 to 9793), nooise to `BlackHole 2ch`, mixer from `BlackHole 2ch` to `BlackHole 16ch`, hub, bridge, foorm; CPU seconds from `ps -o time` over a 15 to 20 second window after 6 seconds of settling. Percent of one core. Script: scratchpad `measure.py`.

| Scenario | nooise | mixer | hub | bridge | foorm | Total |
| --- | --- | --- | --- | --- | --- | --- |
| Song 1, transport stopped | 3.4 | 2.5 | 0.3 | 0.3 | 11.3 (debug) | 17.7 |
| Song 1 playing | 8.4 | 2.3 | 0.6 | 0.5 | 22.0 (debug) | 33.9 |
| Song 9 playing | 13.1 | 2.6 | 1.0 | 0.7 | 27.9 (debug) | 45.4 |
| Song 9 playing, release foorm | 13.0 | 2.6 | 0.9 | 0.7 | 8.5 (release) | 25.8 |

- Resident memory: nooise 16 to 20 MB, mixer 12 to 17 MB, the rest under 7 MB each.
- Terminal output volume: nooise 0.5 to 0.7 MiB/s, foorm 2.1 to 3.3 MiB/s. The terminal emulator has to parse and paint that; its cost is not in the table.
- Offline DSP cost: `nooise render --seconds 60` took 2.44 s of CPU, so the default mix costs about 4% of one core, a 24x real-time margin on average. The rest of nooise's live figure is the UI and OSC threads, which are not on the audio thread.
- OBS: configured for 1920x1080 at 30 fps with the VideoToolbox HEVC hardware encoder at 2500 kbps (`~/Library/Application Support/obs-studio/logs/2026-11-11 22-49-08.txt`). The longest OBS recording in the logs is 2.5 minutes; its graphics thread median was 6.2 ms of a 33.3 ms frame. No lagged or skipped frame lines were logged.

The installed foorm is the debug build: `sha256(~/.local/share/nooise-show/bin/foorm)` = `54cd5b24…` = `foorm/target/debug/foorm`, and the launcher pins that path (`config.mjs:183`). Its file date is September 19, before foorm's last two commits (September 25), so it is probably also stale; that is an inference from dates. The release build measured above is `foorm/target/release/foorm` (October 30, `edd5460f…`) and is not the same source revision, so the 28 to 8.5 comparison mixes build mode and revision.

### Is 60 minutes with everything running safe?

On CPU, yes by a wide margin: the rig's own processes total under half of one core out of ten, measured while the machine was already loaded and in Low Power Mode. The 2026-11-10 run had no dropouts at a load average near 160 (TRUTH.md).

Not established, and these are what a rehearsal is for:

- No run longer than 5.8 minutes exists. Thermal behaviour over an hour is unknown. `pmset -g therm` currently reports no thermal or performance warning.
- OBS and the terminal emulators (Plexi Beta panes, Ghostty for foorm) were not measured running. OBS was not launched for this report.
- Worst-case audio callback time. nooise has no callback timing or overrun counter (no such code in `src/audio.rs`), so on a direct chain a dropout is only detectable by ear or by analysing a recording. On the mixer chain the mixer shows `underruns` / `overruns` per track (`mixer/.../src/ui.rs:2242`) and writes `dropped_frames` and `gaps` into `audio/manifest.json`.

### Biggest dropout risks, in order

1. Other work on the machine. Agent fleets and cargo builds put the load average at 16 to 160 during recent sessions. The audio callback is a real-time thread, but build storms also cost memory bandwidth, disk and heat.
2. Low Power Mode (`lowpowermode 1` in `pmset -g`). It caps CPU performance.
3. Allocation on the audio thread at a state landing. A landing clones a snapshot on the audio thread, and an automation plan rebuild allocates its routes there (`src/AGENTS.md:137`). This is the one remaining known heap use in the callback and it happens exactly at `'`+digit landings.
4. Per-note allocations (perf audit item 3, deferred).
5. foorm debug build on the same machine: the single largest CPU user in the rig, plus megabytes per second of terminal output.
6. The mixer chain itself: a second clock domain (BlackHole against the output device) held together by an adaptive resampler; any device change rebuilds streams with a roughly 2,000 sample blip (TRUTH.md).
7. Event journal at about 6 GB per hour (TRUTH.md): sustained disk writes for something the show does not need.
8. Display sleep is 15 minutes (`pmset -g`). The laptop is being typed on, but a second computer showing foorm is not. The launcher does not run `caffeinate`.
9. TUI redraw: 30 fps (`src/fluid/runtime.rs:31, 37`), off the audio thread. Not a dropout source on the evidence; TRUTH.md item 5 already says to build a "performance mode" only if the UI shows up in measurement. It does not.

### Perf audit: fixed and open

The audit report itself is not in the repo; its findings are recorded in TRUTH.md (`## Next`, "Perf audit early finding" and "Final audit").

| Finding | State | Evidence |
| --- | --- | --- |
| 1. Control tick re-parsed 385 slot ids every sample for 128 samples before each bar (about 3.6 ms of the 5.8 ms budget) | Fixed, in v4 | `f21bb9d` is an ancestor of v4 `43817fb`; TRUTH.md records the bar-crossing buffer at about 0.3 ms afterwards |
| 2a. Audio thread freed retired session state on UI edits | Fixed, in v4 | `75cdcf5` in v4; contract in `src/AGENTS.md:137` |
| 2b. Audio thread allocates on plan rebuild and state landing | Open | `src/AGENTS.md:137` |
| 3. Per-note allocations | Open, deferred past the show | TRUTH.md |
| 4. Meter telemetry at 172 Hz | Fixed, in v4 | `ec4acc0` in v4; `METER_INTERVAL` 33 ms, `src/fluid/osc.rs:199` |
| Landing prebuild | Open, deferred past the show | TRUTH.md |
| Probe for a stopped launch and a mid-bar stop | Not in v4 | `9f35033` exists only on `temp-show-perf` (test only) |

## 4. What to do before the November 17 to 18 freeze, and what after

Ranked by dropout risk removed per unit of effort.

Before the freeze:

| # | Action | Effort | Why |
| --- | --- | --- | --- |
| 1 | Low Power Mode off (`sudo pmset -a lowpowermode 0`), and keep the charger in | Settings | Free headroom |
| 2 | Nothing else running during the set: no agents, no builds, Wi-Fi off, notifications off | Habit | Removes the largest load source seen in every rehearsal so far |
| 3 | First 60 minute rehearsal with logging (section 7) | One hour | Nothing over 5.8 minutes has been run |
| 4 | `recording.events: false` and `safety.idle_stop_minutes: null` in venue profiles | Profile | Stops 6 GB/h of disk writes; stops the rig killing itself during a long soundcheck gap |
| 5 | Pin a release foorm built from the intended revision | Build + `config.mjs` candidate | 28% to about 8.5% of a core, less terminal output |
| 6 | BlackHole to 48 kHz, or rehearse on the direct chain | Settings | Rehearse the sample rate the venue will run |
| 7 | Launcher direct audio mode (section 1) | Launcher JS only, no audio code | Removes BlackHole, the resampler and two real-time callbacks from the PA path. Only if it gets two full rehearsals |
| 8 | Bridge `--listen 0.0.0.0:0` for a remote foorm | One launcher line | Without it tier D does not work at all |
| 9 | Audio callback worst-case timer and over-budget counter in nooise, shown in the footer and printed at exit | Two atomics in `SharedEngine::render` (`src/audio.rs:475-489`) | Makes a direct-chain rehearsal measurable. Touches the callback, so it must land before the last two rehearsals or wait |

After the show:

- Preallocate state landings and plan rebuilds (audit 2b), remove per-note allocations (audit 3).
- nooise records its own master, which retires BlackHole and the mixer for nooise-only sets.
- Root-cause the missing aggregate and speaker devices in the cpal device list.
- A reduced-animation UI mode: not justified by today's numbers.

## 5. Starting state at the show

What exists (all verified in v4 source):

- `--start-stopped` opens with the transport stopped and the song's levels and mutes intact (`src/main.rs:103-105`, `src/fluid/mod.rs:451-457`). The engine renders exact silence and sends no notes until play (`src/fluid/tests.rs:1323`).
- A song is a built-in number or an `n1_` code (`src/main.rs:304-323`). A code launch runs with no auto morph (`src/fluid/mod.rs:459-466`). `nooise 1` is a one-song auto morph (TRUTH.md); for song 1 the two render identically, for other songs they do not.
- Tempo is part of the state: `master.bpm` is a registry control (`src/fluid/registry.rs:1609-1618`) and a stored state restores its BPM after reopening the bank (`src/fluid/states.rs:178-214`). No separate tempo flag exists or is needed.
- `--set PATH` loads and saves states 1 to 9 (`src/main.rs:96-98`); the file is one `number<TAB>code` line per slot (`docs/PERFORMANCE.md`, Prepared states).
- `Ctrl+S` copies the current `n1_` code (`src/AGENTS.md:20`).
- Shift+P starts and stops. Starting always returns to bar 1 (`docs/PERFORMANCE.md`, Clock stop).
- There is no way to load a code or recall a state instantly into a running nooise. A queued state waits for the phrase end (about 94 seconds on song 1), and while stopped it waits for a restart plus a whole phrase.

Procedure:

1. At home, on the pinned show binary, dial the opening state, press `Ctrl+S`, and paste the code into the venue profile as `nooise.song`. Author it on the binary that will play it: another build may refuse the code.
2. Launch. The launcher runs:

```
<nooise.bin> 'n1_<opening code>' --start-stopped --osc=127.0.0.1:9000 --osc-in=127.0.0.1:9001 --output-device '<device>' --set <bank>
```

   Bare tier A, by hand, no launcher:

```
~/.local/share/nooise-show/nooise-bins/<pinned>/nooise 'n1_<opening code>' --start-stopped --set ~/.config/nooise-show/sets/venue.set --output-device '<exact name from nooise --audio-devices>'
```

3. Soundcheck: Shift+P to play, check the bass and levels on the PA, Shift+P to stop.
4. If nothing was edited during soundcheck, Shift+P starts the set from bar 1 in the opening state.
5. If anything was edited (song 1's authored Kick and Bass levels are 0, so a bass check there means raising Bass), relaunch: `nooise-show stop`, then `nooise-show --profile venue`. The 2026-11-10 launch was ready in 7 seconds. That is the only way back to the exact opening state.

Better for step 3: make the opening state one in which the bass is already audible at set level, or keep a loud soundcheck state and launch it with `--song` for the check, then relaunch without the flag.

## 6. Ideas, ranked

What is broadcast today (`nooise/.../src/fluid/osc.rs`): `/nooise/beat`, `/nooise/level`, `/nooise/voice/<layer>/level`, `/nooise/voice/kick`, `/nooise/hit/{kick,clap,perc,pad,arp,tonal,beat}` (level, pitch class), `/nooise/chord`, `/nooise/chord/change`, `/nooise/chord/span`, `/nooise/phrase`, `/nooise/phrase/reset`, `/nooise/gesture/{bloom,submerge,echo,lift,repeat}` (amount), note streams for pad, arp, lead and bass, and UI state (`/nooise/state/cursor`, `/layer/<name>` with level and mute flag, `/page_rows`, `/pad_window`, `/steps`).

What reaches foorm: the bridge forwards beat, level, voice levels, kick, chord, phrase, hit, gesture, plus hub-made `/baton/v1/state/build`, `/baton/v1/song/energy`, `/baton/v1/song/beat` (`baton/.../adapters/foorm.rs:28-41`). It drops all `/nooise/state/*` and all note streams.

What foorm draws from: beat, level, chord, kick, voice levels, and four gestures (bloom, submerge, echo, lift) (`foorm/src/osc.rs:152-180`). It ignores every `/nooise/hit/*`, `/nooise/phrase`, and `/nooise/gesture/repeat`.

| Rank | Idea | Exists | To add |
| --- | --- | --- | --- |
| 1 | foorm on a second computer | Everything except a working send | Bridge bind fix, release foorm built on that machine, static wired link (section 1D). Also moves the largest CPU user off the audio laptop |
| 2 | Brightness pulse on temporary events | Bloom, Submerge, Echo, Lift already arrive and are drawn. Beat Repeat arrives and is ignored | Rise (`e`), Perc Roll (`r`) and the `q` Kick+Bass mute are `HoldMute`, not `GestureKind` (`src/fluid/gesture.rs:14-26, 53-59`), and are not broadcast at all. Needs three new addresses in nooise's OSC thread (not the audio thread) and matching cases in foorm. The `q` mute is not visible in `/nooise/state/layer/*` either, because it never changes individual mutes |
| 3 | ASCII step sequencer and arp view | `/nooise/hit/*` with level and pitch class already reaches foorm; `/nooise/beat` gives the grid | A foorm scene that places hits on a 16 step ring by beat position. foorm only, no nooise or baton change. `/nooise/state/steps` is not useful here: it carries only pad slot gates and lead steps, and the bridge drops it |

Honest view: none of these lowers show risk and only rank 1 changes the rig. Ranks 2 and 3 are foorm work that cannot hurt audio, so they are safe to try after the first full rehearsal, and safest once foorm is on its own machine. The nooise half of rank 2 changes the show binary and belongs before the freeze or after the show, not in between.

## 7. End-to-end rehearsal checklist (60 minutes, OBS recording)

Before:

1. Quit agents, builds, browsers. Wi-Fi off. Do Not Disturb on. Charger in. `sudo pmset -a lowpowermode 0`; confirm with `pmset -g | grep lowpowermode`.
2. Connect the output device that will be used at the venue. `nooise --audio-devices` and `mixer devices`: record the exact name and sample rate.
3. `nooise-show --dry-run --profile <venue profile>`: record the printed commands and the nooise binary path.
4. Free disk: at least the launcher's own estimate (13.25 GiB for two hours without events).
5. Start the logger in a spare terminal (one line every 10 seconds):

```
while true; do date +%T; uptime; pmset -g therm | grep -v Note; ps -Ao pcpu,time,rss,comm | grep -E "nooise|mixer|baton|foorm|[Oo][Bb][Ss]|ghostty|plexi|coreaudiod"; sleep 10; done | tee ~/rehearsal-$(date +%F-%H%M).log
```

6. `nooise-show --profile <venue profile>`; wait for `Ready`. Start OBS recording by hand (the launcher leaves it off).

During (play the real set, real duration, hands on):

7. Soundcheck as in section 5, then start from bar 1. Note the wall-clock start.
8. Use every state landing planned for the set, every gesture (`q w e r b`, vamp, push, loop), and one deliberate wrong-state recovery.
9. At 20 and 40 minutes read the mixer's `underruns` / `overruns` line (mixer chain only) and run `nooise-show status` in another terminal.
10. Write down the time of anything heard: click, gap, stutter, UI lag.

After:

11. `nooise-show stop` first, then stop the OBS recording. Do not close panes by hand before `stop` returns (the 2026-11-10 `stop_failed` hypothesis).
12. Collect: session `manifest.json`, `audio/manifest.json`, the logger file, the OBS log for the session, the OBS file.

Pass, all required:

- No audible dropout, click or stall in 60 minutes, and none visible in the recording at the times noted.
- Mixer chain: `dropped_frames` 0, `gaps` empty, `status` `complete` in `audio/manifest.json`; underruns and overruns unchanged between the 20 minute reading and the end.
- `nooise-show status` healthy at both checks; `stop` ends `clean_stop`.
- Logger: no thermal or performance warning from `pmset -g therm`; nooise CPU does not trend upward across the hour; no process RSS grows steadily.
- OBS log: no "lagged frames" or "skipped frames" lines; "Total frames output" within 1% of 30 x seconds recorded.
- foorm still animating at the end; on tier D, still animating on the second computer.
- The opening state, tempo and bank came back exactly on a relaunch.

Fail on any miss. Fix at most three things, then play another full, uninterrupted hour (`2026-11-02-show-readiness.md`, Rehearsal checkpoints).

## Could not verify

- OBS and terminal emulator CPU while the rig runs; OBS was not started.
- Any run past 5.8 minutes, and thermal behaviour over an hour.
- End-to-end latency by loopback; BlackHole's own latency; the venue interface's hardware latency.
- The Scarlett 2i2's device name and sample rate; whether `External Headphones` disappears with nothing in the jack.
- OBS capturing program audio without BlackHole.
- A real two-machine foorm run; the second computer's model and macOS version.
- That the release foorm matches the revision Ian expects on stage.
- Why aggregate and speaker devices are missing from the cpal device list.

This file was added without updating `docs/proposals/AGENTS.md` or TRUTH.md; the brief was read-only apart from this report.
