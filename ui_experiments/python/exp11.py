"""exp11 - Radial Constellation

A non-functional UI prototype for a generative focus-music engine.

Observable + nudgeable. Five instruments orbit a central point like an
orrery. Distance from center reads as volume/intensity; angular position
reads as stereo pan. A pulsing ring at the center breathes with a slow LFO
(~4s sine). Each node drags a fading orbital trail behind it. Faint lines
connect the nodes and brighten when two instruments drift into phase sync.

Mouse: click-drag a node to nudge it into a new orbit. Release and it
eases into the new path. Press 'q' to quit.

No audio. Visual prototype only.
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass, field
from collections import deque

from rich.style import Style
from rich.text import Text

from textual.app import App, ComposeResult
from textual.widget import Widget


TICK = 1.0 / 30.0          # ~30fps
ASPECT = 0.5               # terminal cells are ~twice as tall as wide
BREATHE_PERIOD = 4.0       # seconds per center-ring breath
TRAIL_LEN = 22
GRAB_DIST = 4.0            # how close a click must be (content units) to grab
SETTLE = 0.12             # radius easing per tick toward target orbit
SYNC_WINDOW = 0.45         # angular distance (rad) under which a link glows

BG = (7, 10, 20)           # deep space blue

# constellation palette: golds, nebula purples, cold blues
PALETTE = [
    ("LO  PAD",  (255, 212, 121)),   # gold
    ("ARP",      (180, 140, 255)),   # nebula purple
    ("BASS",     (110, 168, 255)),   # cold blue
    ("BELL",     (95, 224, 200)),    # teal
    ("VOX",      (255, 158, 196)),   # pink
]


@dataclass
class Node:
    name: str
    color: tuple[int, int, int]
    radius: float
    target_radius: float
    angle: float
    omega: float              # angular velocity (rad/s)
    trail: deque = field(default_factory=lambda: deque(maxlen=TRAIL_LEN))


def _scale(color: tuple[int, int, int], b: float) -> tuple[int, int, int]:
    """Blend a colour toward the background by brightness b (0..1)."""
    b = max(0.0, min(1.0, b))
    return (
        int(BG[0] + (color[0] - BG[0]) * b),
        int(BG[1] + (color[1] - BG[1]) * b),
        int(BG[2] + (color[2] - BG[2]) * b),
    )


def _hex(rgb: tuple[int, int, int]) -> str:
    return f"#{rgb[0]:02x}{rgb[1]:02x}{rgb[2]:02x}"


class Constellation(Widget):
    """Full-screen orrery canvas with custom rendering and mouse nudging."""

    can_focus = True

    def on_mount(self) -> None:
        self._t = 0.0
        self._instrument_nodes: list[Node] = []
        # spread radii and incommensurate angular velocities -> slow drift
        radii = [7.0, 11.0, 15.0, 19.0, 23.0]
        omegas = [0.55, -0.38, 0.27, -0.19, 0.13]
        for i, (name, color) in enumerate(PALETTE):
            self._instrument_nodes.append(
                Node(
                    name=name,
                    color=color,
                    radius=radii[i],
                    target_radius=radii[i],
                    angle=random.uniform(0, math.tau),
                    omega=omegas[i],
                )
            )
        self._dragging: int | None = None
        self._stars: list[tuple[int, int, float]] = []
        self._star_seed = random.Random(11)
        self.set_interval(TICK, self._tick)

    # ---- geometry helpers -------------------------------------------------

    def _center(self) -> tuple[float, float]:
        return (self.size.width / 2.0, self.size.height / 2.0)

    def _to_screen(self, r: float, a: float) -> tuple[float, float]:
        cx, cy = self._center()
        return (cx + r * math.cos(a), cy + r * math.sin(a) * ASPECT)

    def _from_screen(self, x: float, y: float) -> tuple[float, float]:
        cx, cy = self._center()
        dx = x - cx
        dy = (y - cy) / ASPECT
        return (math.hypot(dx, dy), math.atan2(dy, dx))

    def _max_radius(self) -> float:
        w, h = self.size.width, self.size.height
        return max(6.0, min(w / 2.0 - 2.0, (h / 2.0 - 1.0) / ASPECT))

    # ---- simulation -------------------------------------------------------

    def _tick(self) -> None:
        self._t += TICK
        for i, n in enumerate(self._instrument_nodes):
            if i != self._dragging:
                n.angle = (n.angle + n.omega * TICK) % math.tau
            # ease current radius toward its target orbit (the "settle")
            n.radius += (n.target_radius - n.radius) * SETTLE
            x, y = self._to_screen(n.radius, n.angle)
            n.trail.append((x, y))
        self.refresh()

    # ---- mouse ------------------------------------------------------------

    def _local(self, event) -> tuple[float, float]:
        return (float(event.x), float(event.y))

    def on_mouse_down(self, event) -> None:
        mx, my = self._local(event)
        best, best_d = None, GRAB_DIST
        for i, n in enumerate(self._instrument_nodes):
            x, y = self._to_screen(n.radius, n.angle)
            d = math.hypot((x - mx), (y - my) / ASPECT)
            if d < best_d:
                best, best_d = i, d
        if best is not None:
            self._dragging = best
            self.capture_mouse()

    def on_mouse_move(self, event) -> None:
        if self._dragging is None:
            return
        mx, my = self._local(event)
        r, a = self._from_screen(mx, my)
        r = max(3.5, min(self._max_radius(), r))
        n = self._instrument_nodes[self._dragging]
        n.angle = a
        n.target_radius = r
        n.radius += (r - n.radius) * 0.5   # snappier while held

    def on_mouse_up(self, event) -> None:
        if self._dragging is not None:
            self._dragging = None
            self.release_mouse()

    # ---- rendering --------------------------------------------------------

    def render(self) -> Text:
        w, h = self.size.width, self.size.height
        if w <= 0 or h <= 0:
            return Text("")

        # buffer[y][x] = (char, (r,g,b), brightness)
        buf: list[list[tuple[str, tuple[int, int, int], float] | None]]
        buf = [[None] * w for _ in range(h)]

        def plot(x: float, y: float, ch: str, color, bright: float) -> None:
            ix, iy = int(round(x)), int(round(y))
            if 0 <= ix < w and 0 <= iy < h:
                cur = buf[iy][ix]
                if cur is None or bright >= cur[2]:
                    buf[iy][ix] = (ch, color, bright)

        # --- starfield (static deep-space dust) ---
        if not self._stars:
            for _ in range(int(w * h * 0.02)):
                self._stars.append(
                    (
                        self._star_seed.randint(0, max(0, w - 1)),
                        self._star_seed.randint(0, max(0, h - 1)),
                        self._star_seed.uniform(0.06, 0.18),
                    )
                )
        for sx, sy, sb in self._stars:
            plot(sx, sy, "·", (150, 170, 220), sb)

        cx, cy = self._center()

        # --- phase-sync links (drawn faint, behind everything) ---
        for i in range(len(self._instrument_nodes)):
            for j in range(i + 1, len(self._instrument_nodes)):
                a, b = self._instrument_nodes[i], self._instrument_nodes[j]
                diff = abs((a.angle - b.angle + math.pi) % math.tau - math.pi)
                if diff < SYNC_WINDOW:
                    sync = 1.0 - diff / SYNC_WINDOW
                    x0, y0 = self._to_screen(a.radius, a.angle)
                    x1, y1 = self._to_screen(b.radius, b.angle)
                    lc = (
                        (a.color[0] + b.color[0]) // 2,
                        (a.color[1] + b.color[1]) // 2,
                        (a.color[2] + b.color[2]) // 2,
                    )
                    steps = int(max(abs(x1 - x0), abs(y1 - y0) / ASPECT)) + 1
                    for s in range(steps + 1):
                        t = s / steps
                        ch = "•" if sync > 0.66 else ("·" if sync > 0.33 else "˙")
                        plot(
                            x0 + (x1 - x0) * t,
                            y0 + (y1 - y0) * t,
                            ch,
                            lc,
                            0.12 + 0.5 * sync,
                        )

        # --- orbital trails ---
        for n in self._instrument_nodes:
            L = len(n.trail)
            for k, (tx, ty) in enumerate(n.trail):
                age = (k + 1) / L
                plot(tx, ty, "·", n.color, 0.08 + 0.32 * age)

        # --- center breathing ring (LFO) ---
        breath = 0.5 + 0.5 * math.sin(self._t * math.tau / BREATHE_PERIOD)
        ring_r = 2.6 + 2.4 * breath
        sigma = 1.3
        box = int(ring_r + 4)
        for dy in range(-box, box + 1):
            for dx in range(-box, box + 1):
                px, py = cx + dx, cy + dy
                if not (0 <= px < w and 0 <= py < h):
                    continue
                d = math.hypot(dx, dy / ASPECT)
                glow = math.exp(-((d - ring_r) ** 2) / (2 * sigma ** 2))
                core = math.exp(-(d ** 2) / (2 * (ring_r * 0.6) ** 2)) * 0.4
                inten = glow * (0.55 + 0.45 * breath) + core
                if inten < 0.06:
                    continue
                # gold core fading to nebula purple at the rim
                mix = max(0.0, min(1.0, (d / (ring_r + 2))))
                col = (
                    int(255 + (180 - 255) * mix),
                    int(212 + (140 - 212) * mix),
                    int(121 + (255 - 121) * mix),
                )
                if inten > 0.75:
                    ch = "█"
                elif inten > 0.5:
                    ch = "▓"
                elif inten > 0.28:
                    ch = "▒"
                else:
                    ch = "░"
                plot(px, py, ch, col, min(1.0, inten))

        # --- instrument nodes + labels (on top) ---
        for i, n in enumerate(self._instrument_nodes):
            x, y = self._to_screen(n.radius, n.angle)
            held = i == self._dragging
            plot(x, y, "◉" if held else "●", n.color, 1.0)
            # halo
            for hx, hy in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
                plot(hx, hy, "∙", n.color, 0.35)
            # label trailing toward the outside of the orbit
            lx = int(round(x)) + (2 if math.cos(n.angle) >= 0 else -(len(n.name) + 1))
            for ci, c in enumerate(n.name):
                plot(lx + ci, int(round(y)), c, n.color, 0.7 if held else 0.5)

        # --- assemble Rich Text ---
        out = Text()
        for row in range(h):
            for col in range(w):
                cell = buf[row][col]
                if cell is None:
                    out.append(" ")
                else:
                    ch, color, bright = cell
                    out.append(ch, Style(color=_hex(_scale(color, bright))))
            if row < h - 1:
                out.append("\n")
        return out


class RadialConstellation(App):
    CSS = """
    Screen { background: #04060e; }
    Constellation { width: 100%; height: 100%; background: #04060e; }
    """

    def compose(self) -> ComposeResult:
        yield Constellation()

    def on_key(self, event) -> None:
        if event.key in ("q", "escape"):
            self.exit()


if __name__ == "__main__":
    RadialConstellation().run()
