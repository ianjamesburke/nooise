<p align="center">
  <img src="https://raw.githubusercontent.com/ianjamesburke/nooise/v1.0.4/assets/nooise-wordmark.svg" alt="nooise" width="760">
</p>

<p align="center">
  Ambient music generator for the terminal.
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/ianjamesburke/nooise/v1.0.4/assets/nooise-preview.png" alt="nooise running in a terminal" width="900">
</p>

I wanted an excuse to build a Rust synth engine. I kept putting on long, repetitive ambient music to get into flow, so I made a small terminal app that does that. 

Playlist: https://www.youtube.com/playlist?list=PLdHqCi9SgnRY


## Install

nooise requires Rust. Install it with rustup:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Open a new terminal after rustup finishes, then install nooise:

```sh
cargo install nooise --locked
```

## Start

```sh
nooise
```

To morph through the built-in songs starting at song 4:

```sh
nooise auto --from 4 --bars 4
```

The loop plays 4 through the last song, wraps to 1, and keeps going.
`nooise --from 4` also starts this loop. `--bars` sets the approximate length
of each song plus its transition, rounded to whole phrases (default 64).

nooise opens on Master, the hub: one row per layer with its level, then
Master's own controls. Use arrows to browse and adjust, Enter to go into a
layer, and Esc to come back out. Tab steps through the layers and back to
the hub.
Press `Ctrl+Q` to quit.

While browsing, hold `q` to mute Kick and Bass, `w` to Submerge the sound,
`e` to Rise, or `r` for a Perc Roll. Release
to stop within 10 ms. Every gesture plays over the whole mix, from any page.
Arrows and Tab keep working while you hold a gesture. These holds require
a terminal that reports key releases; the footer shows when support is missing.
Press `a` to hold the chord that lands on the next bar until you press `a`
again; `Shift+A` or `/auto` toggles auto-morph.

Space is a leader key: a layer key then a parameter (`j` volume, `k` filter)
puts the cursor on that control, where `h`/`l` move it. The layer keys read
down the hub's layer rows, `asdf` then `qwer`, so `a` is Pads and `r` is
Master. Skip the layer key to aim at the page you are already on, so
`Space k` is the filter on whatever is in front of you. It jumps, it does
not edit.

Press `Shift+P` to stop the clock and let the song end on its tails: nothing
new plays, and reverb, delay, and releasing notes ring out. Press `Shift+P`
again to start.

See [live performance](docs/PERFORMANCE.md) for overlap, saving, the clock
stop, and the Jump leader.

```sh
nooise --osc
nooise --osc --osc-in
```

`--osc` mirrors the beat, chord changes, Pad phrase position, every voice's
level, every musical hit, voiced Pad/Arp/Lead/Bass notes, and live UI/layer
state as OSC over UDP to `127.0.0.1:9000`, where
[foorm](https://github.com/ianjamesburke/foorm) listens by default.
`--osc=ADDR` sends elsewhere, such as TouchDesigner's OSC In CHOP or an
external controller/MIDI bridge. `--osc-in` (default `127.0.0.1:9001`, or
`--osc-in=ADDR`) listens for inbound `/nooise/cmd/*` OSC messages to drive
controls, navigation, mutes, performance gestures, transport, Motion Grab, and
voice notes. Both are off unless asked for. See `src/fluid/osc.rs` for the
address vocabulary.

At each Pad chord boundary, `/nooise/phrase` sends three integers: the
zero-based phrase cycle since the last reset, the zero-based chord position
within the sounding chord window, and that window's chord count. An empty
`/nooise/phrase/reset` arrives first on initial play, clock restart, a new
progression, or an auto-morph landing. Both messages are opt-in with `--osc`.

Add `pad.trigger` with `/`, then change Trigger from Hold to Stabs for a 16-step chord rhythm.
Enter on Trigger opens the Hit/Rest steps. Up/Down selects, Left/Right changes,
and Esc returns to the Pads page. Each step is a sixteenth note. Add the Swing
module to Pads with `/` to delay the offbeat steps. Stabs follow the Pad's Attack
and Release, use the current progression, and send the same chord hits to audio
and OSC. Gate sets how many beats each hit stays held, from 0.125 to
2, with a 0.5-beat default. The next hit retriggers the chord even if Gate has
not elapsed. Hold restores the normal sustained Pad.

Pads' Chord Notes row sets how many notes each Pad chord plays in both audio
and OSC. Two keeps the root and fifth; three plays the triad; four keeps the
original voicing; five adds a high root. The default is four.
Arp, Bass, and Lead still follow the original chord tones.

```sh
nooise --version
```

The app checks crates.io at most once every 24 hours while running and shows a
small update message when a newer release is available.

## Update

```sh
nooise update
```

Checks crates.io and only reinstalls when a newer release exists.
`nooise upgrade` does the same thing.

## Render to a File

```sh
nooise render --seconds 60 --out ambient.wav
```

Renders the default mix straight to a wav, no audio device needed. Pass `--seed N` to make the render reproducible.

## From Source

```sh
cargo run
```
