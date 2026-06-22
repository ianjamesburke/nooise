"""exp09 — Spatial Web (Observable + Nudgeable)

Non-functional visual prototype for a generative focus-music engine.
Instruments are nodes on a 2D canvas connected by dependency lines. Nodes drift
on their own via slow random walks; position encodes attributes (X = brightness/
density, Y = tempo/rate). Connection lines pulse brighter as their endpoints draw
closer. Grab a node with the mouse to nudge it, then watch it resume drifting.

No audio. Press 'q' to quit.

    uv run --with textual exp09_spatial_web.py
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass, field

from rich.style import Style
from rich.text import Text
from textual.app import App, ComposeResult
from textual.events import MouseDown, MouseMove, MouseUp
from textual.geometry import Size
from textual.widget import Widget
from textual.widgets import Footer, Static


# Terminal cells are roughly twice as tall as they are wide. Scale Y distances so
# the web feels spatially even rather than vertically stretched.
CELL_ASPECT = 0.5

# Closeness at or below this normalized distance counts as "fully connected".
NEAR = 0.12
FAR = 0.75


@dataclass
class Node:
    name: str
    short: str
    color: str
    # Normalized position in [0, 1] for both axes.
    x: float
    y: float
    # Random-walk velocity, also normalized per tick.
    vx: float = 0.0
    vy: float = 0.0
    phase: float = field(default_factory=lambda: random.uniform(0, math.tau))


NODES: list[Node] = [
    Node("Tonal Bed", "BED", "#4ec9b0", 0.30, 0.35),
    Node("Bilateral Pulse", "PLS", "#b48ead", 0.68, 0.28),
    Node("Noise Texture", "NSE", "#e0af68", 0.50, 0.66),
    Node("Kick", "KCK", "#7aa2f7", 0.78, 0.70),
    Node("Pad", "PAD", "#f7a8b8", 0.22, 0.74),
]

# Dependency graph between instruments.
EDGES: list[tuple[int, int]] = [
    (0, 1),  # Bed   -> Pulse
    (0, 2),  # Bed   -> Noise
    (0, 4),  # Bed   -> Pad
    (1, 3),  # Pulse -> Kick
    (2, 3),  # Noise -> Kick
    (2, 4),  # Noise -> Pad
    (3, 4),  # Kick  -> Pad
]


def _hex_to_rgb(h: str) -> tuple[int, int, int]:
    h = h.lstrip("#")
    return int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)


def _blend(a: tuple[int, int, int], b: tuple[int, int, int], t: float) -> tuple[int, int, int]:
    return (
        round(a[0] + (b[0] - a[0]) * t),
        round(a[1] + (b[1] - a[1]) * t),
        round(a[2] + (b[2] - a[2]) * t),
    )


def _dim(rgb: tuple[int, int, int], amount: float) -> str:
    r, g, b = _blend((10, 10, 16), rgb, max(0.0, min(1.0, amount)))
    return f"#{r:02x}{g:02x}{b:02x}"


class WebCanvas(Widget):
    """Full-screen canvas: drifting nodes, pulsing connection lines, drag to nudge."""

    DEFAULT_CSS = """
    WebCanvas {
        width: 1fr;
        height: 1fr;
        background: #06070d;
    }
    """

    def __init__(self) -> None:
        super().__init__()
        self.nodes = NODES
        self.time = 0.0
        self._dragging: int | None = None
        self._grab_dx = 0.0
        self._grab_dy = 0.0

    def on_mount(self) -> None:
        self.set_interval(1 / 20, self._tick)

    # --- simulation -----------------------------------------------------

    def _tick(self) -> None:
        self.time += 1 / 20
        for i, n in enumerate(self.nodes):
            if i == self._dragging:
                continue
            # Slow random walk with light damping; jitter steers the velocity.
            n.vx += random.uniform(-0.0016, 0.0016)
            n.vy += random.uniform(-0.0016, 0.0016)
            n.vx *= 0.94
            n.vy *= 0.94
            n.x += n.vx
            n.y += n.vy
            # Bounce softly off the margins so nodes stay on canvas.
            if n.x < 0.04 or n.x > 0.96:
                n.vx *= -1
                n.x = min(max(n.x, 0.04), 0.96)
            if n.y < 0.06 or n.y > 0.94:
                n.vy *= -1
                n.y = min(max(n.y, 0.06), 0.94)
        self.refresh()

    # --- coordinate mapping --------------------------------------------

    def _to_cells(self, n: Node, size: Size) -> tuple[int, int]:
        cx = round(n.x * (size.width - 1))
        cy = round(n.y * (size.height - 1))
        return cx, cy

    def _norm_dist(self, a: Node, b: Node) -> float:
        dx = a.x - b.x
        dy = (a.y - b.y) / CELL_ASPECT
        return math.hypot(dx, dy)

    # --- mouse ----------------------------------------------------------

    def _nearest(self, mx: int, my: int) -> int | None:
        size = self.size
        best, best_d = None, 1e9
        for i, n in enumerate(self.nodes):
            cx, cy = self._to_cells(n, size)
            d = math.hypot(cx - mx, (cy - my) / CELL_ASPECT)
            if d < best_d:
                best, best_d = i, d
        return best if best_d <= 6 else None

    def on_mouse_down(self, event: MouseDown) -> None:
        idx = self._nearest(event.x, event.y)
        if idx is None:
            return
        self._dragging = idx
        n = self.nodes[idx]
        cx, cy = self._to_cells(n, self.size)
        self._grab_dx = cx - event.x
        self._grab_dy = cy - event.y
        n.vx = n.vy = 0.0
        self.capture_mouse()

    def on_mouse_move(self, event: MouseMove) -> None:
        if self._dragging is None:
            return
        size = self.size
        n = self.nodes[self._dragging]
        n.x = min(max((event.x + self._grab_dx) / max(size.width - 1, 1), 0.04), 0.96)
        n.y = min(max((event.y + self._grab_dy) / max(size.height - 1, 1), 0.06), 0.94)
        self.refresh()

    def on_mouse_up(self, event: MouseUp) -> None:
        if self._dragging is None:
            return
        # Resume autonomous drift with a small kick in a random direction.
        n = self.nodes[self._dragging]
        n.vx = random.uniform(-0.004, 0.004)
        n.vy = random.uniform(-0.004, 0.004)
        self._dragging = None
        self.release_mouse()

    # --- rendering ------------------------------------------------------

    def render_line(self, y: int):
        from textual.strip import Strip
        from rich.segment import Segment

        size = self.size
        width = size.width
        bg = "#06070d"

        # Build this row as a list of (char, fg) cells, then coalesce to segments.
        row: list[tuple[str, str]] = [(" ", bg)] * width

        # 1) Connection lines (drawn underneath nodes).
        for a_i, b_i in EDGES:
            a, b = self.nodes[a_i], self.nodes[b_i]
            dist = self._norm_dist(a, b)
            # Closeness 0..1, brighter as nodes approach.
            close = 1.0 - (dist - NEAR) / (FAR - NEAR)
            close = max(0.05, min(1.0, close))
            # Breathing pulse travels along the link; stronger links pulse harder.
            pulse = 0.5 + 0.5 * math.sin(self.time * 1.6 + (a_i + b_i))
            intensity = 0.12 + close * (0.45 + 0.45 * pulse)
            col = _blend(_hex_to_rgb(a.color), _hex_to_rgb(b.color), 0.5)
            self._draw_segment(row, y, a, b, size, col, intensity, close)

        # 2) Nodes on top.
        for n in self.nodes:
            cx, cy = self._to_cells(n, size)
            rgb = _hex_to_rgb(n.color)
            if cy == y and 0 <= cx < width:
                glow = 0.7 + 0.3 * math.sin(self.time * 2.2 + n.phase)
                row[cx] = ("●", _dim(rgb, glow))
                # Label to the right of the node.
                label = f" {n.short}"
                for k, ch in enumerate(label):
                    px = cx + 1 + k
                    if 0 <= px < width:
                        row[px] = (ch, _dim(rgb, 0.85))

        # Coalesce consecutive same-style cells.
        segments: list[Segment] = []
        run_chars: list[str] = []
        run_fg = row[0][1]
        for ch, fg in row:
            if fg == run_fg:
                run_chars.append(ch)
            else:
                segments.append(Segment("".join(run_chars), Style(color=run_fg, bgcolor=bg)))
                run_chars = [ch]
                run_fg = fg
        segments.append(Segment("".join(run_chars), Style(color=run_fg, bgcolor=bg)))
        return Strip(segments, width)

    def _draw_segment(
        self,
        row: list[tuple[str, str]],
        y: int,
        a: Node,
        b: Node,
        size: Size,
        col: tuple[int, int, int],
        intensity: float,
        close: float,
    ) -> None:
        """Plot the cells of edge a-b that fall on this row using Bresenham."""
        x0, y0 = self._to_cells(a, size)
        x1, y1 = self._to_cells(b, size)
        dx = abs(x1 - x0)
        dy = -abs(y1 - y0)
        sx = 1 if x0 < x1 else -1
        sy = 1 if y0 < y1 else -1
        err = dx + dy
        cx, cy = x0, y0
        # Pick a glyph that hints at the line direction.
        slope = (y1 - y0) / (x1 - x0) if x1 != x0 else 9
        if abs(slope) < 0.35:
            glyph = "─"
        elif abs(slope) > 3:
            glyph = "│"
        elif slope > 0:
            glyph = "╲"
        else:
            glyph = "╱"
        char = glyph if close > 0.45 else "·"
        color = _dim(col, intensity)
        while True:
            if cy == y and 0 <= cx < size.width:
                # Don't overwrite an endpoint node cell.
                if (cx, cy) != (x0, y0) and (cx, cy) != (x1, y1):
                    row[cx] = (char, color)
            if cx == x1 and cy == y1:
                break
            e2 = 2 * err
            if e2 >= dy:
                err += dy
                cx += sx
            if e2 <= dx:
                err += dx
                cy += sy


class NodePanel(Static):
    """Side readout of each node's encoded attributes."""

    DEFAULT_CSS = """
    NodePanel {
        width: 34;
        height: 1fr;
        background: #0a0c14;
        border-left: tall #1b1f2e;
        padding: 1 2;
    }
    """

    def on_mount(self) -> None:
        self.set_interval(1 / 6, self.refresh)

    def render(self) -> Text:
        t = Text()
        t.append("SPATIAL WEB\n", style="bold #c8d3f5")
        t.append("X brightness · Y rate\n\n", style="#5b6275")
        for n in NODES:
            rgb = _dim(_hex_to_rgb(n.color), 0.95)
            t.append("● ", style=rgb)
            t.append(f"{n.name}\n", style=f"bold {rgb}")
            t.append(
                f"   bright {n.x:.2f}  rate {1 - n.y:.2f}\n",
                style="#7a8294",
            )
        t.append("\ndrag a node to nudge it.\n", style="#5b6275")
        t.append("it resumes drifting on release.", style="#5b6275")
        return t


class SpatialWebApp(App):
    CSS = """
    Screen {
        layout: horizontal;
        background: #06070d;
    }
    """

    BINDINGS = [("q", "quit", "Quit")]

    def compose(self) -> ComposeResult:
        yield WebCanvas()
        yield NodePanel()
        yield Footer()


if __name__ == "__main__":
    SpatialWebApp().run()
