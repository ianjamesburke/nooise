use crossterm::{
    cursor,
    event::{self, Event, KeyCode},
    execute, terminal,
};
use rand::Rng;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Widget};
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

// Each terminal cell holds a 2x4 braille grid -> sub-cell resolution.
const BRAILLE_W: usize = 2;
const BRAILLE_H: usize = 4;

// Braille dot bit layout (Unicode pattern):
//   (0,0)=0x01  (1,0)=0x08
//   (0,1)=0x02  (1,1)=0x10
//   (0,2)=0x04  (1,2)=0x20
//   (0,3)=0x40  (1,3)=0x80
const BRAILLE_DOTS: [[u8; BRAILLE_H]; BRAILLE_W] = [
    [0x01, 0x02, 0x04, 0x40],
    [0x08, 0x10, 0x20, 0x80],
];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Fountain,
    Spiral,
    Explosion,
    Rain,
}

impl Mode {
    fn next(self) -> Self {
        match self {
            Mode::Fountain => Mode::Spiral,
            Mode::Spiral => Mode::Explosion,
            Mode::Explosion => Mode::Rain,
            Mode::Rain => Mode::Fountain,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Mode::Fountain => "fountain",
            Mode::Spiral => "spiral",
            Mode::Explosion => "explosion",
            Mode::Rain => "rain",
        }
    }
}

#[derive(Clone, Copy)]
struct Particle {
    // Position/velocity in sub-cell (braille dot) units.
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    age: f32,
    life: f32,
    splashed: bool,
    generation: u8,
}

const GRAVITY: f32 = 28.0; // dots / s^2

impl Particle {
    fn glyph(&self) -> char {
        let t = (self.age / self.life).clamp(0.0, 1.0);
        match (t * 6.0) as u32 {
            0 => '\u{25cf}', // ●
            1 => '\u{25c9}', // ◉
            2 => '\u{25cb}', // ○
            3 => '\u{25cc}', // ◌
            4 => '\u{00b7}', // ·
            _ => '\u{02d9}', // ˙
        }
    }

    // Fire palette: white -> yellow -> orange -> red -> dark red -> fade.
    fn color(&self) -> (u8, u8, u8) {
        let t = (self.age / self.life).clamp(0.0, 1.0);
        let stops: [(f32, (f32, f32, f32)); 6] = [
            (0.00, (255.0, 255.0, 245.0)),
            (0.20, (255.0, 240.0, 130.0)),
            (0.40, (255.0, 160.0, 40.0)),
            (0.62, (220.0, 60.0, 20.0)),
            (0.82, (110.0, 20.0, 10.0)),
            (1.00, (20.0, 5.0, 5.0)),
        ];
        let mut prev = stops[0];
        for s in stops.iter().skip(1) {
            if t <= s.0 {
                let span = (s.0 - prev.0).max(1e-4);
                let f = (t - prev.0) / span;
                let r = prev.1 .0 + (s.1 .0 - prev.1 .0) * f;
                let g = prev.1 .1 + (s.1 .1 - prev.1 .1) * f;
                let b = prev.1 .2 + (s.1 .2 - prev.1 .2) * f;
                return (r as u8, g as u8, b as u8);
            }
            prev = *s;
        }
        (20, 5, 5)
    }
}

struct Field {
    particles: Vec<Particle>,
    mode: Mode,
    spiral_phase: f32,
    explosion_timer: f32,
    // sub-cell dimensions
    w: f32,
    h: f32,
}

impl Field {
    fn new(w: usize, h: usize) -> Self {
        Field {
            particles: Vec::with_capacity(2048),
            mode: Mode::Fountain,
            spiral_phase: 0.0,
            explosion_timer: 0.0,
            w: w as f32,
            h: h as f32,
        }
    }

    fn resize(&mut self, w: usize, h: usize) {
        self.w = w as f32;
        self.h = h as f32;
    }

    fn emit(&mut self, dt: f32) {
        let mut rng = rand::thread_rng();
        let cx = self.w * 0.5;
        let bottom = self.h - 1.0;
        match self.mode {
            Mode::Fountain => {
                let n = (240.0 * dt) as usize + 1;
                for _ in 0..n {
                    if self.particles.len() >= 4000 {
                        break;
                    }
                    let angle = -std::f32::consts::FRAC_PI_2
                        + rng.gen_range(-0.45_f32..0.45);
                    let speed = rng.gen_range(34.0_f32..52.0);
                    self.particles.push(Particle {
                        x: cx + rng.gen_range(-1.0..1.0),
                        y: bottom,
                        vx: angle.cos() * speed,
                        vy: angle.sin() * speed,
                        age: 0.0,
                        life: rng.gen_range(1.6..2.8),
                        splashed: false,
                        generation: 0,
                    });
                }
            }
            Mode::Spiral => {
                self.spiral_phase += dt * 6.0;
                let arms = 3;
                for a in 0..arms {
                    let ang = self.spiral_phase
                        + a as f32 * std::f32::consts::TAU / arms as f32;
                    let speed = 44.0;
                    self.particles.push(Particle {
                        x: cx,
                        y: bottom,
                        vx: ang.cos() * speed * 0.6,
                        vy: -(ang.sin().abs() * 0.4 + 0.9) * speed,
                        age: 0.0,
                        life: rng.gen_range(1.8..2.6),
                        splashed: false,
                        generation: 0,
                    });
                }
            }
            Mode::Explosion => {
                self.explosion_timer -= dt;
                if self.explosion_timer <= 0.0 {
                    self.explosion_timer = 0.9;
                    let ex = rng.gen_range(self.w * 0.25..self.w * 0.75);
                    let ey = rng.gen_range(self.h * 0.3..self.h * 0.6);
                    let burst = 220;
                    for _ in 0..burst {
                        if self.particles.len() >= 4000 {
                            break;
                        }
                        let ang = rng.gen_range(0.0..std::f32::consts::TAU);
                        let speed = rng.gen_range(10.0..60.0);
                        self.particles.push(Particle {
                            x: ex,
                            y: ey,
                            vx: ang.cos() * speed,
                            vy: ang.sin() * speed,
                            age: 0.0,
                            life: rng.gen_range(1.2..2.4),
                            splashed: false,
                            generation: 0,
                        });
                    }
                }
            }
            Mode::Rain => {
                let n = (260.0 * dt) as usize + 1;
                for _ in 0..n {
                    if self.particles.len() >= 4000 {
                        break;
                    }
                    self.particles.push(Particle {
                        x: rng.gen_range(0.0..self.w),
                        y: 0.0,
                        vx: rng.gen_range(-3.0..3.0),
                        vy: rng.gen_range(18.0..30.0),
                        age: 0.0,
                        life: rng.gen_range(1.6..3.0),
                        splashed: false,
                        generation: 0,
                    });
                }
            }
        }
    }

    fn step(&mut self, dt: f32) {
        let bottom = self.h - 1.0;
        let mut splashes: Vec<Particle> = Vec::new();
        let mut rng = rand::thread_rng();

        for p in self.particles.iter_mut() {
            p.vy += GRAVITY * dt;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.age += dt;

            if p.y >= bottom && p.vy > 0.0 {
                p.y = bottom;
                if !p.splashed && p.generation == 0 && p.vy > 6.0 {
                    p.splashed = true;
                    let kids = rng.gen_range(2..=3);
                    for _ in 0..kids {
                        let ang = -std::f32::consts::FRAC_PI_2
                            + rng.gen_range(-1.0_f32..1.0);
                        let speed = p.vy.abs() * rng.gen_range(0.25..0.45);
                        splashes.push(Particle {
                            x: p.x,
                            y: bottom,
                            vx: ang.cos() * speed,
                            vy: ang.sin() * speed,
                            age: p.age * 0.5,
                            life: p.life * 0.5,
                            splashed: true,
                            generation: 1,
                        });
                    }
                }
                // kill on ground
                p.age = p.life + 1.0;
            }
        }

        self.particles.retain(|p| {
            p.age < p.life && p.x >= -2.0 && p.x < self.w + 2.0 && p.y < self.h + 2.0
        });
        self.particles.extend(splashes);
    }
}

struct FieldWidget<'a> {
    field: &'a Field,
}

impl Widget for FieldWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let sub_w = area.width as usize * BRAILLE_W;
        let sub_h = area.height as usize * BRAILLE_H;
        if sub_w == 0 || sub_h == 0 {
            return;
        }

        // Per terminal cell: accumulated braille mask + dominant color.
        let cols = area.width as usize;
        let rows = area.height as usize;
        let mut mask = vec![0u8; cols * rows];
        let mut col_r = vec![0u32; cols * rows];
        let mut col_g = vec![0u32; cols * rows];
        let mut col_b = vec![0u32; cols * rows];
        let mut weight = vec![0u32; cols * rows];

        let plot = |sx: f32,
                    sy: f32,
                    (r, g, b): (u8, u8, u8),
                    dim: u32,
                    mask: &mut [u8],
                    col_r: &mut [u32],
                    col_g: &mut [u32],
                    col_b: &mut [u32],
                    weight: &mut [u32]| {
            if sx < 0.0 || sy < 0.0 {
                return;
            }
            let sxi = sx as usize;
            let syi = sy as usize;
            if sxi >= sub_w || syi >= sub_h {
                return;
            }
            let cell = (syi / BRAILLE_H) * cols + (sxi / BRAILLE_W);
            let bx = sxi % BRAILLE_W;
            let by = syi % BRAILLE_H;
            mask[cell] |= BRAILLE_DOTS[bx][by];
            col_r[cell] += (r as u32) * dim;
            col_g[cell] += (g as u32) * dim;
            col_b[cell] += (b as u32) * dim;
            weight[cell] += dim;
        };

        for p in &self.field.particles {
            let c = p.color();
            // Trail: a couple of dimmer samples behind the velocity vector.
            let speed = (p.vx * p.vx + p.vy * p.vy).sqrt().max(1e-3);
            let (nx, ny) = (p.vx / speed, p.vy / speed);
            for (i, dim) in [(0.0_f32, 4u32), (1.4, 2), (2.8, 1)] {
                let tr = (
                    (c.0 as f32 * (dim as f32 / 4.0)) as u8,
                    (c.1 as f32 * (dim as f32 / 4.0)) as u8,
                    (c.2 as f32 * (dim as f32 / 4.0)) as u8,
                );
                plot(
                    p.x - nx * i,
                    p.y - ny * i,
                    tr,
                    dim,
                    &mut mask,
                    &mut col_r,
                    &mut col_g,
                    &mut col_b,
                    &mut weight,
                );
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
                    (col_r[idx] / w) as u8,
                    (col_g[idx] / w) as u8,
                    (col_b[idx] / w) as u8,
                );
                let x = area.x + col as u16;
                let y = area.y + row as u16;
                buf[(x, y)]
                    .set_char(ch)
                    .set_style(Style::default().fg(color));
            }
        }
    }
}

pub fn run() -> io::Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_loop(&mut terminal);

    let _ = terminal::disable_raw_mode();
    let _ = execute!(io::stdout(), terminal::LeaveAlternateScreen, cursor::Show);
    res
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let size = terminal.size()?;
    let mut field = Field::new(
        size.width as usize * BRAILLE_W,
        (size.height.saturating_sub(1)) as usize * BRAILLE_H,
    );

    let mut last = Instant::now();
    let mut mode_clock = 0.0_f32;
    let mode_interval = 6.0_f32;

    loop {
        if event::poll(Duration::from_millis(0))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char(' ') => {
                        field.mode = field.mode.next();
                        mode_clock = 0.0;
                    }
                    _ => {}
                }
            }
        }

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05);
        last = now;

        mode_clock += dt;
        if mode_clock >= mode_interval {
            mode_clock = 0.0;
            field.mode = field.mode.next();
        }

        field.emit(dt);
        field.step(dt);

        let count = field.particles.len();
        let mode = field.mode;

        terminal.draw(|f| {
            let area = f.area();
            let canvas = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
            field.resize(
                canvas.width as usize * BRAILLE_W,
                canvas.height as usize * BRAILLE_H,
            );
            f.render_widget(FieldWidget { field: &field }, canvas);

            let status = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
            let block = Block::default().borders(Borders::NONE);
            f.render_widget(block, status);
            let line = Line::from(vec![
                Span::styled(
                    format!(" {} ", mode.label()),
                    Style::default().fg(Color::Rgb(255, 180, 60)),
                ),
                Span::styled(
                    format!("· {count:>4} particles "),
                    Style::default().fg(Color::Rgb(120, 120, 120)),
                ),
                Span::styled(
                    "· [space] mode · [q] quit",
                    Style::default().fg(Color::Rgb(80, 80, 80)),
                ),
            ]);
            f.render_widget(line, status);
        })?;

        std::thread::sleep(Duration::from_millis(16));
    }
    Ok(())
}
