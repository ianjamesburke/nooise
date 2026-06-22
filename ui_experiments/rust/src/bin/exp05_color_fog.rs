// exp05_color_fog — non-functional UI prototype for a generative focus music
// engine. Pure visual atmosphere: a full-screen RGB color field built from
// layered sine waves (Perlin-ish) that drifts slowly through deep ocean blues,
// purples, and occasional teal/green highlights. The least informational of the
// five — just fog to stare into, like deep water or a nebula. No audio.
//
// Each terminal cell is split into two vertical pixels via the upper half-block
// '▀': the foreground color paints the top pixel, the background the bottom,
// doubling vertical resolution.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, widgets::Paragraph};

const FRAME: Duration = Duration::from_millis(33); // ~30fps
const DRIFT_PERIOD: f64 = 12.0; // seconds for the field to drift one full cycle
const UPPER_HALF: &str = "▀";

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
    let w = area.width as usize;
    let h = area.height as usize;
    if w < 2 || h < 2 {
        return;
    }

    // Drift offsets: the whole field slides on a slow loop so it never settles.
    let drift = std::f64::consts::TAU * t / DRIFT_PERIOD;
    let dx = 6.0 * drift.cos();
    let dy = 4.0 * (drift * 0.5).sin();

    // Two slow LFOs nudge hue and overall brightness over the cycle, so active
    // "layers" feel like they swell and recede.
    let hue_lfo = (t * 0.13).sin() * 0.5 + 0.5; // 0..1
    let bright_lfo = 0.5 + 0.18 * (t * 0.21).sin();

    let mut lines: Vec<Line> = Vec::with_capacity(h);

    for row in 0..h {
        let mut spans: Vec<Span> = Vec::with_capacity(w);
        // Two vertical pixels per cell: top and bottom.
        let py_top = (row * 2) as f64;
        let py_bot = (row * 2 + 1) as f64;

        for col in 0..w {
            let px = col as f64;
            let top = color_at(px + dx, py_top + dy, t, hue_lfo, bright_lfo);
            let bot = color_at(px + dx, py_bot + dy, t, hue_lfo, bright_lfo);
            spans.push(Span::styled(
                UPPER_HALF,
                Style::default().fg(top).bg(bot),
            ));
        }
        lines.push(Line::from(spans));
    }

    f.render_widget(Paragraph::new(lines), area);
}

// Sample the fog field at a pixel coordinate. Layered sine waves at differing
// frequencies, phases, and drift speeds approximate smooth Perlin-like noise.
fn color_at(x: f64, y: f64, t: f64, hue_lfo: f64, bright_lfo: f64) -> Color {
    // Compress the vertical axis so fog reads as wide horizontal bands rather
    // than square cells (terminal cells are ~2:1 tall).
    let yc = y * 0.5;

    // Density field: several octaves summed and normalized to 0..1.
    let mut n = 0.0;
    n += (x * 0.06 + t * 0.30).sin();
    n += (yc * 0.10 - t * 0.22).sin();
    n += ((x * 0.04 + yc * 0.05) + t * 0.17).sin();
    n += ((x * 0.11 - yc * 0.07) - t * 0.13).sin() * 0.6;
    n += ((x * 0.02 + yc * 0.13) + t * 0.09).sin() * 0.8;
    n += ((x * 0.17 + yc * 0.03) + t * 0.40).sin() * 0.35;
    n /= 4.15; // normalize the summed amplitudes
    let density = (n * 0.5 + 0.5).clamp(0.0, 1.0); // 0..1

    // A second, slower field selects hue regions so blues bleed into purples
    // and the occasional teal/green pocket surfaces.
    let mut hf = 0.0;
    hf += (x * 0.03 - t * 0.11).sin();
    hf += (yc * 0.06 + t * 0.07).sin() * 0.7;
    hf += ((x * 0.05 + yc * 0.04) + t * 0.05).sin() * 0.5;
    hf /= 2.2;
    let hue_sel = (hf * 0.5 + 0.5).clamp(0.0, 1.0);

    palette(density, hue_sel, hue_lfo, bright_lfo)
}

// Deep ocean palette: near-black at low density, rising through indigo and
// violet, with a teal/green highlight where the hue selector and density both
// peak. Kept dark and calm to stay focus-friendly.
fn palette(density: f64, hue_sel: f64, hue_lfo: f64, bright_lfo: f64) -> Color {
    // Brightness eases in with density; the slow LFO breathes the whole field.
    let lum = (density.powf(1.4) * bright_lfo).clamp(0.0, 1.0);

    // Blend two anchor hues by the hue selector, modulated by the slow hue LFO.
    let mix = (hue_sel * 0.7 + hue_lfo * 0.3).clamp(0.0, 1.0);

    // Anchor A: deep ocean blue.  Anchor B: violet/purple.
    let (ar, ag, ab) = (20.0, 70.0, 150.0);
    let (br, bg, bb) = (95.0, 40.0, 165.0);
    let mut r = ar + (br - ar) * mix;
    let mut g = ag + (bg - ag) * mix;
    let mut b = ab + (bb - ab) * mix;

    // Teal/green highlight: surfaces only in bright pockets where the hue
    // selector sits in a narrow band. Rare, so it reads as a glint.
    let teal_band = (1.0 - ((hue_sel - 0.5).abs() * 4.0)).clamp(0.0, 1.0);
    let teal = teal_band * lum.powf(2.0);
    g += 90.0 * teal;
    b += 30.0 * teal;
    r -= 25.0 * teal;

    // Apply luminance and a dark floor so empty fog stays nearly black.
    let floor = 8.0;
    let r = (floor + (r - floor).max(0.0) * lum).clamp(0.0, 255.0) as u8;
    let g = (floor + (g - floor).max(0.0) * lum).clamp(0.0, 255.0) as u8;
    let b = (floor + 4.0 + (b - floor).max(0.0) * lum).clamp(0.0, 255.0) as u8;
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
