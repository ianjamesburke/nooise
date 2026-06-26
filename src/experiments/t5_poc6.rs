use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute, terminal,
};
use rand::Rng;
use ratatui::prelude::*;
use ratatui::widgets::Widget;
use std::io;
use std::time::{Duration, Instant};

const BRAILLE_W: usize = 2;
const BRAILLE_H: usize = 4;
const BRAILLE_DOTS: [[u8; BRAILLE_H]; BRAILLE_W] = [
    [0x01, 0x02, 0x04, 0x40],
    [0x08, 0x10, 0x20, 0x80],
];

const MAX_PARTICLES: usize = 6000;
const EMITTERS: usize = 5;

struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    age: f32,
    life: f32,
    hue: f32,
}

struct FluidSim {
    t: f32,
    particles: Vec<Particle>,
    w: f32,
    h: f32,
    ripple: Option<(f32, f32, f32)>,
}

fn flow_at(t: f32, nx: f32, ny: f32, ripple: Option<(f32, f32, f32)>) -> (f32, f32) {
    let z = t * 0.4;
    let angle1 = (nx * 5.0 + z).sin() * (ny * 4.0 - z * 0.6).cos() * std::f32::consts::TAU;
    let angle2 = ((nx * 3.0 - ny * 3.5) + z * 1.1).sin() * std::f32::consts::PI;
    let angle3 = (nx * 8.0 + ny * 7.0 - z * 0.3).sin() * std::f32::consts::FRAC_PI_2;
    let angle = angle1 * 0.5 + angle2 * 0.3 + angle3 * 0.2;

    let strength1 = ((nx * 6.0 + z * 0.7).cos() * (ny * 5.0 + z).sin()).abs();
    let strength2 = ((nx + ny) * 4.0 + (z * 0.5).sin() * 3.0).cos().abs();
    let strength = 0.6 + (strength1 * 0.5 + strength2 * 0.5) * 0.8;

    let mut fx = angle.cos() * strength;
    let mut fy = angle.sin() * strength;

    let vortices = [
        (0.3 + (z * 0.15).sin() * 0.15, 0.4 + (z * 0.2).cos() * 0.15, 1.0_f32),
        (0.7 + (z * 0.12).cos() * 0.12, 0.6 + (z * 0.18).sin() * 0.1, -0.8),
        (0.5 + (z * 0.1).sin() * 0.2, 0.2 + (z * 0.25).cos() * 0.1, 0.6),
    ];
    for (vcx, vcy, spin) in vortices {
        let dx = nx - vcx;
        let dy = ny - vcy;
        let dist = (dx * dx + dy * dy).sqrt().max(0.02);
        let pull = (-dist * 8.0).exp() * spin * 2.0;
        fx += -dy * pull;
        fy += dx * pull;
    }

    if let Some((rx, ry, rage)) = ripple {
        let dx = nx - rx;
        let dy = ny - ry;
        let dist = (dx * dx + dy * dy).sqrt().max(0.01);
        let front = rage * 0.15;
        let ring = (-((dist - front) * 20.0).powi(2)).exp();
        let fade = (1.0 - rage / 4.0).max(0.0);
        let push = ring * fade * 3.0;
        if dist > 0.01 {
            fx += (dx / dist) * push;
            fy += (dy / dist) * push;
        }
    }

    (fx, fy)
}

fn field_intensity(t: f32, nx: f32, ny: f32) -> f32 {
    let z = t * 0.55;
    let mut v = 0.0_f32;
    v += (nx * 6.0 + z).sin() * (ny * 5.0 - z * 0.7).cos();
    v += ((nx * 3.3 - ny * 4.1) + z * 1.3).sin() * 0.7;
    v += (nx * 11.0 + ny * 9.0 - z * 0.4).sin() * 0.35;
    (v / 2.5).tanh() * 0.5 + 0.5
}

impl FluidSim {
    fn new(w: f32, h: f32) -> Self {
        Self {
            t: 0.0,
            particles: Vec::with_capacity(MAX_PARTICLES),
            w,
            h,
            ripple: None,
        }
    }

    fn emit(&mut self) {
        let mut rng = rand::thread_rng();
        let emitter_positions: Vec<(f32, f32)> = (0..EMITTERS)
            .map(|i| {
                let phase = i as f32 * std::f32::consts::TAU / EMITTERS as f32;
                let cx = 0.5 + (self.t * 0.3 + phase).sin() * 0.3;
                let cy = 0.5 + (self.t * 0.25 + phase).cos() * 0.3;
                (cx, cy)
            })
            .collect();

        for (ex, ey) in &emitter_positions {
            for _ in 0..4 {
                if self.particles.len() >= MAX_PARTICLES {
                    return;
                }
                let spread = 0.02;
                let px = (ex + rng.gen_range(-spread..spread)).clamp(0.0, 1.0) * self.w;
                let py = (ey + rng.gen_range(-spread..spread)).clamp(0.0, 1.0) * self.h;
                let hue = (ex * 360.0 + self.t * 20.0) % 360.0;
                self.particles.push(Particle {
                    x: px,
                    y: py,
                    vx: rng.gen_range(-2.0..2.0),
                    vy: rng.gen_range(-2.0..2.0),
                    age: 0.0,
                    life: rng.gen_range(3.0..7.0),
                    hue,
                });
            }
        }
    }

    fn step(&mut self, dt: f32) {
        self.t += dt;
        if let Some((_, _, ref mut age)) = self.ripple {
            *age += dt;
            if *age > 4.0 {
                self.ripple = None;
            }
        }

        self.emit();

        let field_strength = 40.0;
        let damping = 0.97_f32;
        let w = self.w;
        let h = self.h;
        let t = self.t;
        let ripple = self.ripple;

        for p in self.particles.iter_mut() {
            let nx = (p.x / w).clamp(0.0, 1.0);
            let ny = (p.y / h).clamp(0.0, 1.0);

            let (fx, fy) = flow_at(t, nx, ny, ripple);
            p.vx += fx * field_strength * dt;
            p.vy += fy * field_strength * dt;
            p.vx *= damping;
            p.vy *= damping;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.age += dt;

            if p.x < 0.0 { p.x += w; }
            if p.x >= w { p.x -= w; }
            if p.y < 0.0 { p.y += h; }
            if p.y >= h { p.y -= h; }
        }

        self.particles.retain(|p| p.age < p.life);
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

struct FluidWidget<'a> {
    sim: &'a FluidSim,
}

impl Widget for FluidWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let cols = area.width as usize;
        let rows = area.height as usize;
        let sub_w = cols * BRAILLE_W;
        let sub_h = rows * BRAILLE_H;
        if sub_w == 0 || sub_h == 0 {
            return;
        }

        for y in 0..rows {
            for x in 0..cols {
                let nx = x as f32 / cols as f32;
                let ny = y as f32 / rows as f32;
                let intensity = field_intensity(self.sim.t, nx, ny);
                let bg_val = (intensity * 18.0) as u8;
                buf[(area.x + x as u16, area.y + y as u16)]
                    .set_char(' ')
                    .set_style(Style::default().bg(Color::Rgb(bg_val, bg_val / 2, bg_val * 2)));
            }
        }

        let mut mask = vec![0u8; cols * rows];
        let mut col_r = vec![0u32; cols * rows];
        let mut col_g = vec![0u32; cols * rows];
        let mut col_b = vec![0u32; cols * rows];
        let mut weight = vec![0u32; cols * rows];

        for p in &self.sim.particles {
            let fade = 1.0 - (p.age / p.life).clamp(0.0, 1.0);
            let brightness = fade * 0.9 + 0.1;
            let (r, g, b) = hsv_to_rgb(p.hue + p.age * 15.0, 0.85, brightness);

            let speed = (p.vx * p.vx + p.vy * p.vy).sqrt().max(0.001);
            let (nx, ny) = (-p.vx / speed, -p.vy / speed);
            for (i, dim) in [(0.0_f32, 4u32), (1.0, 3), (2.0, 2), (3.0, 1)] {
                let tx = p.x + nx * i;
                let ty = p.y + ny * i;
                if tx < 0.0 || ty < 0.0 {
                    continue;
                }
                let txi = tx as usize;
                let tyi = ty as usize;
                if txi >= sub_w || tyi >= sub_h {
                    continue;
                }
                let cell = (tyi / BRAILLE_H) * cols + (txi / BRAILLE_W);
                let bx = txi % BRAILLE_W;
                let by = tyi % BRAILLE_H;
                mask[cell] |= BRAILLE_DOTS[bx][by];
                col_r[cell] += r as u32 * dim;
                col_g[cell] += g as u32 * dim;
                col_b[cell] += b as u32 * dim;
                weight[cell] += dim;
            }
        }

        for row in 0..rows {
            for col in 0..cols {
                let idx = row * cols + col;
                if mask[idx] == 0 {
                    continue;
                }
                let ch = char::from_u32(0x2800 + mask[idx] as u32).unwrap_or(' ');
                let w = weight[idx].max(1);
                let color = Color::Rgb(
                    (col_r[idx] / w).min(255) as u8,
                    (col_g[idx] / w).min(255) as u8,
                    (col_b[idx] / w).min(255) as u8,
                );
                buf[(area.x + col as u16, area.y + row as u16)]
                    .set_char(ch)
                    .set_style(Style::default().fg(color));
            }
        }
    }
}

pub fn run() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let size = terminal.size()?;
    let sub_w = size.width as f32 * BRAILLE_W as f32;
    let sub_h = (size.height.saturating_sub(1)) as f32 * BRAILLE_H as f32;
    let mut sim = FluidSim::new(sub_w, sub_h);
    let mut last = Instant::now();

    let result = loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05);
        last = now;

        sim.step(dt);

        let size = terminal.size()?;
        sim.w = size.width as f32 * BRAILLE_W as f32;
        sim.h = (size.height.saturating_sub(1)) as f32 * BRAILLE_H as f32;

        let count = sim.particles.len();
        if let Err(e) = terminal.draw(|f| {
            let area = f.area();
            let canvas = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
            f.render_widget(FluidWidget { sim: &sim }, canvas);

            let status = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
            let line = Line::from(vec![
                Span::styled(
                    " fluid ",
                    Style::default().fg(Color::Rgb(120, 200, 255)),
                ),
                Span::styled(
                    format!("· {count:>4} particles "),
                    Style::default().fg(Color::Rgb(100, 100, 100)),
                ),
                Span::styled(
                    "· [r] ripple · [q] quit",
                    Style::default().fg(Color::Rgb(70, 70, 70)),
                ),
            ]);
            f.render_widget(line, status);
        }) {
            break Err(e);
        }

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                        KeyCode::Char('r') => {
                            let mut rng = rand::thread_rng();
                            sim.ripple = Some((
                                rng.r#gen::<f32>(),
                                rng.r#gen::<f32>(),
                                0.0,
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
    };

    let _ = terminal::disable_raw_mode();
    let _ = execute!(io::stdout(), terminal::LeaveAlternateScreen, cursor::Show);
    result
}
