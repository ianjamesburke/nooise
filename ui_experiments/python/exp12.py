"""exp12 - Envelope Garden (Living Shapes)

A non-functional UI prototype for a generative focus-music engine.

Each instrument owns a living organic shape: a smooth curve drawn in braille
that represents its current envelope. The curve is defined by a handful of
control points that drift autonomously in slow random walks, so the shape
continuously morphs. The whole shape "breathes" (scales) at its own rate.

Mouse:
  - Drag a control point (the ◆ markers) to bias the shape; it resumes
    drifting on its own once you let go.
  - Click empty space in a panel to expand it to a full-screen detail view.
  - Click the detail view (or press Esc) to return to the garden.

No audio. Visual only. Press 'q' to quit.
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass, field

from rich.text import Text
from textual.app import App, ComposeResult
from textual.containers import Vertical
from textual.widget import Widget
from textual.widgets import Static


TICK = 1.0 / 30.0          # animation refresh (~30fps)
BG = (7, 8, 12)            # near-black canvas
GRAB_DX = 2                # hit radius (cells) horizontally for a control point
GRAB_DY = 1                # hit radius (cells) vertically

# braille dot -> bit value, indexed by (col 0..1, row 0..3)
_BRAILLE = {
    (0, 0): 0x01, (0, 1): 0x02, (0, 2): 0x04, (0, 3): 0x40,
    (1, 0): 0x08, (1, 1): 0x10, (1, 2): 0x20, (1, 3): 0x80,
}


@dataclass(frozen=True)
class Instrument:
    name: str
    color: tuple[int, int, int]
    breath_rate: float       # radians/sec of the breathing oscillation
    breath_amp: float        # fractional scale of the breathing


INSTRUMENTS: list[Instrument] = [
    Instrument("TONAL BED", (45, 212, 191), 0.55, 0.14),       # teal
    Instrument("BILATERAL PULSE", (167, 139, 250), 1.30, 0.10),  # purple
    Instrument("NOISE TEXTURE", (251, 191, 36), 2.10, 0.07),   # amber
    Instrument("KICK", (244, 164, 194), 0.85, 0.18),           # soft pink
]


def _blend(a: tuple[int, int, int], b: tuple[int, int, int], t: float) -> str:
    t = max(0.0, min(1.0, t))
    r = int(a[0] + (b[0] - a[0]) * t)
    g = int(a[1] + (b[1] - a[1]) * t)
    bl = int(a[2] + (b[2] - a[2]) * t)
    return f"#{r:02x}{g:02x}{bl:02x}"


def _catmull(y0: float, y1: float, y2: float, y3: float, t: float) -> float:
    t2 = t * t
    t3 = t2 * t
    return 0.5 * (
        (2 * y1)
        + (-y0 + y2) * t
        + (2 * y0 - 5 * y1 + 4 * y2 - y3) * t2
        + (-y0 + 3 * y1 - 3 * y2 + y3) * t3
    )


class Braille:
    """A braille dot canvas sized in character cells (2x4 dots per cell)."""

    def __init__(self, w: int, h: int) -> None:
        self.w = w
        self.h = h
        self.dw = w * 2
        self.dh = h * 4
        self.cells = [[0] * w for _ in range(h)]

    def set(self, x: int, y: int) -> None:
        if 0 <= x < self.dw and 0 <= y < self.dh:
            self.cells[y // 4][x // 2] |= _BRAILLE[(x % 2, y % 4)]

    def char(self, cx: int, cy: int) -> str:
        v = self.cells[cy][cx]
        return chr(0x2800 + v) if v else " "


@dataclass
class Point:
    x: float
    y: float
    vx: float = 0.0
    vy: float = 0.0


@dataclass
class ShapeModel:
    """Autonomous control points + breathing phase for one instrument."""

    instrument: Instrument
    points: list[Point] = field(default_factory=list)
    phase: float = 0.0
    held: int | None = None  # index of a point currently grabbed (frozen drift)

    def __post_init__(self) -> None:
        rng = random.Random(hash(self.instrument.name) & 0xFFFF)
        n = 5
        for i in range(n):
            x = i / (n - 1)
            y = 0.5 + (rng.random() - 0.5) * 0.5
            self.points.append(Point(x, y))
        self.phase = rng.random() * math.tau
        self._rng = rng

    def scale(self) -> float:
        i = self.instrument
        return 1.0 + i.breath_amp * math.sin(self.phase)

    def update(self, dt: float) -> None:
        self.phase += self.instrument.breath_rate * dt
        rng = self._rng
        for idx, p in enumerate(self.points):
            if idx == self.held:
                continue
            # Ornstein-Uhlenbeck-ish smooth random walk on velocity
            p.vx = p.vx * 0.90 + rng.gauss(0, 1) * 0.0035
            p.vy = p.vy * 0.90 + rng.gauss(0, 1) * 0.0045
            p.x += p.vx
            p.y += p.vy
            if p.x < 0.0:
                p.x, p.vx = 0.0, -p.vx
            elif p.x > 1.0:
                p.x, p.vx = 1.0, -p.vx
            if p.y < 0.08:
                p.y, p.vy = 0.08, -p.vy
            elif p.y > 0.92:
                p.y, p.vy = 0.92, -p.vy

    def sorted_points(self) -> list[Point]:
        return sorted(self.points, key=lambda p: p.x)

    def eval_y(self, xn: float) -> float:
        """Breathing-applied curve height (0..1) at normalized x."""
        pts = self.sorted_points()
        xs = [p.x for p in pts]
        ys = [p.y for p in pts]
        for i in range(1, len(xs)):
            if xs[i] <= xs[i - 1]:
                xs[i] = xs[i - 1] + 1e-3
        if xn <= xs[0]:
            y = ys[0]
        elif xn >= xs[-1]:
            y = ys[-1]
        else:
            i = 0
            while i < len(xs) - 1 and not (xs[i] <= xn <= xs[i + 1]):
                i += 1
            t = (xn - xs[i]) / (xs[i + 1] - xs[i])
            y0 = ys[max(0, i - 1)]
            y1 = ys[i]
            y2 = ys[i + 1]
            y3 = ys[min(len(ys) - 1, i + 2)]
            y = _catmull(y0, y1, y2, y3, t)
        s = self.scale()
        return 0.5 + (y - 0.5) * s


class ShapeView(Widget):
    """Renders a ShapeModel as a glowing braille curve. Handles mouse."""

    def __init__(self, model: ShapeModel, role: str) -> None:
        super().__init__()
        self.model = model
        self.role = role  # "grid" or "detail"
        self._drag: int | None = None
        self._moved = False

    def set_model(self, model: ShapeModel) -> None:
        self.model = model
        self.refresh()

    # ----- rendering -------------------------------------------------------
    def render(self) -> Text:
        w = self.size.width
        h = self.size.height
        if w < 2 or h < 1:
            return Text("")

        m = self.model
        canvas = Braille(w, h)

        prev: int | None = None
        for dx in range(canvas.dw):
            xn = dx / (canvas.dw - 1)
            yn = m.eval_y(xn)
            dy = int(round(yn * (canvas.dh - 1)))
            if prev is None:
                canvas.set(dx, dy)
            else:
                lo, hi = min(prev, dy), max(prev, dy)
                for yy in range(lo, hi + 1):
                    canvas.set(dx, yy)
            prev = dy

        # breathing-driven glow brightness
        glow = 0.30 + 0.30 * (0.5 + 0.5 * math.sin(m.phase))
        line_color = _blend(m.instrument.color, (255, 255, 255), glow * 0.35)
        dim_color = _blend(BG, m.instrument.color, 0.30)

        # control-point markers overlaid on their cell
        s = m.scale()
        markers: dict[tuple[int, int], tuple[str, str]] = {}
        for idx, p in enumerate(m.points):
            yb = 0.5 + (p.y - 0.5) * s
            col = int(round(p.x * (w - 1)))
            row = int(round(yb * (h - 1)))
            if 0 <= col < w and 0 <= row < h:
                if idx == m.held:
                    markers[(col, row)] = ("✦", _blend(m.instrument.color, (255, 255, 255), 0.7))
                else:
                    markers[(col, row)] = ("◆", _blend(m.instrument.color, (255, 255, 255), 0.25))

        out = Text()
        for cy in range(h):
            for cx in range(w):
                marker = markers.get((cx, cy))
                if marker is not None:
                    out.append(marker[0], style=f"bold {marker[1]}")
                    continue
                ch = canvas.char(cx, cy)
                if ch == " ":
                    out.append(" ")
                else:
                    out.append(ch, style=line_color)
            if cy != h - 1:
                out.append("\n")
        return out

    # ----- mouse -----------------------------------------------------------
    def _hit_point(self, ox: int, oy: int) -> int | None:
        w = self.size.width
        h = self.size.height
        if w < 2 or h < 1:
            return None
        s = self.model.scale()
        best: int | None = None
        best_d = 1e9
        for idx, p in enumerate(self.model.points):
            yb = 0.5 + (p.y - 0.5) * s
            col = p.x * (w - 1)
            row = yb * (h - 1)
            if abs(col - ox) <= GRAB_DX and abs(row - oy) <= GRAB_DY:
                d = (col - ox) ** 2 + (row - oy) ** 2
                if d < best_d:
                    best_d = d
                    best = idx
        return best

    def on_mouse_down(self, event) -> None:
        self._moved = False
        idx = self._hit_point(event.offset.x, event.offset.y)
        self._drag = idx
        if idx is not None:
            self.model.held = idx
            self.capture_mouse()

    def on_mouse_move(self, event) -> None:
        if self._drag is None:
            return
        w = self.size.width
        h = self.size.height
        if w < 2 or h < 1:
            return
        self._moved = True
        s = self.model.scale() or 1.0
        p = self.model.points[self._drag]
        nx = max(0.0, min(1.0, event.offset.x / (w - 1)))
        yb = event.offset.y / (h - 1)
        base = 0.5 + (yb - 0.5) / s
        p.x = nx
        p.y = max(0.08, min(0.92, base))
        p.vx = 0.0
        p.vy = 0.0
        self.refresh()

    def on_mouse_up(self, event) -> None:
        was_drag = self._drag
        self._drag = None
        self.model.held = None
        if self.app.mouse_captured is self:
            self.release_mouse()
        # an empty-space tap (not a point grab) toggles the detail view
        if was_drag is None:
            if self.role == "grid":
                self.app.expand(self.model)
            else:
                self.app.collapse()


class EnvelopeGarden(App):
    CSS = """
    Screen {
        background: #07080c;
    }

    #grid {
        layout: grid;
        grid-size: 2 2;
        grid-gutter: 1 1;
        padding: 1 1;
        width: 1fr;
        height: 1fr;
    }

    .panel {
        width: 1fr;
        height: 1fr;
        border: round #1b1f2b;
        background: #0a0c12;
    }

    .panel-label {
        height: 1;
        text-style: bold;
        padding: 0 1;
    }

    ShapeView {
        width: 1fr;
        height: 1fr;
    }

    #detail {
        display: none;
        width: 1fr;
        height: 1fr;
        padding: 1 2;
    }

    #detail.shown {
        display: block;
    }

    #detail-label {
        height: 1;
        text-style: bold;
        padding: 0 1;
    }

    #detail-hint {
        height: 1;
        color: #3d4252;
        padding: 0 1;
    }

    #footer {
        dock: bottom;
        height: 1;
        content-align: center middle;
        color: #3d4252;
    }
    """

    def compose(self) -> ComposeResult:
        self._models = [ShapeModel(inst) for inst in INSTRUMENTS]
        self._grid_views: list[ShapeView] = []

        with Vertical(id="grid"):
            for model in self._models:
                with Vertical(classes="panel") as panel:
                    panel._inst = model.instrument  # type: ignore[attr-defined]
                    label = Static(model.instrument.name, classes="panel-label")
                    label._inst = model.instrument  # type: ignore[attr-defined]
                    yield label
                    view = ShapeView(model, role="grid")
                    self._grid_views.append(view)
                    yield view

        with Vertical(id="detail"):
            self._detail_label = Static("", id="detail-label")
            yield self._detail_label
            self._detail_view = ShapeView(self._models[0], role="detail")
            yield self._detail_view
            yield Static("click or press Esc to return to the garden", id="detail-hint")

        yield Static(
            "N O O I S E   ·   envelope garden   ·   drag ◆ to bias   ·   click to expand   ·   q to quit",
            id="footer",
        )

    def on_mount(self) -> None:
        # tint each panel border + label in its instrument colour
        for panel in self.query(".panel"):
            inst: Instrument = panel._inst  # type: ignore[attr-defined]
            panel.styles.border = ("round", _blend(BG, inst.color, 0.45))
        for label in self.query(".panel-label"):
            inst = label._inst  # type: ignore[attr-defined]
            label.styles.color = _blend(inst.color, (255, 255, 255), 0.2)

        self._expanded = False
        self.set_interval(TICK, self._tick)

    def _tick(self) -> None:
        for m in self._models:
            m.update(TICK)
        if self._expanded:
            self._detail_view.refresh()
        else:
            for v in self._grid_views:
                v.refresh()

    def expand(self, model: ShapeModel) -> None:
        if self._expanded:
            return
        self._expanded = True
        inst = model.instrument
        self._detail_view.set_model(model)
        self._detail_label.update(inst.name)
        self._detail_label.styles.color = _blend(inst.color, (255, 255, 255), 0.2)
        detail = self.query_one("#detail")
        detail.styles.border = ("round", _blend(BG, inst.color, 0.45))
        detail.add_class("shown")
        self.query_one("#grid").styles.display = "none"

    def collapse(self) -> None:
        if not self._expanded:
            return
        self._expanded = False
        self.query_one("#detail").remove_class("shown")
        self.query_one("#grid").styles.display = "block"

    def on_key(self, event) -> None:
        if event.key == "q":
            self.exit()
        elif event.key == "escape":
            if self._expanded:
                self.collapse()
            else:
                self.exit()


if __name__ == "__main__":
    EnvelopeGarden().run()
