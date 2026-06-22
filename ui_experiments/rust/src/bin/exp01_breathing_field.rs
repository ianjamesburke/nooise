// exp01_breathing_field — non-functional UI prototype for a generative focus
// music engine. Pure visual: a full-screen field of Unicode dots that breathes
// in a sine cycle, denser at center on inhale, with left/right phase offset to
// hint at bilateral stereo panning. No audio.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

const BREATH_PERIOD: f64 = 4.0; // seconds per full inhale/exhale cycle
const PHASE_OFFSET: f64 = 0.5; // seconds the right half lags the left
const FRAME: Duration = Duration::from_millis(33); // ~30fps

// Density ramp: weight grows with local intensity.
const GLYPHS: [char; 5] = [' ', '·', '•', '●', '◉'];

fn main() -> io::Result<()> {
    let mut terminal = setup()?;
    let start = Instant::now();
    let result = run(&mut terminal, start);
    teardown(terminal)?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, start: Instant) -> io::Result<()> {
    loop {
        let t = start.elapsed().as_secs_f64();
        terminal.draw(|f| render(f, t))?;

        if event::poll(FRAME)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

fn render(f: &mut Frame, t: f64) {
    let area = f.area();
    let w = area.width as f64;
    let h = area.height as f64;
    if w < 2.0 || h < 2.0 {
        return;
    }

    let cx = (w - 1.0) / 2.0;
    let cy = (h - 1.0) / 2.0;
    // Normalize so the corner distance maps to ~1.0.
    let max_dist = (cx * cx + cy * cy).sqrt().max(1.0);

    // Two breath phases, left and right, offset in time for stereo feel.
    let two_pi = std::f64::consts::TAU;
    let breath_left = (two_pi * t / BREATH_PERIOD).sin();
    let breath_right = (two_pi * (t - PHASE_OFFSET) / BREATH_PERIOD).sin();

    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);

    for row in 0..area.height {
        let mut spans: Vec<Span> = Vec::with_capacity(area.width as usize);
        let dy = row as f64 - cy;

        for col in 0..area.width {
            let dx = col as f64 - cx;
            let dist = (dx * dx + dy * dy).sqrt() / max_dist; // 0 center .. 1 corner

            // Pick the breath phase for this half of the screen.
            let breath = if col as f64 <= cx {
                breath_left
            } else {
                breath_right
            };

            // Radial falloff: center is brightest. Breath shifts the radius of
            // the bright core in and out (inhale = expands outward).
            let core = 0.55 + 0.35 * breath; // breathing radius
            let intensity = (1.0 - (dist / core)).clamp(0.0, 1.0);

            // Soft edge shimmer so the field never looks static.
            let shimmer = 0.06 * ((dx * 0.7 + dy * 1.3 + t * 1.5).sin());
            let value = (intensity + shimmer).clamp(0.0, 1.0);

            let glyph_idx = (value * (GLYPHS.len() as f64 - 1.0)).round() as usize;
            let ch = GLYPHS[glyph_idx.min(GLYPHS.len() - 1)];

            let color = color_for(value, breath);
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
        }
        lines.push(Line::from(spans));
    }

    let title = format!(
        " breathing field  ·  L {:+.2}  R {:+.2}  ·  q to quit ",
        breath_left, breath_right
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title)
        .title_style(Style::default().fg(Color::Gray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Re-render the field clipped to the inner area.
    let field = Paragraph::new(clip_lines(lines, inner, area));
    f.render_widget(field, inner);
}

// Crop the full-area lines down to the bordered inner rect.
fn clip_lines<'a>(lines: Vec<Line<'a>>, inner: Rect, area: Rect) -> Vec<Line<'a>> {
    let top = (inner.y - area.y) as usize;
    let left = (inner.x - area.x) as usize;
    let height = inner.height as usize;
    let width = inner.width as usize;

    lines
        .into_iter()
        .skip(top)
        .take(height)
        .map(|line| {
            let spans: Vec<Span> = line.spans.into_iter().skip(left).take(width).collect();
            Line::from(spans)
        })
        .collect()
}

// Cool indigo at rest, warming toward the inhale peak. Keeps the palette calm
// and focus-friendly rather than alarming.
fn color_for(value: f64, breath: f64) -> Color {
    if value <= 0.0 {
        return Color::Rgb(8, 10, 18);
    }
    let warmth = (breath * 0.5 + 0.5).clamp(0.0, 1.0); // 0..1 across the cycle
    let base = value.powf(0.8);

    let r = (30.0 + 90.0 * warmth + 110.0 * base) as u8;
    let g = (40.0 + 30.0 * warmth + 150.0 * base) as u8;
    let b = (90.0 + 60.0 * (1.0 - warmth) + 120.0 * base) as u8;
    Color::Rgb(r, g, b)
}

fn setup() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn teardown(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    terminal.show_cursor()?;
    Ok(())
}
