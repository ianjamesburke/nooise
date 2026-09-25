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

Use arrows to browse and adjust controls, and Tab to move between layers.
Press `Ctrl+Q` to quit.

While browsing, hold `z` for a reverb Bloom, `c` to Submerge the sound,
`v` for Echo, or `x` to Lift its low end out. Release
to stop within 10 ms. Every gesture plays over the whole mix, from any page.
Arrows and Tab keep working while you hold a gesture. These holds require
a terminal that reports key releases; the footer shows when support is missing.

Space is a leader key: a layer key then a parameter (`j` volume, `k` filter)
puts the cursor on that control, where `h`/`l` move it. The layer keys read
left to right across the tab strip, `asdf` then `qwer`, so `a` is Pads and
`r` is Master. Skip the layer key to aim at the page you are already on, so
`Space k` is the filter on whatever is in front of you. It jumps, it does
not edit.

Press `Shift+P` to stop the clock and let the song end on its tails: nothing
new plays, and reverb, delay, and releasing notes ring out. Press `Shift+P`
again to start.

See [live performance](docs/PERFORMANCE.md) for overlap, saving, the clock
stop, and the Jump leader.

```sh
nooise --osc
```

Mirrors the beat, chord changes, every voice's level, every musical hit, and
every held gesture as OSC over UDP to
`127.0.0.1:9000`, where [foorm](https://github.com/ianjamesburke/foorm)
listens by default. `--osc=ADDR` sends elsewhere, such as TouchDesigner's
OSC In CHOP. Off unless asked for. See `src/fluid/osc.rs` for the address
vocabulary.

For a connected MIDI synth, list its output port name and start nooise with
that exact name:

```sh
nooise midi-ports
nooise --midi-out Take5
```

MIDI output uses channel 1. It sends the Pad's current four-note chord at
startup and on chord changes, plus 24 MIDI clock pulses per beat and the
transport's Start, Stop, and Continue messages. The Pad chord releases when
another chord starts or the clock stops. MIDI output runs alongside nooise
audio; set the Pad Level to zero to hear only the external synth for that
layer. `--midi-out` also works with a song number, song code, or `auto`.
Set the receiving synth to follow USB MIDI clock if you want its own timed
features to sync.

On the Pads page, change Trigger from Hold to Stabs for a 16-step chord rhythm.
Enter on Trigger opens the Hit/Rest steps. Up/Down selects, Left/Right changes,
and Esc returns to the Pads page. Each step is a sixteenth note. Swing delays the
offbeat steps. Stabs use the current progression and send the same short chord
hits to audio and MIDI, with note-offs between hits. Hold restores the normal
sustained Pad.

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
