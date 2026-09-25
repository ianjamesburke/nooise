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

List the exact input and output port names, then choose one device for both
directions or name them separately:

```sh
nooise midi-ports
nooise --midi Take5
nooise --midi-in Keyboard --midi-out Take5
nooise --midi-out Take5
```

Input and output each default to channel 1. Use `--midi-in-channel N` and
`--midi-out-channel N` to choose different channels (1–16). `--midi Take5`
opens both directions; either directional flag overrides its side. With Pad
MIDI Out on, nooise sends the Pad's current
2–5-note chord at startup and on chord changes, plus 24 MIDI clock pulses per
beat and the transport's Start, Stop, and Continue messages. The Pad chord
releases when another chord starts or the clock stops. MIDI output runs
alongside nooise audio. With `--midi` or `--midi-out`, Pad starts with MIDI Out
on and visible, MIDI In off, and Level at 0%, including when loading a song. `--midi`
still opens the input port for tracks whose MIDI In switch you turn on. An
input-only `--midi-in` launch starts Pad with MIDI In on and visible, MIDI Out off, and
Level at 0%; raise Pad Level to hear your keyboard through its sound. The
other tracks keep their saved MIDI switches. MIDI launches tuck away Pad
Trigger, Swing, and Gate. Find any of those rows with `/` to add it back.
Set the receiving synth to follow USB MIDI clock if you want its own timed
features to sync.

On the Pads page, change Trigger from Hold to Stabs for a 16-step chord rhythm.
Enter on Trigger opens the Hit/Rest steps. Up/Down selects, Left/Right changes,
and Esc returns to the Pads page. Each step is a sixteenth note. Swing delays the
offbeat steps. Stabs use the current progression and send the same chord hits
to audio and MIDI. Gate sets how many beats each hit stays held, from 0.125 to
2, with a 0.5-beat default. The next hit retriggers the chord even if Gate has
not elapsed. MIDI Note Off starts the external synth's own release envelope;
nooise does not change its ADSR. Hold restores the normal sustained Pad. Inside
Trigger, turn MIDI Trigger On to stop the step hits and fire the current chord
from incoming MIDI Note On messages. The step rows dim while MIDI Trigger is
On; Gate still sets the chord's hold time.

Other MIDI switches stay hidden from the Pads, Arp, and Lead pages until you
add one with `/`: find `pad.midi_in`, `arp.midi_out`, or another track's switch
and press Enter. The switch then stays at the bottom of that page even when Off;
Arp and Lead also show MIDI Gate when their Out switch is added. Adding a
switch does not change its On/Off value. A track can use only one direction at a time: turning In on
turns Out off, and vice versa. Pad defaults to Out; Arp and Lead default Off.
Ordinary Pad MIDI In plays incoming pitches with the Pad sound. With a
connected input, Hold-mode Pad audio follows your keyboard. Held Arp input
notes replace its Pad-chord note source until released. Lead input notes play
its sound at their incoming pitch. MIDI input never echoes notes to output.
More than one track can send MIDI, sharing the chosen output channel and the
synth's available voices. MIDI Out never mutes nooise audio.
Arp and Lead play through MIDI even with their audio Level at zero. Their MIDI
Gate rows set the held length of step notes from 0.125 to 2 beats (default 0.5);
a held Lead play key stays on until released.

Pads' Chord Notes row sets how many notes each Pad chord plays in both audio
and MIDI. Two keeps the root and fifth; three plays the triad; four keeps the
original voicing; five adds a high root. The default is four. For a Take 5
patch with a long release, try two or three to leave voices for the next chord.
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
