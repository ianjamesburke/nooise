use std::io;
use std::time::{Duration, Instant};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Terminal;

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const SKY_SHADES: [char; 4] = [' ', '░', '▒', '▓'];
const STAR_GLYPHS: [char; 3] = ['·', '✦', '✶'];

struct Star {
    x: u16,
    y: u16,
    phase: f32,
    speed: f32,
    glyph: char,
}

pub fn run() -> io::Result<()> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = render_loop(&mut terminal);

    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)?;
    terminal::disable_raw_mode()?;
    terminal.show_cursor()?;
    result
}

fn render_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let mut rng = StdRng::seed_from_u64(0xC0FFEE);
    let start = Instant::now();
    let frame = Duration::from_millis(50);
    let mut stars: Vec<Star> = Vec::new();
    let mut star_area: (u16, u16) = (0, 0);

    loop {
        if event::poll(frame)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    _ => {}
                }
            }
        }

        let t = start.elapsed().as_secs_f32();

        terminal.draw(|f| {
            let area = f.area();
            if star_area != (area.width, area.height) {
                stars = seed_stars(&mut rng, area);
                star_area = (area.width, area.height);
            }
            let buf = f.buffer_mut();
            paint(buf, area, t, &stars);
        })?;
    }
    Ok(())
}

fn seed_stars(rng: &mut StdRng, area: Rect) -> Vec<Star> {
    let horizon = (area.height as f32 * 0.45) as u16;
    let count = (area.width as usize * horizon.max(1) as usize) / 22;
    (0..count)
        .map(|_| Star {
            x: rng.gen_range(0..area.width.max(1)),
            y: rng.gen_range(0..horizon.max(1)),
            phase: rng.gen_range(0.0..std::f32::consts::TAU),
            speed: rng.gen_range(0.8..3.2),
            glyph: STAR_GLYPHS[rng.gen_range(0..STAR_GLYPHS.len())],
        })
        .collect()
}

fn lerp_color(a: (u8, u8, u8), b: (u8, u8, u8), f: f32) -> Color {
    let f = f.clamp(0.0, 1.0);
    Color::Rgb(
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * f) as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * f) as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * f) as u8,
    )
}

fn paint(buf: &mut Buffer, area: Rect, t: f32, stars: &[Star]) {
    let w = area.width;
    let h = area.height;
    if w == 0 || h == 0 {
        return;
    }
    let horizon = (h as f32 * 0.45) as u16;

    paint_sky(buf, area, horizon);
    paint_stars(buf, area, horizon, t, stars);
    paint_sun(buf, area, horizon, t);
    paint_terrain(buf, area, horizon, t);
}

fn paint_sky(buf: &mut Buffer, area: Rect, horizon: u16) {
    // dark blue at horizon fading to near-black at the top
    for y in 0..horizon {
        let f = y as f32 / horizon.max(1) as f32; // 0 top .. 1 horizon
        let bg = lerp_color((4, 2, 14), (40, 12, 70), f);
        // banded ░▒▓ texture, denser near horizon
        let band = (f * 3.999) as usize;
        let glyph = SKY_SHADES[band.min(3)];
        let fg = lerp_color((20, 10, 40), (90, 40, 130), f);
        for x in 0..area.width {
            let cell = &mut buf[(area.x + x, area.y + y)];
            cell.set_char(glyph);
            cell.set_fg(fg);
            cell.set_bg(bg);
        }
    }
}

fn paint_stars(buf: &mut Buffer, area: Rect, horizon: u16, t: f32, stars: &[Star]) {
    for s in stars {
        if s.y >= horizon {
            continue;
        }
        let tw = ((t * s.speed + s.phase).sin() * 0.5 + 0.5).powf(2.0);
        if tw < 0.18 {
            continue;
        }
        let fg = lerp_color((120, 110, 160), (255, 250, 255), tw);
        let cell = &mut buf[(area.x + s.x, area.y + s.y)];
        cell.set_char(s.glyph);
        cell.set_fg(fg);
    }
}

fn paint_sun(buf: &mut Buffer, area: Rect, horizon: u16, t: f32) {
    let cx = area.width as f32 * 0.5;
    let cy = horizon as f32 - 1.0;
    // terminal cells are ~2x tall; squash vertically
    let radius = (area.width as f32 * 0.16).clamp(4.0, 18.0);
    let drift = (t * 0.15).sin() * area.width as f32 * 0.12;

    for y in 0..horizon {
        for x in 0..area.width {
            let dx = (x as f32 - cx - drift) / radius;
            let dy = (y as f32 - cy) / (radius * 0.5);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= 1.0 {
                // horizontal scanline gaps near the bottom = retro sun
                let band = ((cy - y as f32) * 0.6) as i32;
                if y as f32 > cy - radius * 0.5 && band % 2 == 0 {
                    continue;
                }
                let grad = (y as f32 - (cy - radius * 0.5)) / radius;
                let fg = lerp_color((255, 230, 90), (255, 60, 150), grad.clamp(0.0, 1.0));
                let cell = &mut buf[(area.x + x, area.y + y)];
                cell.set_char('█');
                cell.set_fg(fg);
            }
        }
    }
}

fn terrain_height(wx: f32, depth: f32, t: f32) -> f32 {
    // layered sin/cos at different frequencies, phase-shifting over time
    let a = (wx * 0.25 + t * 0.8).sin() * 1.0;
    let b = (wx * 0.11 - depth * 0.3 + t * 0.5).cos() * 0.7;
    let c = (wx * 0.55 + depth * 0.15 + t * 1.6).sin() * 0.4;
    let d = (depth * 0.4 - t * 0.6).sin() * 0.5;
    (a + b + c + d) * 0.5 + 0.5
}

fn paint_terrain(buf: &mut Buffer, area: Rect, horizon: u16, t: f32) {
    let w = area.width;
    let h = area.height;
    let rows = h - horizon;
    if rows == 0 {
        return;
    }
    let cx = w as f32 * 0.5;

    // scroll terrain toward viewer
    let scroll = t * 4.0;

    for row in 0..rows {
        let screen_y = horizon + row;
        // perspective: near rows (bottom) sample fewer, wider; far rows compressed
        let depth_norm = 1.0 - row as f32 / rows as f32; // 1 at horizon, 0 at bottom
        let depth = depth_norm * 18.0 + scroll;
        // foreshortening factor: spread columns wider near the viewer
        let persp = 0.4 + (1.0 - depth_norm) * 1.4;

        for x in 0..w {
            let wx = (x as f32 - cx) / persp;
            let height = terrain_height(wx, depth, t);

            // height threshold rises with the row, carving a silhouette
            let level = (height * 7.999) as usize;
            let glyph = BLOCKS[level.min(7)];

            // deep purple valleys -> hot pink/cyan peaks
            let fg = if height > 0.78 {
                lerp_color((255, 70, 200), (90, 240, 255), (height - 0.78) / 0.22)
            } else {
                lerp_color((60, 18, 90), (255, 70, 200), height / 0.78)
            };
            let bg = lerp_color((10, 4, 24), (30, 8, 50), depth_norm);

            let cell = &mut buf[(area.x + x, area.y + screen_y)];
            cell.set_char(glyph);
            cell.set_fg(fg);
            cell.set_bg(bg);
        }

        // glowing grid lines that race toward the viewer
        let grid_phase = (depth * 0.5).fract();
        if grid_phase < 0.12 {
            for x in 0..w {
                let cell = &mut buf[(area.x + x, area.y + screen_y)];
                cell.set_fg(lerp_color((255, 70, 200), (90, 240, 255), depth_norm));
                if cell.symbol() == " " {
                    cell.set_char('─');
                }
            }
        }
    }
}
