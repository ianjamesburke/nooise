// exp02 — Layered Ribbons
// Non-functional visual prototype for a generative focus-music engine.
// Four braille wave ribbons stacked vertically, phase-shifting at
// polyrhythmic rates to evoke a tonal bed, bilateral pulse, noise
// texture and kick. No audio — pure motion.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    symbols::Marker,
    widgets::canvas::{Canvas, Points},
    widgets::{Block, Padding},
};

const FPS: u64 = 30;

struct Ribbon {
    /// fundamental oscillation rate in Hz — the polyrhythm comes from
    /// these staying mutually irrational-ish (0.3 / 0.5 / 0.7 / 1.1)
    freq: f64,
    /// vertical centre of the band, 0.0 (bottom) .. 1.0 (top)
    center: f64,
    /// peak vertical excursion as a fraction of total height
    amplitude: f64,
    /// horizontal wavelength scale — how many crests span the screen
    wavelength: f64,
    color: Color,
}

fn ribbons() -> Vec<Ribbon> {
    vec![
        // tonal bed — slow, wide, teal
        Ribbon {
            freq: 0.3,
            center: 0.78,
            amplitude: 0.085,
            wavelength: 1.4,
            color: Color::Rgb(72, 138, 142),
        },
        // bilateral pulse — muted blue
        Ribbon {
            freq: 0.5,
            center: 0.56,
            amplitude: 0.07,
            wavelength: 2.1,
            color: Color::Rgb(86, 116, 178),
        },
        // noise texture — quicker ripple, soft purple
        Ribbon {
            freq: 0.7,
            center: 0.34,
            amplitude: 0.06,
            wavelength: 3.3,
            color: Color::Rgb(134, 104, 176),
        },
        // kick — fastest, tight, indigo
        Ribbon {
            freq: 1.1,
            center: 0.16,
            amplitude: 0.055,
            wavelength: 4.4,
            color: Color::Rgb(96, 92, 168),
        },
    ]
}

/// Organic vertical offset for a ribbon at horizontal position `u`
/// (0..1 across the screen) and time `t`. Layered, mutually detuned
/// sines so the curve never reads as a clean textbook waveform.
fn wave(r: &Ribbon, u: f64, t: f64) -> f64 {
    let k = r.wavelength * std::f64::consts::TAU;
    let phase = r.freq * t * std::f64::consts::TAU;
    let primary = (k * u + phase).sin();
    let detune = 0.45 * (k * 1.73 * u - phase * 0.6 + 1.3).sin();
    let drift = 0.28 * (k * 0.41 * u + phase * 0.27).sin();
    let breathe = 0.85 + 0.15 * (t * 0.21 * std::f64::consts::TAU).sin();
    r.amplitude * breathe * (primary + detune + drift) / 1.73
}

fn draw(frame: &mut Frame, ribbons: &[Ribbon], t: f64) {
    let area = frame.area();
    let w = area.width.max(1) as f64;

    // braille gives 2x4 subcell resolution; oversample x accordingly
    let samples = (w * 2.0) as usize;

    let canvas = Canvas::default()
        .block(Block::default().padding(Padding::uniform(0)))
        .marker(Marker::Braille)
        .x_bounds([0.0, 1.0])
        .y_bounds([0.0, 1.0])
        .paint(move |ctx| {
            for r in ribbons {
                let mut coords: Vec<(f64, f64)> = Vec::with_capacity(samples * 3);
                // a little vertical body so the ribbon reads as a band,
                // not a hairline — three stacked strands, thinning toward edges
                let strands = [0.0_f64, 0.012, -0.012];
                for s in 0..samples {
                    let u = s as f64 / (samples - 1).max(1) as f64;
                    let y = r.center + wave(r, u, t);
                    for off in strands {
                        coords.push((u, y + off));
                    }
                }
                ctx.draw(&Points {
                    coords: &coords,
                    color: r.color,
                });
            }
        });

    frame.render_widget(canvas, area);
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let ribbons = ribbons();
    let frame_budget = Duration::from_millis(1000 / FPS);
    let start = Instant::now();

    loop {
        let t = start.elapsed().as_secs_f64();
        terminal.draw(|f| draw(f, &ribbons, t))?;

        let frame_start = Instant::now();
        while frame_start.elapsed() < frame_budget {
            let remaining = frame_budget.saturating_sub(frame_start.elapsed());
            if event::poll(remaining)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
