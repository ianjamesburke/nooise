use std::f64::consts::PI;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode};
use crossterm::{cursor, execute, terminal};
use ratatui::prelude::*;
use ratatui::widgets::canvas::{Canvas, Context, Line, Points};
use ratatui::widgets::Block;

const BANDS: usize = 32;
const EASE: f64 = 0.22;

struct Band {
    freq: f64,
    phase: f64,
    speed: f64,
    target: f64,
    value: f64,
}

struct Analyzer {
    bands: Vec<Band>,
    t: f64,
    rotation: f64,
    ring_rotation: f64,
    bass: f64,
    base_radius: f64,
}

impl Analyzer {
    fn new() -> Self {
        let mut bands = Vec::with_capacity(BANDS);
        for i in 0..BANDS {
            let f = i as f64 / BANDS as f64;
            bands.push(Band {
                // higher bands oscillate faster
                freq: 0.6 + f * 6.0,
                phase: f * PI * 2.0,
                speed: 0.7 + f * 1.3,
                target: 0.0,
                value: 0.0,
            });
        }
        Self {
            bands,
            t: 0.0,
            rotation: 0.0,
            ring_rotation: 0.0,
            bass: 0.0,
            base_radius: 14.0,
        }
    }

    fn update(&mut self, dt: f64) {
        self.t += dt;
        self.rotation += dt * 0.18; // slow clockwise drift
        self.ring_rotation -= dt * 0.5; // inner ring counter-rotates

        for b in &mut self.bands {
            // layered sines fake a spectrum: a slow swell + per-band wiggle + shared beat
            let beat = (self.t * 2.4).sin() * 0.5 + 0.5;
            let wig = (self.t * b.freq * b.speed + b.phase).sin() * 0.5 + 0.5;
            let swell = (self.t * 0.4 + b.phase * 0.3).sin() * 0.5 + 0.5;
            b.target = (wig * 0.55 + swell * 0.25 + beat * 0.20).powf(1.4);
            b.value += (b.target - b.value) * EASE;
        }

        // bass = mean of lowest 4 bands, eased
        let bass_now: f64 = self.bands.iter().take(4).map(|b| b.value).sum::<f64>() / 4.0;
        self.bass += (bass_now - self.bass) * 0.15;
    }
}

fn lerp_color(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let r = a.0 as f64 + (b.0 as f64 - a.0 as f64) * t;
    let g = a.1 as f64 + (b.1 as f64 - a.1 as f64) * t;
    let bl = a.2 as f64 + (b.2 as f64 - a.2 as f64) * t;
    Color::Rgb(r as u8, g as u8, bl as u8)
}

fn draw(ctx: &mut Context, a: &Analyzer) {
    let warm = (255u8, 150, 40);
    let mid = (255u8, 60, 90);
    let cool = (90u8, 80, 230);

    let radius = a.base_radius + a.bass * 8.0;

    // background concentric rings (subtle, dark gray)
    for k in 1..=6 {
        let rr = radius + 6.0 + k as f64 * 9.0;
        draw_circle(ctx, 0.0, 0.0, rr, 96, Color::Rgb(28, 28, 36));
    }

    // bands radiating from circle edge, mirrored inward
    for (i, band) in a.bands.iter().enumerate() {
        let ang = a.rotation + (i as f64 / BANDS as f64) * PI * 2.0;
        let (ca, sa) = (ang.cos(), ang.sin());

        let len = band.value * 22.0;
        let inner_len = band.value * 9.0; // mirror inward, shorter

        let x0 = ca * radius;
        let y0 = sa * radius;
        let x1 = ca * (radius + len);
        let y1 = sa * (radius + len);

        // outward gradient: split into segments to fake per-distance color
        let segs = 6;
        for s in 0..segs {
            let f0 = s as f64 / segs as f64;
            let f1 = (s + 1) as f64 / segs as f64;
            let sx0 = x0 + (x1 - x0) * f0;
            let sy0 = y0 + (y1 - y0) * f0;
            let sx1 = x0 + (x1 - x0) * f1;
            let sy1 = y0 + (y1 - y0) * f1;
            let col = if f0 < 0.5 {
                lerp_color(warm, mid, f0 * 2.0)
            } else {
                lerp_color(mid, cool, (f0 - 0.5) * 2.0)
            };
            ctx.draw(&Line {
                x1: sx0,
                y1: sy0,
                x2: sx1,
                y2: sy1,
                color: col,
            });
        }

        // inward mirror (warm core)
        let ix1 = ca * (radius - inner_len);
        let iy1 = sa * (radius - inner_len);
        ctx.draw(&Line {
            x1: x0,
            y1: y0,
            x2: ix1,
            y2: iy1,
            color: lerp_color(warm, mid, band.value),
        });
    }

    // main pulsing circle
    let circ_col = lerp_color((255, 180, 60), (255, 90, 110), a.bass);
    draw_circle(ctx, 0.0, 0.0, radius, 160, circ_col);

    // second smaller rotating concentric ring (dashed via gaps)
    let inner_r = radius * 0.55;
    draw_ring_dashed(ctx, inner_r, a.ring_rotation, Color::Rgb(120, 200, 255));
}

fn draw_circle(ctx: &mut Context, cx: f64, cy: f64, r: f64, steps: usize, color: Color) {
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(steps);
    for i in 0..steps {
        let a = (i as f64 / steps as f64) * PI * 2.0;
        pts.push((cx + a.cos() * r, cy + a.sin() * r));
    }
    ctx.draw(&Points {
        coords: &pts,
        color,
    });
}

fn draw_ring_dashed(ctx: &mut Context, r: f64, rot: f64, color: Color) {
    let steps = 120;
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for i in 0..steps {
        // dash pattern: skip every 4th group
        if (i / 4) % 2 == 0 {
            let a = rot + (i as f64 / steps as f64) * PI * 2.0;
            pts.push((a.cos() * r, a.sin() * r));
        }
    }
    ctx.draw(&Points {
        coords: &pts,
        color,
    });
}

pub fn run() -> io::Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let mut analyzer = Analyzer::new();
    let mut last = Instant::now();

    let result = (|| -> io::Result<()> {
        loop {
            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f64();
            last = now;
            analyzer.update(dt);

            term.draw(|f| {
                let area = f.area();
                // keep aspect roughly square: terminal cells are ~2:1 tall
                let bound = 60.0;
                let canvas = Canvas::default()
                    .block(Block::default())
                    .marker(symbols::Marker::Braille)
                    .x_bounds([-bound, bound])
                    .y_bounds([-bound / 2.0, bound / 2.0])
                    .paint(|ctx| draw(ctx, &analyzer));
                f.render_widget(canvas, area);
            })?;

            if event::poll(Duration::from_millis(16))? {
                if let Event::Key(k) = event::read()? {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    })();

    terminal::disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        terminal::LeaveAlternateScreen,
        cursor::Show
    )?;
    term.show_cursor()?;
    result
}
