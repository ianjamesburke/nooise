"""exp10_pad_fog — Pad Fog (XY Space + Atmosphere)

A non-functional visual prototype for a generative focus-music engine.
No audio. Instruments float as glowing labeled points inside a colored fog
rendered with Unicode half-blocks. Fog hue/density emerge from the combined
positions of nearby instruments. Points drift autonomously and can be dragged.

Controls:
    mouse drag  reposition an instrument (it resumes drifting on release)
    q           quit
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass, field

from rich.segment import Segment
from rich.style import Style
from textual.app import App, ComposeResult
from textual.geometry import Offset
from textual.strip import Strip
from textual.widget import Widget

# deep ocean base the fog sits on
BASE = (6, 9, 22)

# instrument hues: ocean blues, purples, teal highlights
INSTRUMENTS = [
    ("PADS", (70, 110, 235)),    # deep blue
    ("BASS", (120, 70, 220)),    # violet
    ("KEYS", (40, 200, 195)),    # teal
    ("PERC", (150, 90, 245)),    # purple
    ("AIR", (60, 170, 255)),     # sky / cyan
]

GLYPH = "◉"


@dataclass
class Point:
    name: str
    color: tuple[int, int, int]
    x: float           # normalized 0..1
    y: float           # normalized 0..1
    vx: float = 0.0
    vy: float = 0.0
    held: bool = False
    phase: float = field(default_factory=lambda: random.uniform(0, math.tau))

    def wander(self, dt: float) -> None:
        if self.held:
            return
        # slow random-walk acceleration
        self.vx += random.uniform(-1, 1) * 0.06 * dt
        self.vy += random.uniform(-1, 1) * 0.06 * dt
        # damping keeps drift gentle
        self.vx *= 0.92
        self.vy *= 0.92
        self.x += self.vx * dt
        self.y += self.vy * dt
        # soft bounce off the edges
        if self.x < 0.04:
            self.x = 0.04
            self.vx = abs(self.vx)
        elif self.x > 0.96:
            self.x = 0.96
            self.vx = -abs(self.vx)
        if self.y < 0.04:
            self.y = 0.04
            self.vy = abs(self.vy)
        elif self.y > 0.96:
            self.y = 0.96
            self.vy = -abs(self.vy)
        self.phase += dt * 2.0


class FogCanvas(Widget):
    """Full-screen canvas: atmospheric fog + drifting instrument points."""

    DEFAULT_CSS = """
    FogCanvas {
        width: 1fr;
        height: 1fr;
    }
    """

    SIGMA = 0.26          # fog blob radius (normalized)
    INTENSITY = 1.35      # how strongly an instrument colors nearby fog

    def __init__(self) -> None:
        super().__init__()
        self.points: list[Point] = [
            Point(name, color, random.uniform(0.2, 0.8), random.uniform(0.2, 0.8))
            for name, color in INSTRUMENTS
        ]
        self._dragging: Point | None = None
        self.time = 0.0

    def on_mount(self) -> None:
        self.set_interval(1 / 30, self._tick)

    def _tick(self) -> None:
        self.time += 1 / 30
        for p in self.points:
            p.wander(1 / 30)
        self.refresh()

    # --- fog field -----------------------------------------------------

    def _fog_color(self, nx: float, ny: float, aspect: float) -> tuple[int, int, int]:
        """Blend instrument colors into the base fog at a normalized point."""
        r, g, b = BASE
        for p in self.points:
            dx = (nx - p.x) * aspect
            dy = ny - p.y
            d2 = dx * dx + dy * dy
            w = math.exp(-d2 / (2 * self.SIGMA * self.SIGMA))
            # gentle breathing so the fog never feels static
            w *= 0.85 + 0.15 * math.sin(self.time * 0.8 + p.phase)
            w *= self.INTENSITY
            r += p.color[0] * w
            g += p.color[1] * w
            b += p.color[2] * w
        return (min(255, int(r)), min(255, int(g)), min(255, int(b)))

    # --- rendering -----------------------------------------------------

    def render_line(self, y: int) -> Strip:
        width = self.size.width
        height = self.size.height
        if width == 0 or height == 0:
            return Strip([])

        # square-ish pixels: half-blocks double vertical resolution
        sub_h = height * 2
        aspect = sub_h / max(width, 1)

        # instrument overlays for this row: col -> (char, fg, fg_is_glyph)
        overlay: dict[int, tuple[str, tuple[int, int, int], bool]] = {}
        for p in self.points:
            cy = int(round(p.y * (height - 1)))
            if cy != y:
                continue
            cx = int(round(p.x * (width - 1)))
            # twinkle the glyph brightness a touch
            tw = 0.75 + 0.25 * math.sin(self.time * 3 + p.phase)
            glow = (min(255, int(p.color[0] * tw + 60)), min(255, int(p.color[1] * tw + 60)), min(255, int(p.color[2] * tw + 60)))
            overlay[cx] = (GLYPH, glow, True)
            # tiny label trailing the point
            label = f" {p.name}"
            dim = (min(255, int(p.color[0] * 0.55 + 30)), min(255, int(p.color[1] * 0.55 + 30)), min(255, int(p.color[2] * 0.55 + 30)))
            for i, ch in enumerate(label):
                lx = cx + 1 + i
                if 0 <= lx < width and lx not in overlay:
                    overlay[lx] = (ch, dim, False)

        top_row = y * 2
        bot_row = y * 2 + 1
        segments: list[Segment] = []
        for x in range(width):
            nx = (x + 0.5) / width
            bot = self._fog_color(nx, (bot_row + 0.5) / sub_h, aspect)

            if x in overlay:
                ch, fg, _is_glyph = overlay[x]
                segments.append(
                    Segment(ch, Style(color=f"rgb({fg[0]},{fg[1]},{fg[2]})",
                                      bgcolor=f"rgb({bot[0]},{bot[1]},{bot[2]})"))
                )
                continue

            top = self._fog_color(nx, (top_row + 0.5) / sub_h, aspect)
            segments.append(
                Segment("▀", Style(color=f"rgb({top[0]},{top[1]},{top[2]})",
                                   bgcolor=f"rgb({bot[0]},{bot[1]},{bot[2]})"))
            )
        return Strip(segments)

    # --- mouse ---------------------------------------------------------

    def _local_to_norm(self, offset: Offset) -> tuple[float, float]:
        w = max(self.size.width - 1, 1)
        h = max(self.size.height - 1, 1)
        return (offset.x / w, offset.y / h)

    def on_mouse_down(self, event) -> None:
        nx, ny = self._local_to_norm(event.offset)
        aspect = (self.size.height * 2) / max(self.size.width, 1)
        # nearest instrument within a grab radius
        best: Point | None = None
        best_d = 0.18
        for p in self.points:
            dx = (nx - p.x) * aspect
            dy = ny - p.y
            d = math.hypot(dx, dy)
            if d < best_d:
                best_d = d
                best = p
        if best is not None:
            self._dragging = best
            best.held = True
            best.vx = best.vy = 0.0
            self.capture_mouse()

    def on_mouse_move(self, event) -> None:
        if self._dragging is None:
            return
        nx, ny = self._local_to_norm(event.offset)
        self._dragging.x = min(0.96, max(0.04, nx))
        self._dragging.y = min(0.96, max(0.04, ny))

    def on_mouse_up(self, event) -> None:
        if self._dragging is not None:
            self._dragging.held = False
            self._dragging = None
            self.release_mouse()


class PadFogApp(App):
    CSS = """
    Screen { background: rgb(6,9,22); }
    """
    BINDINGS = [("q", "quit", "Quit")]

    def compose(self) -> ComposeResult:
        yield FogCanvas()


if __name__ == "__main__":
    PadFogApp().run()
