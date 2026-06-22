// exp06_drift_field — non-functional UI prototype for a generative focus music
// engine. The breathing dot field from exp01, now inhabited by four instrument
// nodes that wander on slow random walks. Each node warps the field around it
// into a brighter, denser "gravity well". Click and drag a node to reposition
// it; on release it resumes autonomous drift from wherever you left it. No audio.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::Rng;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

const BREATH_PERIOD: f64 = 4.0; // seconds per full inhale/exhale cycle
const PHASE_OFFSET: f64 = 0.5; // seconds the right half lags the left
const FRAME: Duration = Duration::from_millis(33); // ~30fps

const WELL_RADIUS: f64 = 5.0; // cells of influence around each node
const DRIFT_ACCEL: f64 = 0.06; // random velocity kick per frame (cells/frame)
const DRIFT_DAMP: f64 = 0.92; // velocity decay, keeps the wander gentle
const DRIFT_MAX: f64 = 0.35; // speed cap (cells/frame)
const GRAB_RADIUS: f64 = 2.5; // how close a click must land to grab a node

// Density ramp: weight grows with local intensity.
const GLYPHS: [char; 5] = [' ', '·', '•', '●', '◉'];

// Bright glyphs that mark the instrument nodes themselves.
const NODE_GLYPHS: [char; 4] = ['◈', '◇', '◆', '●'];
const NODE_COLORS: [Color; 4] = [
    Color::Rgb(64, 224, 208),  // teal
    Color::Rgb(170, 120, 240), // purple
    Color::Rgb(90, 150, 250),  // blue
    Color::Rgb(240, 190, 90),  // amber
];

struct Node {
    x: f64, // cell column (terminal coordinate space)
    y: f64, // cell row
    vx: f64,
    vy: f64,
}

struct App {
    nodes: [Node; 4],
    dragging: Option<usize>, // index of the node currently held by the mouse
    bounds: Rect,            // inner field rect, kept fresh each draw
    rng: rand::rngs::ThreadRng,
}

impl App {
    fn new() -> Self {
        // Spread the four nodes across the field; positions get re-seeded once
        // we know the real terminal size on the first drift step.
        let nodes = [
            Node { x: 20.0, y: 8.0, vx: 0.0, vy: 0.0 },
            Node { x: 50.0, y: 6.0, vx: 0.0, vy: 0.0 },
            Node { x: 30.0, y: 16.0, vx: 0.0, vy: 0.0 },
            Node { x: 60.0, y: 14.0, vx: 0.0, vy: 0.0 },
        ];
        App {
            nodes,
            dragging: None,
            bounds: Rect::new(0, 0, 0, 0),
            rng: rand::thread_rng(),
        }
    }

    // Advance every non-dragged node by one step of smooth random walk,
    // bouncing softly off the field edges.
    fn drift(&mut self) {
        let b = self.bounds;
        if b.width < 2 || b.height < 2 {
            return;
        }
        let min_x = b.x as f64;
        let max_x = (b.x + b.width - 1) as f64;
        let min_y = b.y as f64;
        let max_y = (b.y + b.height - 1) as f64;

        for (i, node) in self.nodes.iter_mut().enumerate() {
            if self.dragging == Some(i) {
                continue;
            }

            node.vx += self.rng.gen_range(-DRIFT_ACCEL..DRIFT_ACCEL);
            node.vy += self.rng.gen_range(-DRIFT_ACCEL..DRIFT_ACCEL);
            node.vx *= DRIFT_DAMP;
            node.vy *= DRIFT_DAMP;
            node.vx = node.vx.clamp(-DRIFT_MAX, DRIFT_MAX);
            node.vy = node.vy.clamp(-DRIFT_MAX, DRIFT_MAX);

            node.x += node.vx;
            node.y += node.vy;

            // Soft bounce: clamp to the edge and reverse the offending axis.
            if node.x < min_x {
                node.x = min_x;
                node.vx = node.vx.abs();
            } else if node.x > max_x {
                node.x = max_x;
                node.vx = -node.vx.abs();
            }
            if node.y < min_y {
                node.y = min_y;
                node.vy = node.vy.abs();
            } else if node.y > max_y {
                node.y = max_y;
                node.vy = -node.vy.abs();
            }
        }
    }

    // Find the closest node within grab range of a click, if any.
    fn node_at(&self, col: f64, row: f64) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (i, node) in self.nodes.iter().enumerate() {
            let dx = node.x - col;
            let dy = node.y - row;
            // Terminal cells are ~twice as tall as wide; weight Y so the grab
            // zone reads as round on screen.
            let d = (dx * dx + (dy * 2.0) * (dy * 2.0)).sqrt();
            if d <= GRAB_RADIUS && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    fn grab(&mut self, col: u16, row: u16) {
        if let Some(i) = self.node_at(col as f64, row as f64) {
            self.dragging = Some(i);
            let node = &mut self.nodes[i];
            node.x = col as f64;
            node.y = row as f64;
            node.vx = 0.0;
            node.vy = 0.0;
        }
    }

    fn drag_to(&mut self, col: u16, row: u16) {
        if let Some(i) = self.dragging {
            let b = self.bounds;
            let cx = (col as f64).clamp(b.x as f64, (b.x + b.width.max(1) - 1) as f64);
            let cy = (row as f64).clamp(b.y as f64, (b.y + b.height.max(1) - 1) as f64);
            let node = &mut self.nodes[i];
            node.x = cx;
            node.y = cy;
            // Zero velocity while held; it resumes drifting fresh on release.
            node.vx = 0.0;
            node.vy = 0.0;
        }
    }

    fn release(&mut self) {
        self.dragging = None;
    }
}

fn main() -> io::Result<()> {
    let mut terminal = setup()?;
    let mut app = App::new();
    let start = Instant::now();
    let result = run(&mut terminal, &mut app, start);
    teardown(terminal)?;
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    start: Instant,
) -> io::Result<()> {
    let mut last = Instant::now();
    loop {
        let t = start.elapsed().as_secs_f64();
        terminal.draw(|f| render(f, app, t))?;

        // Drain input until the next frame is due, then advance the sim once.
        let frame_deadline = last + FRAME;
        loop {
            let now = Instant::now();
            let wait = frame_deadline.saturating_duration_since(now);
            if wait.is_zero() || !event::poll(wait)? {
                break;
            }
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    _ => {}
                },
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => app.grab(m.column, m.row),
                    MouseEventKind::Drag(MouseButton::Left) => app.drag_to(m.column, m.row),
                    MouseEventKind::Up(MouseButton::Left) => app.release(),
                    _ => {}
                },
                _ => {}
            }
        }

        last = frame_deadline;
        app.drift();
    }
}

fn render(f: &mut Frame, app: &mut App, t: f64) {
    let area = f.area();
    if area.width < 2 || area.height < 2 {
        return;
    }

    let dragging = app.dragging.is_some();
    let title = format!(
        " drift field  ·  drag a node to nudge it  ·  {}  ·  q to quit ",
        if dragging { "holding" } else { "wandering" }
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title)
        .title_style(Style::default().fg(Color::Gray));

    let inner = block.inner(area);
    app.bounds = inner;
    f.render_widget(block, area);

    if inner.width < 1 || inner.height < 1 {
        return;
    }

    let w = inner.width as f64;
    let h = inner.height as f64;
    let cx = inner.x as f64 + (w - 1.0) / 2.0;
    let cy = inner.y as f64 + (h - 1.0) / 2.0;
    let max_dist = (((w - 1.0) / 2.0).powi(2) + ((h - 1.0) / 2.0).powi(2))
        .sqrt()
        .max(1.0);

    let two_pi = std::f64::consts::TAU;
    let breath_left = (two_pi * t / BREATH_PERIOD).sin();
    let breath_right = (two_pi * (t - PHASE_OFFSET) / BREATH_PERIOD).sin();

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);

    for row in inner.y..inner.y + inner.height {
        let mut spans: Vec<Span> = Vec::with_capacity(inner.width as usize);
        let ry = row as f64;
        let dy = ry - cy;

        for col in inner.x..inner.x + inner.width {
            let rx = col as f64;
            let dx = rx - cx;
            let dist = (dx * dx + dy * dy).sqrt() / max_dist; // 0 center .. 1 corner

            let breath = if rx <= cx { breath_left } else { breath_right };

            // Base breathing field, identical in spirit to exp01.
            let core = 0.55 + 0.35 * breath;
            let intensity = (1.0 - (dist / core)).clamp(0.0, 1.0);
            let shimmer = 0.06 * ((dx * 0.7 + dy * 1.3 + t * 1.5).sin());
            let mut value = (intensity + shimmer).clamp(0.0, 1.0);

            // Gravity wells: each node pulls the local density up. Y is doubled
            // to compensate for the tall aspect ratio of terminal cells, so the
            // wells read as round halos rather than vertical streaks.
            let mut well = 0.0;
            let mut well_node = 0usize;
            for (i, node) in app.nodes.iter().enumerate() {
                let ndx = rx - node.x;
                let ndy = (ry - node.y) * 2.0;
                let nd = (ndx * ndx + ndy * ndy).sqrt();
                if nd < WELL_RADIUS {
                    let pull = (1.0 - nd / WELL_RADIUS).powf(1.5);
                    if pull > well {
                        well = pull;
                        well_node = i;
                    }
                }
            }
            value = (value + well * 0.9).clamp(0.0, 1.0);

            // Is this cell a node center? Stamp the bright instrument glyph.
            let node_here = app.nodes.iter().enumerate().find(|(_, n)| {
                n.x.round() as i64 == col as i64 && n.y.round() as i64 == row as i64
            });

            if let Some((i, _)) = node_here {
                let held = app.dragging == Some(i);
                let style = Style::default()
                    .fg(NODE_COLORS[i])
                    .add_modifier(if held { Modifier::BOLD } else { Modifier::empty() });
                spans.push(Span::styled(NODE_GLYPHS[i].to_string(), style));
                continue;
            }

            let glyph_idx = (value * (GLYPHS.len() as f64 - 1.0)).round() as usize;
            let ch = GLYPHS[glyph_idx.min(GLYPHS.len() - 1)];

            let color = if well > 0.05 {
                // Tint the well toward its owning node's hue.
                blend(color_for(value, breath), NODE_COLORS[well_node], well * 0.7)
            } else {
                color_for(value, breath)
            };
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
        }
        lines.push(Line::from(spans));
    }

    let field = Paragraph::new(lines);
    f.render_widget(field, inner);
}

// Cool indigo at rest, warming toward the inhale peak.
fn color_for(value: f64, breath: f64) -> Color {
    if value <= 0.0 {
        return Color::Rgb(8, 10, 18);
    }
    let warmth = (breath * 0.5 + 0.5).clamp(0.0, 1.0);
    let base = value.powf(0.8);

    let r = (30.0 + 90.0 * warmth + 110.0 * base) as u8;
    let g = (40.0 + 30.0 * warmth + 150.0 * base) as u8;
    let b = (90.0 + 60.0 * (1.0 - warmth) + 120.0 * base) as u8;
    Color::Rgb(r, g, b)
}

// Linear blend between two RGB colors; falls back to `a` for non-RGB inputs.
fn blend(a: Color, b: Color, m: f64) -> Color {
    let m = m.clamp(0.0, 1.0);
    if let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) {
        let mix = |x: u8, y: u8| (x as f64 * (1.0 - m) + y as f64 * m) as u8;
        Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
    } else {
        a
    }
}

fn setup() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::cursor::Hide
    )?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn teardown(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::cursor::Show
    )?;
    terminal.show_cursor()?;
    Ok(())
}
