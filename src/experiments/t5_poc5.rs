// Liquid Noise — an organic fluid-field visualizer.
//
// A 2D noise field (layered sin/cos, a cheap Perlin stand-in) scrolls and
// morphs over time. Each cell maps to a glyph + heat-map color. Random ripples
// distort the field outward, sparkles flare at local maxima, dim text labels
// float over the top, and an edge vignette frames the whole thing.

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute, terminal,
};
use rand::Rng;
use ratatui::prelude::*;
use ratatui::widgets::Widget;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

const GRADIENT: &[char] = &[' ', '·', '∙', '•', '●', '◉', '⬤'];
const SPARKLES: &[char] = &['✦', '✧', '⋆'];
const RIPPLE_INTERVAL: f32 = 2.6;
const RIPPLE_SPEED: f32 = 14.0;
const RIPPLE_LIFETIME: f32 = 3.5;

struct Ripple {
    cx: f32,
    cy: f32,
    age: f32,
}

struct Label {
    text: &'static str,
    // position in field units (0..1 of width/height)
    fx: f32,
    fy: f32,
}

struct App {
    t: f32,
    ripples: Vec<Ripple>,
    next_ripple: f32,
    labels: Vec<Label>,
    rng: rand::rngs::ThreadRng,
}

impl App {
    fn new() -> Self {
        App {
            t: 0.0,
            ripples: Vec::new(),
            next_ripple: 0.8,
            labels: vec![
                Label { text: "NOOISE", fx: 0.5, fy: 0.42 },
                Label { text: "440.0 Hz", fx: 0.18, fy: 0.7 },
                Label { text: "220.0 Hz", fx: 0.78, fy: 0.25 },
                Label { text: "~ liquid ~", fx: 0.62, fy: 0.82 },
            ],
            rng: rand::thread_rng(),
        }
    }

    fn tick(&mut self, dt: f32) {
        self.t += dt;

        // age + cull ripples
        for r in &mut self.ripples {
            r.age += dt;
        }
        self.ripples.retain(|r| r.age < RIPPLE_LIFETIME);

        // spawn ripples on a cadence
        self.next_ripple -= dt;
        if self.next_ripple <= 0.0 {
            self.next_ripple = RIPPLE_INTERVAL * (0.6 + self.rng.r#gen::<f32>());
            self.ripples.push(Ripple {
                cx: self.rng.r#gen::<f32>(),
                cy: self.rng.r#gen::<f32>(),
                age: 0.0,
            });
        }
    }

    /// Base liquid field value in 0..1 at normalized coords (nx, ny).
    fn field(&self, nx: f32, ny: f32) -> f32 {
        let z = self.t * 0.55;
        // layered, drifting trigonometric noise — smooth everywhere
        let mut v = 0.0;
        v += (nx * 6.0 + z).sin() * (ny * 5.0 - z * 0.7).cos();
        v += ((nx * 3.3 - ny * 4.1) + z * 1.3).sin() * 0.7;
        v += (nx * 11.0 + ny * 9.0 - z * 0.4).sin() * 0.35;
        v += ((nx + ny) * 7.5 + (z * 0.9).sin() * 2.0).cos() * 0.5;

        // ripple distortion: radial sine pulse fading with age + distance
        for r in &self.ripples {
            let dx = nx - r.cx;
            let dy = ny - r.cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let front = r.age * RIPPLE_SPEED * 0.04;
            let fade = (1.0 - r.age / RIPPLE_LIFETIME).max(0.0);
            let ring = (-((dist - front) * 14.0).powi(2)).exp();
            v += (dist * 30.0 - r.age * 8.0).sin() * ring * fade * 1.6;
        }

        // squash to 0..1
        (v / 3.0).tanh() * 0.5 + 0.5
    }
}

/// Heat-map color: deep blue → teal → green → yellow → orange.
fn heat_color(v: f32) -> Color {
    let v = v.clamp(0.0, 1.0);
    let stops = [
        (0.0_f32, (10, 20, 70)),
        (0.3, (10, 110, 130)),
        (0.55, (30, 170, 90)),
        (0.78, (210, 200, 60)),
        (1.0, (235, 130, 40)),
    ];
    let mut lo = stops[0];
    let mut hi = stops[stops.len() - 1];
    for w in stops.windows(2) {
        if v >= w[0].0 && v <= w[1].0 {
            lo = w[0];
            hi = w[1];
            break;
        }
    }
    let span = (hi.0 - lo.0).max(1e-4);
    let f = ((v - lo.0) / span).clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f) as u8;
    Color::Rgb(
        lerp(lo.1 .0, hi.1 .0),
        lerp(lo.1 .1, hi.1 .1),
        lerp(lo.1 .2, hi.1 .2),
    )
}

fn dim(c: Color, amount: f32) -> Color {
    if let Color::Rgb(r, g, b) = c {
        let a = amount.clamp(0.0, 1.0);
        Color::Rgb(
            (r as f32 * a) as u8,
            (g as f32 * a) as u8,
            (b as f32 * a) as u8,
        )
    } else {
        c
    }
}

struct LiquidWidget<'a> {
    app: &'a App,
}

impl<'a> Widget for LiquidWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let w = area.width.max(1) as f32;
        let h = area.height.max(1) as f32;
        let app = self.app;

        // sample the field once per cell, keep values for sparkle maxima check
        let cols = area.width as usize;
        let mut vals = vec![0.0f32; cols * area.height as usize];

        for y in 0..area.height {
            for x in 0..area.width {
                let nx = x as f32 / w;
                let ny = y as f32 / h;
                let v = app.field(nx, ny);
                vals[y as usize * cols + x as usize] = v;

                // edge vignette: fade toward 0 near borders
                let edge_x = (nx.min(1.0 - nx) * 2.0).min(1.0);
                let edge_y = (ny.min(1.0 - ny) * 2.0).min(1.0);
                let vig = (edge_x.min(edge_y) * 1.4).clamp(0.15, 1.0);

                let gi = ((v * (GRADIENT.len() - 1) as f32).round() as usize)
                    .min(GRADIENT.len() - 1);
                let ch = GRADIENT[gi];
                let col = dim(heat_color(v), vig);

                buf[(area.x + x, area.y + y)]
                    .set_char(ch)
                    .set_style(Style::default().fg(col));
            }
        }

        // sparkles: local maxima above a threshold, flickering by time/pos
        for y in 1..area.height.saturating_sub(1) {
            for x in 1..area.width.saturating_sub(1) {
                let idx = y as usize * cols + x as usize;
                let v = vals[idx];
                if v < 0.82 {
                    continue;
                }
                let is_max = v >= vals[idx - 1]
                    && v >= vals[idx + 1]
                    && v >= vals[idx - cols]
                    && v >= vals[idx + cols];
                if !is_max {
                    continue;
                }
                // flicker so sparkles appear briefly
                let flick = ((x as f32 * 1.7 + y as f32 * 2.3 + app.t * 6.0).sin()).abs();
                if flick < 0.7 {
                    continue;
                }
                let si = (x as usize + y as usize + (app.t * 4.0) as usize) % SPARKLES.len();
                buf[(area.x + x, area.y + y)]
                    .set_char(SPARKLES[si])
                    .set_style(
                        Style::default()
                            .fg(Color::Rgb(255, 255, 235))
                            .add_modifier(Modifier::BOLD),
                    );
            }
        }

        // floating semi-transparent labels
        for label in &app.labels {
            let lx = (label.fx * w) as u16;
            let ly = (label.fy * h) as u16;
            let start = lx.saturating_sub(label.text.len() as u16 / 2);
            for (i, ch) in label.text.chars().enumerate() {
                let cx = start + i as u16;
                if cx >= area.width || ly >= area.height {
                    continue;
                }
                // contrast against the field underneath but dim
                let under = vals[ly as usize * cols + cx as usize];
                let fg = if under > 0.5 {
                    Color::Rgb(20, 25, 40)
                } else {
                    Color::Rgb(210, 220, 235)
                };
                buf[(area.x + cx, area.y + ly)]
                    .set_char(ch)
                    .set_style(Style::default().fg(fg).add_modifier(Modifier::DIM));
            }
        }
    }
}

fn setup() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        terminal::LeaveAlternateScreen,
        cursor::Show
    )?;
    terminal.show_cursor()
}

pub fn run() -> io::Result<()> {
    let mut terminal = setup()?;
    let mut app = App::new();
    let mut last = Instant::now();
    let frame = Duration::from_millis(33);

    let result = loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;
        app.tick(dt);

        if let Err(e) = terminal.draw(|f| {
            f.render_widget(LiquidWidget { app: &app }, f.area());
        }) {
            break Err(e);
        }

        if event::poll(frame)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                {
                    break Ok(());
                }
            }
        }
    };

    restore(terminal)?;
    result
}
