// exp07_web_of_threads — non-functional UI prototype for a generative focus
// music engine. Pure visual, no audio. Five instrument nodes orbit the screen
// center at different rates, connected by a complete web of wires. Each wire
// brightens as its two endpoints drift closer ("phase alignment"). Click-drag a
// node to reposition it; on release it begins a fresh slow orbit from where you
// dropped it. The feel: a constellation you can rearrange.

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

const FRAME: Duration = Duration::from_millis(33); // ~30fps
const TAU: f64 = std::f64::consts::TAU;

// Rows are roughly twice as tall as columns are wide. Scaling vertical deltas by
// this factor keeps orbits looking circular and distances visually honest.
const ASPECT: f64 = 2.0;

const BG: Color = Color::Rgb(8, 10, 18);

struct Node {
    label: &'static str,
    color: (f64, f64, f64),
    // Orbit definition. Center persists across drags; on release it is shifted
    // so the node keeps its current screen position with no jump.
    ox: f64,
    oy: f64,
    radius: f64,
    omega: f64, // rad/s, small => slow drift
    phase: f64,
    // Current resolved screen position, in cell coords relative to the field.
    x: f64,
    y: f64,
}

struct App {
    nodes: Vec<Node>,
    dragging: Option<usize>,
    initialized: bool,
    t: f64,
    // Field origin/size in absolute terminal cells (inside the border).
    ox0: u16,
    oy0: u16,
    fw: usize,
    fh: usize,
}

impl App {
    fn new() -> Self {
        // Muted blues, teals, purples, warm amber.
        let spec = |label, color, radius, omega, phase| Node {
            label,
            color,
            ox: 0.0,
            oy: 0.0,
            radius,
            omega,
            phase,
            x: 0.0,
            y: 0.0,
        };
        let nodes = vec![
            spec("Tonal", (90.0, 190.0, 180.0), 10.0, 0.060, 0.0),
            spec("Pulse", (220.0, 165.0, 75.0), 18.0, -0.090, 1.2),
            spec("Noise", (155.0, 115.0, 200.0), 24.0, 0.050, 2.5),
            spec("Kick", (215.0, 120.0, 85.0), 14.0, 0.110, 3.9),
            spec("Pad", (95.0, 135.0, 205.0), 26.0, -0.045, 5.1),
        ];
        App {
            nodes,
            dragging: None,
            initialized: false,
            t: 0.0,
            ox0: 0,
            oy0: 0,
            fw: 0,
            fh: 0,
        }
    }

    // Resolve node positions for time t inside a field of fw x fh cells.
    fn update(&mut self, t: f64, ox0: u16, oy0: u16, fw: usize, fh: usize) {
        self.t = t;
        self.ox0 = ox0;
        self.oy0 = oy0;
        self.fw = fw;
        self.fh = fh;
        let cx = (fw as f64 - 1.0) / 2.0;
        let cy = (fh as f64 - 1.0) / 2.0;

        if !self.initialized {
            for n in &mut self.nodes {
                n.ox = cx;
                n.oy = cy;
            }
            self.initialized = true;
        }

        for (i, n) in self.nodes.iter_mut().enumerate() {
            if self.dragging == Some(i) {
                continue; // position pinned to the cursor
            }
            let a = n.phase + n.omega * t;
            n.x = n.ox + n.radius * a.cos();
            n.y = n.oy + n.radius * a.sin() / ASPECT;
        }
    }

    fn field_coords(&self, col: u16, row: u16) -> (f64, f64) {
        let fx = col as f64 - self.ox0 as f64;
        let fy = row as f64 - self.oy0 as f64;
        (fx, fy)
    }

    fn handle_down(&mut self, col: u16, row: u16) {
        let (fx, fy) = self.field_coords(col, row);
        let mut best: Option<(usize, f64)> = None;
        for (i, n) in self.nodes.iter().enumerate() {
            let dx = n.x - fx;
            let dy = (n.y - fy) * ASPECT;
            let d = (dx * dx + dy * dy).sqrt();
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        if let Some((i, d)) = best {
            if d <= 4.0 {
                self.dragging = Some(i);
                self.nodes[i].x = fx.clamp(0.0, self.fw.saturating_sub(1) as f64);
                self.nodes[i].y = fy.clamp(0.0, self.fh.saturating_sub(1) as f64);
            }
        }
    }

    fn handle_drag(&mut self, col: u16, row: u16) {
        if let Some(i) = self.dragging {
            let (fx, fy) = self.field_coords(col, row);
            self.nodes[i].x = fx.clamp(0.0, self.fw.saturating_sub(1) as f64);
            self.nodes[i].y = fy.clamp(0.0, self.fh.saturating_sub(1) as f64);
        }
    }

    fn handle_up(&mut self) {
        if let Some(i) = self.dragging.take() {
            let t = self.t;
            let mut rng = rand::thread_rng();
            let n = &mut self.nodes[i];
            // Fresh slow orbit: new angular velocity and radius, with the center
            // recomputed so the node stays exactly where it was dropped.
            n.omega = rng.gen_range(0.04..0.12) * if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
            n.radius = rng.gen_range(10.0..26.0);
            n.phase = rng.gen_range(0.0..TAU);
            let a = n.phase + n.omega * t;
            n.ox = n.x - n.radius * a.cos();
            n.oy = n.y - n.radius * a.sin() / ASPECT;
        }
    }
}

struct Grid {
    w: usize,
    h: usize,
    ch: Vec<char>,
    col: Vec<Color>,
    bright: Vec<f64>, // max-blend accumulator for wires
}

impl Grid {
    fn new(w: usize, h: usize) -> Self {
        Grid {
            w,
            h,
            ch: vec![' '; w * h],
            col: vec![BG; w * h],
            bright: vec![-1.0; w * h],
        }
    }

    fn plot_wire(&mut self, x: i32, y: i32, glyph: char, b: f64, color: (f64, f64, f64)) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = y as usize * self.w + x as usize;
        if b <= self.bright[i] {
            return;
        }
        self.bright[i] = b;
        self.ch[i] = glyph;
        self.col[i] = lerp_color(BG, color, b);
    }

    fn put(&mut self, x: i32, y: i32, glyph: char, color: Color) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = y as usize * self.w + x as usize;
        self.ch[i] = glyph;
        self.col[i] = color;
        self.bright[i] = 2.0; // protect from later wire writes
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
    loop {
        let t = start.elapsed().as_secs_f64();
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);

        if area.width >= 4 && area.height >= 4 {
            let fw = (area.width - 2) as usize;
            let fh = (area.height - 2) as usize;
            app.update(t, area.x + 1, area.y + 1, fw, fh);
        }

        terminal.draw(|f| render(f, app))?;

        if event::poll(FRAME)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    _ => {}
                },
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => app.handle_down(m.column, m.row),
                    MouseEventKind::Drag(MouseButton::Left) => app.handle_drag(m.column, m.row),
                    MouseEventKind::Up(MouseButton::Left) => app.handle_up(),
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}

fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.width < 4 || area.height < 4 {
        return;
    }

    let title = match app.dragging {
        Some(i) => format!(
            " web of threads  ·  moving {}  ·  drag nodes · q to quit ",
            app.nodes[i].label
        ),
        None => " web of threads  ·  drag a node · q to quit ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(40, 46, 66)))
        .title(title)
        .title_style(Style::default().fg(Color::Rgb(120, 130, 160)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let w = inner.width as usize;
    let h = inner.height as usize;
    if w == 0 || h == 0 {
        return;
    }
    let mut grid = Grid::new(w, h);

    // Wires: complete graph over all node pairs. Closer pairs glow brighter.
    let ref_dist = (w as f64).max(1.0) * 0.55;
    for a in 0..app.nodes.len() {
        for b in (a + 1)..app.nodes.len() {
            let na = &app.nodes[a];
            let nb = &app.nodes[b];
            let dx = nb.x - na.x;
            let dy = (nb.y - na.y) * ASPECT;
            let dist = (dx * dx + dy * dy).sqrt();
            let prox = (1.0 - dist / ref_dist).clamp(0.0, 1.0);
            // Slow shimmer keyed to the pair so nothing is ever fully static.
            let shimmer = 0.85 + 0.15 * (app.t * 0.9 + (a * 7 + b * 13) as f64).sin();
            let bright = (0.08 + 0.92 * prox.powf(1.3) * shimmer).clamp(0.05, 1.0);
            let color = blend(na.color, nb.color);
            let glyph = wire_glyph(dx, dy);
            draw_line(
                &mut grid,
                na.x.round() as i32,
                na.y.round() as i32,
                nb.x.round() as i32,
                nb.y.round() as i32,
                glyph,
                bright,
                color,
            );
        }
    }

    // Nodes and labels drawn on top of the wires.
    for n in &app.nodes {
        let nx = n.x.round() as i32;
        let ny = n.y.round() as i32;
        let core = Color::Rgb(n.color.0 as u8, n.color.1 as u8, n.color.2 as u8);
        grid.put(nx, ny, '●', core);
        let label_color = lerp_color(BG, n.color, 0.85);
        for (k, ch) in n.label.chars().enumerate() {
            grid.put(nx + 2 + k as i32, ny, ch, label_color);
        }
    }

    let lines = grid_to_lines(&grid);
    f.render_widget(Paragraph::new(lines), inner);
}

// Bresenham rasterization of a wire segment.
#[allow(clippy::too_many_arguments)]
fn draw_line(
    grid: &mut Grid,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    glyph: char,
    bright: f64,
    color: (f64, f64, f64),
) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        grid.plot_wire(x, y, glyph, bright, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

// Pick a box-drawing glyph that matches the wire's overall direction.
fn wire_glyph(dx: f64, dy: f64) -> char {
    // dy already aspect-scaled by the caller.
    let ang = dy.atan2(dx); // -pi..pi
    let mut a = ang;
    if a < 0.0 {
        a += std::f64::consts::PI; // fold to 0..pi, lines are undirected
    }
    let deg = a.to_degrees();
    if !(22.5..157.5).contains(&deg) {
        '─'
    } else if deg < 67.5 {
        '╲'
    } else if deg < 112.5 {
        '│'
    } else {
        '╱'
    }
}

fn grid_to_lines<'a>(grid: &Grid) -> Vec<Line<'a>> {
    let mut lines = Vec::with_capacity(grid.h);
    for row in 0..grid.h {
        let mut spans = Vec::with_capacity(grid.w);
        for col in 0..grid.w {
            let i = row * grid.w + col;
            spans.push(Span::styled(
                grid.ch[i].to_string(),
                Style::default().fg(grid.col[i]),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn blend(a: (f64, f64, f64), b: (f64, f64, f64)) -> (f64, f64, f64) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5, (a.2 + b.2) * 0.5)
}

fn lerp_color(from: Color, to: (f64, f64, f64), t: f64) -> Color {
    let (fr, fg, fb) = match from {
        Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
        _ => (0.0, 0.0, 0.0),
    };
    let r = fr + (to.0 - fr) * t;
    let g = fg + (to.1 - fg) * t;
    let b = fb + (to.2 - fb) * t;
    Color::Rgb(r as u8, g as u8, b as u8)
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
