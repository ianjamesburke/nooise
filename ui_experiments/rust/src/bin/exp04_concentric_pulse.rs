// exp04_concentric_pulse — non-functional UI prototype for a generative focus
// music engine. Pure visual: concentric rings expand outward from the terminal
// center like ripples in water. New rings spawn on a steady beat, expand at a
// tempo-tracked rate, and fade as they reach the edges. Alternating rings favor
// the left or right half to hint at bilateral stereo. No audio.

use std::collections::VecDeque;
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

const FRAME: Duration = Duration::from_millis(33); // ~30fps
const BEAT_PERIOD: f64 = 1.5; // seconds between new rings
const RING_SPEED: f64 = 0.22; // fraction of max radius traveled per second
const RING_THICKNESS: f64 = 0.10; // ring half-width as a fraction of max radius
const MAX_RINGS: usize = 16; // cap so old rings get retired

// Density ramp: weight grows with ring intensity. Empty space stays dark.
const GLYPHS: [char; 5] = [' ', '░', '▒', '▓', '█'];

// A single expanding ring. `radius` is normalized 0..1 (1 = corner distance).
struct Ring {
    birth: f64,       // seconds since start when the ring spawned
    lean_left: bool,  // which half glows brighter, for the bilateral feel
}

fn main() -> io::Result<()> {
    let mut terminal = setup()?;
    let start = Instant::now();
    let result = run(&mut terminal, start);
    teardown(terminal)?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, start: Instant) -> io::Result<()> {
    let mut rings: VecDeque<Ring> = VecDeque::new();
    let mut next_beat = 0.0_f64; // spawn the first ring immediately
    let mut beat_index = 0_u64;

    loop {
        let t = start.elapsed().as_secs_f64();

        // Spawn rings on the beat, alternating the bright half each time.
        while t >= next_beat {
            rings.push_back(Ring {
                birth: next_beat,
                lean_left: beat_index % 2 == 0,
            });
            beat_index += 1;
            next_beat += BEAT_PERIOD;
            if rings.len() > MAX_RINGS {
                rings.pop_front();
            }
        }

        // Retire rings that have fully expanded past the corners and faded.
        while rings
            .front()
            .map(|r| (t - r.birth) * RING_SPEED > 1.0 + RING_THICKNESS * 2.0)
            .unwrap_or(false)
        {
            rings.pop_front();
        }

        terminal.draw(|f| render(f, t, &rings, beat_index))?;

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

fn render(f: &mut Frame, t: f64, rings: &VecDeque<Ring>, beat_index: u64) {
    let area = f.area();
    let w = area.width as f64;
    let h = area.height as f64;
    if w < 2.0 || h < 2.0 {
        return;
    }

    let cx = (w - 1.0) / 2.0;
    let cy = (h - 1.0) / 2.0;
    // Terminal cells are ~twice as tall as wide; scale Y so rings read round.
    let aspect = 2.0;
    let max_dist = ((cx * cx) + (cy * aspect) * (cy * aspect))
        .sqrt()
        .max(1.0);

    let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);

    for row in 0..area.height {
        let mut spans: Vec<Span> = Vec::with_capacity(area.width as usize);
        let dy = (row as f64 - cy) * aspect;

        for col in 0..area.width {
            let dx = col as f64 - cx;
            let dist = (dx * dx + dy * dy).sqrt() / max_dist; // 0 center .. 1 corner
            let on_left = col as f64 <= cx;

            // Sum each ring's contribution at this radius.
            let mut value = 0.0_f64;
            for ring in rings {
                let age = t - ring.birth;
                let radius = age * RING_SPEED;
                if radius <= 0.0 {
                    continue;
                }

                // Gaussian-ish band centered on the ring's current radius.
                let delta = (dist - radius) / RING_THICKNESS;
                let band = (-(delta * delta)).exp();

                // Fade out as the ring travels toward the edge.
                let life = (1.0 - radius).clamp(0.0, 1.0).powf(0.7);

                // Bilateral weighting: brighter on the favored half.
                let side = if ring.lean_left == on_left { 1.0 } else { 0.45 };

                value += band * life * side;
            }

            // Gentle center glow so the origin never looks hollow.
            value += 0.18 * (1.0 - dist).clamp(0.0, 1.0).powf(2.0);

            let value = value.clamp(0.0, 1.0);
            let glyph_idx = (value * (GLYPHS.len() as f64 - 1.0)).round() as usize;
            let ch = GLYPHS[glyph_idx.min(GLYPHS.len() - 1)];

            let color = color_for(value, on_left);
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
        }
        lines.push(Line::from(spans));
    }

    let phase = (t / BEAT_PERIOD).fract();
    let title = format!(
        " concentric pulse  ·  beat {:>4}  ·  {}  ·  q to quit ",
        beat_index,
        beat_meter(phase),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(title)
        .title_style(Style::default().fg(Color::Gray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let field = Paragraph::new(clip_lines(lines, inner, area));
    f.render_widget(field, inner);
}

// A tiny ticking meter in the title: brightest right after each beat.
fn beat_meter(phase: f64) -> &'static str {
    if phase < 0.12 {
        "◉ ○ ○ ○"
    } else if phase < 0.35 {
        "○ ◉ ○ ○"
    } else if phase < 0.6 {
        "○ ○ ◉ ○"
    } else {
        "○ ○ ○ ◉"
    }
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

// Muted deep-blue to violet palette. The favored half leans a touch warmer
// (toward violet) so the bilateral split reads without breaking the calm.
fn color_for(value: f64, on_left: bool) -> Color {
    if value <= 0.0 {
        return Color::Rgb(6, 8, 16);
    }
    let v = value.powf(0.85);
    let violet = if on_left { 0.35 } else { 0.55 }; // subtle hue shift per half

    let r = (20.0 + 70.0 * violet * v + 60.0 * v) as u8;
    let g = (24.0 + 40.0 * v) as u8;
    let b = (60.0 + 150.0 * v) as u8;
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
