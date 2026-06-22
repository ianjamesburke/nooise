"""exp08 - Phrase Grid

A non-functional UI prototype for a generative focus-music engine.

Observation only: watch the engine "think" from above, like a music box
mechanism. Four instrument rows (Tonal, Pulse, Noise, Kick) play across a
16-step grid. Each instrument runs a different pattern length, so the
voices drift in and out of polyrhythmic alignment. A playhead sweeps left
to right at ~120 BPM. Cells flare in their instrument colour when a note
triggers and fade out over ~1 second.

No audio. No editing. Press 'q' to quit.
"""

from __future__ import annotations

from dataclasses import dataclass

from textual.app import App, ComposeResult
from textual.containers import Horizontal, Vertical
from textual.widgets import Static


STEPS = 16
STEP_INTERVAL = 0.5  # seconds per step -> 120 BPM
FADE_TIME = 1.0      # seconds for a triggered cell to fully decay
TICK = 1.0 / 30.0    # animation refresh (~30fps)


@dataclass(frozen=True)
class Instrument:
    name: str
    color: tuple[int, int, int]
    length: int          # pattern length in steps (polyrhythm)
    pattern: tuple[int, ...]  # which positions within `length` trigger


INSTRUMENTS: list[Instrument] = [
    Instrument("TONAL", (45, 212, 191), 13, (0, 3, 5, 8, 11)),   # teal
    Instrument("PULSE", (167, 139, 250), 11, (0, 2, 4, 6, 9)),   # purple
    Instrument("NOISE", (251, 191, 36), 7, (0, 3, 5)),           # amber
    Instrument("KICK", (244, 164, 194), 16, (0, 4, 8, 12)),      # soft pink
]


def _blend(color: tuple[int, int, int], intensity: float) -> str:
    """Mix an instrument colour toward the dark background by `intensity` (0..1)."""
    bg = (16, 18, 24)
    r = int(bg[0] + (color[0] - bg[0]) * intensity)
    g = int(bg[1] + (color[1] - bg[1]) * intensity)
    b = int(bg[2] + (color[2] - bg[2]) * intensity)
    return f"#{r:02x}{g:02x}{b:02x}"


class Cell(Static):
    """A single grid cell that glows and fades."""

    def __init__(self, instrument: Instrument) -> None:
        super().__init__("")
        self._instrument = instrument
        self._energy = 0.0          # 0..1 current glow
        self._under_playhead = False

    def trigger(self) -> None:
        self._energy = 1.0

    def decay(self, dt: float) -> None:
        if self._energy > 0.0:
            self._energy = max(0.0, self._energy - dt / FADE_TIME)
            self.refresh_color()

    def set_playhead(self, on: bool) -> None:
        if on != self._under_playhead:
            self._under_playhead = on
            self.refresh_color()

    def refresh_color(self) -> None:
        intensity = self._energy
        if self._under_playhead:
            # lift the floor under the playhead so the column reads as "now"
            intensity = max(intensity, 0.18)
        if intensity <= 0.001:
            self.styles.background = "#10121a"
        else:
            self.styles.background = _blend(self._instrument.color, intensity)


class RowLabel(Static):
    pass


class PhraseGrid(App):
    CSS = """
    Screen {
        background: #0a0b10;
        align: center middle;
    }

    #frame {
        width: auto;
        height: auto;
        padding: 2 3;
        background: #0d0f16;
        border: round #232838;
    }

    #title {
        width: 100%;
        content-align: center middle;
        color: #5b6478;
        text-style: bold;
        margin-bottom: 1;
    }

    .grid-row {
        height: 3;
        width: auto;
    }

    RowLabel {
        width: 8;
        height: 3;
        content-align: right middle;
        color: #8b93a7;
        text-style: bold;
        padding-right: 1;
    }

    Cell {
        width: 5;
        height: 3;
        margin: 0 1;
        background: #10121a;
        border: round #1b1f2b;
    }

    #footer {
        width: 100%;
        content-align: center middle;
        color: #3d4252;
        margin-top: 1;
    }
    """

    def compose(self) -> ComposeResult:
        with Vertical(id="frame"):
            yield Static("N O O I S E   ·   phrase grid", id="title")
            for inst in INSTRUMENTS:
                with Horizontal(classes="grid-row"):
                    yield RowLabel(inst.name)
                    for _ in range(STEPS):
                        yield Cell(inst)
            yield Static("120 BPM   ·   polyrhythmic   ·   press q to quit", id="footer")

    def on_mount(self) -> None:
        # cells[row][col]
        self._cells: list[list[Cell]] = []
        rows = self.query(".grid-row").nodes
        for row in rows:
            self._cells.append(list(row.query(Cell).nodes))

        self._playhead = -1
        self._accum = 0.0
        self.set_interval(TICK, self._tick)
        self._advance_playhead()

    def _tick(self) -> None:
        self._accum += TICK
        for row in self._cells:
            for cell in row:
                cell.decay(TICK)
        if self._accum >= STEP_INTERVAL:
            self._accum -= STEP_INTERVAL
            self._advance_playhead()

    def _advance_playhead(self) -> None:
        prev = self._playhead
        self._playhead = (self._playhead + 1) % STEPS

        for row in self._cells:
            if 0 <= prev < STEPS:
                row[prev].set_playhead(False)
            row[self._playhead].set_playhead(True)

        # fire notes whose polyrhythmic pattern lands on this absolute step
        step = self._playhead
        for inst, row in zip(INSTRUMENTS, self._cells):
            if (step % inst.length) in inst.pattern:
                row[step].trigger()
                row[step].refresh_color()

    def on_key(self, event) -> None:
        if event.key in ("q", "escape"):
            self.exit()


if __name__ == "__main__":
    PhraseGrid().run()
