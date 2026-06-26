use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute, terminal,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Widget};

const ORB_COUNT: usize = 7;
const GRAVITY: f32 = 6.0;
const RESTITUTION: f32 = 0.82;
const TRAIL_DECAY: f32 = 0.86;
const TARGET_FPS: u64 = 60;

// Braille dots: each terminal cell is a 2x4 grid of sub-pixels.
const BRAILLE_BASE: u32 = 0x2800;
const BRAILLE_DOTS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

struct Orb {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    base_radius: f32,
    radius: f32,
    freq: f32,
    phase: f32,
    color: (u8, u8, u8),
}

impl Orb {
    fn new(rng: &mut StdRng, w: f32, h: f32, idx: usize) -> Self {
        let hue = idx as f32 / ORB_COUNT as f32;
        let color = hsv_to_rgb(hue, 0.85, 1.0);
        Orb {
            x: rng.gen_range(w * 0.2..w * 0.8),
            y: rng.gen_range(h * 0.1..h * 0.5),
            vx: rng.gen_range(-12.0..12.0),
            vy: rng.gen_range(-6.0..6.0),
            base_radius: rng.gen_range(3.5..6.5),
            radius: 4.0,
            freq: rng.gen_range(0.8..3.2),
            phase: rng.gen_range(0.0..std::f32::consts::TAU),
            color,
        }
    }

    fn update(&mut self, dt: f32, t: f32, w: f32, h: f32) {
        self.vy += GRAVITY * dt;
        self.x += self.vx * dt;
        self.y += self.vy * dt;

        let r = self.radius;
        if self.x - r < 0.0 {
            self.x = r;
            self.vx = self.vx.abs() * RESTITUTION;
        } else if self.x + r > w {
            self.x = w - r;
            self.vx = -self.vx.abs() * RESTITUTION;
        }
        if self.y - r < 0.0 {
            self.y = r;
            self.vy = self.vy.abs() * RESTITUTION;
        } else if self.y + r > h {
            self.y = h - r;
            self.vy = -self.vy.abs() * RESTITUTION;
            // floor kick keeps them alive against gravity
            self.vy -= 22.0;
        }

        // simulated audio amplitude pulses the size
        let amp = (t * self.freq + self.phase).sin() * 0.5 + 0.5;
        let amp2 = (t * self.freq * 2.3 + self.phase).sin() * 0.25;
        self.radius = self.base_radius * (0.7 + 0.6 * amp + amp2);
    }
}

/// Sub-pixel canvas: braille for orb bodies, RGB color per terminal cell.
struct Canvas {
    cw: usize, // cells wide
    ch: usize, // cells tall
    sw: usize, // sub-pixels wide (cw*2)
    sh: usize, // sub-pixels tall (ch*4)
    dots: Vec<u8>,
    color: Vec<(f32, f32, f32)>, // accumulated brightness per cell
}

impl Canvas {
    fn new(cw: usize, ch: usize) -> Self {
        Canvas {
            cw,
            ch,
            sw: cw * 2,
            sh: ch * 4,
            dots: vec![0u8; cw * ch],
            color: vec![(0.0, 0.0, 0.0); cw * ch],
        }
    }

    fn fade(&mut self) {
        self.dots.iter_mut().for_each(|d| *d = 0);
        for c in self.color.iter_mut() {
            c.0 *= TRAIL_DECAY;
            c.1 *= TRAIL_DECAY;
            c.2 *= TRAIL_DECAY;
        }
    }

    fn plot(&mut self, sx: i32, sy: i32, color: (u8, u8, u8), intensity: f32) {
        if sx < 0 || sy < 0 || sx as usize >= self.sw || sy as usize >= self.sh {
            return;
        }
        let (sx, sy) = (sx as usize, sy as usize);
        let cell = (sy / 4) * self.cw + (sx / 2);
        self.dots[cell] |= BRAILLE_DOTS[sy % 4][sx % 2];
        // blend: keep the brighter contribution
        let add = (
            color.0 as f32 * intensity,
            color.1 as f32 * intensity,
            color.2 as f32 * intensity,
        );
        let c = &mut self.color[cell];
        c.0 = c.0.max(add.0);
        c.1 = c.1.max(add.1);
        c.2 = c.2.max(add.2);
    }

    fn draw_orb(&mut self, orb: &Orb) {
        // orb coords are in sub-pixel space
        let r = orb.radius * 2.0; // scale radius into sub-pixel units (x dense)
        let ry = orb.radius; // y has 4 sub-rows so already dense
        let cx = orb.x * 2.0;
        let cy = orb.y * 4.0;
        let r2 = r * r;
        let y0 = (cy - ry * 4.0).floor() as i32;
        let y1 = (cy + ry * 4.0).ceil() as i32;
        let x0 = (cx - r).floor() as i32;
        let x1 = (cx + r).ceil() as i32;
        for sy in y0..=y1 {
            for sx in x0..=x1 {
                let dx = sx as f32 - cx;
                let dy = (sy as f32 - cy) * (r / (ry * 4.0)); // normalize aspect
                let d2 = dx * dx + dy * dy;
                if d2 <= r2 {
                    let edge = 1.0 - (d2 / r2).sqrt();
                    let intensity = 0.35 + 0.65 * edge; // brighter core
                    self.plot(sx, sy, orb.color, intensity);
                }
            }
        }
    }
}

impl Widget for &Canvas {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for cy in 0..self.ch.min(area.height as usize) {
            for cx in 0..self.cw.min(area.width as usize) {
                let idx = cy * self.cw + cx;
                let px = area.x + cx as u16;
                let py = area.y + cy as u16;
                let cell = &mut buf[(px, py)];

                let dot = self.dots[idx];
                let (r, g, b) = self.color[idx];
                let lum = (r + g + b) / 3.0;

                if dot != 0 && lum > 2.0 {
                    let ch = char::from_u32(BRAILLE_BASE + dot as u32).unwrap_or(' ');
                    cell.set_char(ch);
                    cell.set_fg(Color::Rgb(
                        r.min(255.0) as u8,
                        g.min(255.0) as u8,
                        b.min(255.0) as u8,
                    ));
                } else if lum > 1.0 {
                    // fading trail residue rendered as dim block
                    cell.set_char('░');
                    let f = (lum * 0.6).min(120.0) as u8;
                    cell.set_fg(Color::Rgb(
                        (r * 0.5).min(f as f32) as u8,
                        (g * 0.5).min(f as f32) as u8,
                        (b * 0.5).min(f as f32) as u8,
                    ));
                } else {
                    // subtle vertical background gradient
                    let t = cy as f32 / self.ch.max(1) as f32;
                    let bg = 6 + (t * 14.0) as u8;
                    let shade = if (cx + cy) % 7 == 0 { '·' } else { ' ' };
                    cell.set_char(shade);
                    cell.set_fg(Color::Rgb(bg / 2, bg / 2, bg + 6));
                }
                cell.set_bg(Color::Rgb(2, 2, 6));
            }
        }
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

pub fn run() -> io::Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    let res = run_loop(&mut stdout);

    execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    res
}

fn run_loop(stdout: &mut Stdout) -> io::Result<()> {
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let size = terminal.size()?;
    let (cw, ch) = (size.width as usize, size.height as usize);
    let mut canvas = Canvas::new(cw, ch);

    let mut rng = StdRng::from_entropy();
    let (sw, sh) = (canvas.sw as f32, canvas.sh as f32);
    // orbs live in cell-space (x: 0..cw, y: 0..ch) but drawn at sub-pixel density
    let (ow, oh) = (cw as f32, ch as f32);
    let mut orbs: Vec<Orb> = (0..ORB_COUNT)
        .map(|i| Orb::new(&mut rng, ow, oh, i))
        .collect();

    let frame_budget = Duration::from_millis(1000 / TARGET_FPS);
    let start = Instant::now();
    let mut last = start;

    loop {
        if event::poll(frame_budget)? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press
                    && matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                {
                    break;
                }
            }
        }

        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05);
        last = now;
        let t = (now - start).as_secs_f32();

        // handle resize
        let cur = terminal.size()?;
        if cur.width as usize != canvas.cw || cur.height as usize != canvas.ch {
            canvas = Canvas::new(cur.width as usize, cur.height as usize);
        }

        canvas.fade();
        for orb in orbs.iter_mut() {
            orb.update(dt, t, canvas.cw as f32, canvas.ch as f32);
            canvas.draw_orb(orb);
        }
        let _ = (sw, sh);

        terminal.draw(|f| {
            let area = f.area();
            Block::default().render(area, f.buffer_mut());
            f.render_widget(&canvas, area);
        })?;
    }

    Ok(())
}
