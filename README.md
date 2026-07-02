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

nooise is one Rust binary: terminal UI, synth engine, and live controls.

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

Press `q` to quit.

```sh
nooise --version
```

## Update

```sh
nooise update
```

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
