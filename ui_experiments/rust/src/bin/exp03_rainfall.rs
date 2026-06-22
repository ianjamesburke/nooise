// exp03_rainfall — non-functional visual prototype for a generative focus music engine.
//
// Concept: rain on a window. Vertical streams of characters fall at layer-specific
// speeds. Tonal notes drift down slowly, bilateral pulses drop fast as bright flashes,
// noise scatters as faint specks. Stereo position is read off which columns are active
// (a left/right bias that drifts over ~10s). Overall density breathes with a ~6s sine
// LFO. No audio — pure visual.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Frame, Terminal};

const FPS: u64 = 30;
const FRAME: Duration = Duration::from_millis(1000 / FPS);

const DENSITY_PERIOD: f32 = 6.0; // seconds, overall density LFO
const BIAS_PERIOD: f32 = 10.0; // seconds, left/right stereo drift

#[derive(Clone, Copy)]
enum Kind {
    Tonal,
    Bilateral,
    Noise,
}

impl Kind {
    // Vertical speed in rows per second.
    fn speed(self, rng: &mut StdRng) -> f32 {
        match self {
            Kind::Tonal => rng.gen_range(3.0..6.0),
            Kind::Bilateral => rng.gen_range(26.0..40.0),
            Kind::Noise => rng.gen_range(1.5..3.5),
        }
    }

    fn glyph(self, rng: &mut StdRng) -> char {
        match self {
            Kind::Tonal => {
                if rng.gen_bool(0.4) {
                    '┃'
                } else {
                    '│'
                }
            }
            Kind::Bilateral => {
                if rng.gen_bool(0.5) {
                    '◆'
                } else {
                    '•'
                }
            }
            Kind::Noise => {
                if rng.gen_bool(0.5) {
                    '·'
                } else {
                    '∙'
                }
            }
        }
    }
}

struct Drop {
    kind: Kind,
    col: u16,
    row: f32,
    speed: f32,
    glyph: char,
    // 0.0..=1.0 base brightness, used to pick a color ramp step.
    intensity: f32,
}

struct App {
    rng: StdRng,
    drops: Vec<Drop>,
    start: Instant,
    width: u16,
    height: u16,
}

impl App {
    fn new(width: u16, height: u16) -> Self {
        App {
            rng: StdRng::from_entropy(),
            drops: Vec::new(),
            start: Instant::now(),
            width,
            height,
        }
    }

    fn elapsed(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }

    // 0.0..=1.0, breathing density envelope.
    fn density(&self) -> f32 {
        let t = self.elapsed();
        let lfo = (t * std::f32::consts::TAU / DENSITY_PERIOD).sin();
        0.5 + 0.45 * lfo
    }

    // -1.0 (full left) .. 1.0 (full right), slow stereo drift.
    fn bias(&self) -> f32 {
        let t = self.elapsed();
        (t * std::f32::consts::TAU / BIAS_PERIOD).sin()
    }

    // Pick a column weighted toward the current stereo bias so active clusters
    // shift left/right over time.
    fn biased_col(&mut self) -> u16 {
        if self.width == 0 {
            return 0;
        }
        let bias = self.bias();
        let center = (0.5 + 0.5 * bias).clamp(0.0, 1.0) * (self.width as f32 - 1.0);
        // Gaussian-ish spread around the bias center.
        let spread = self.width as f32 * 0.30 + 1.0;
        let mut col = center + (self.rng.gen::<f32>() - 0.5) * spread * 2.0;
        col = col.clamp(0.0, self.width as f32 - 1.0);
        col as u16
    }

    fn spawn(&mut self) {
        let density = self.density();

        // Tonal: sparse, slow. A handful of sustained vertical streams.
        let tonal_rate = 0.20 * density;
        if self.rng.gen::<f32>() < tonal_rate {
            self.push(Kind::Tonal);
        }

        // Bilateral: fast bright flashes, come in occasional bursts.
        let bilateral_rate = 0.35 * density;
        if self.rng.gen::<f32>() < bilateral_rate {
            let burst = self.rng.gen_range(1..=2);
            for _ in 0..burst {
                self.push(Kind::Bilateral);
            }
        }

        // Noise: many faint specks, always present but thinner when quiet.
        let noise_rate = 0.9 * (0.4 + 0.6 * density);
        if self.rng.gen::<f32>() < noise_rate {
            let count = self.rng.gen_range(1..=3);
            for _ in 0..count {
                self.push(Kind::Noise);
            }
        }
    }

    fn push(&mut self, kind: Kind) {
        let col = self.biased_col();
        let speed = kind.speed(&mut self.rng);
        let glyph = kind.glyph(&mut self.rng);
        let intensity = match kind {
            Kind::Tonal => self.rng.gen_range(0.5..0.9),
            Kind::Bilateral => self.rng.gen_range(0.8..1.0),
            Kind::Noise => self.rng.gen_range(0.25..0.55),
        };
        self.drops.push(Drop {
            kind,
            col,
            row: -1.0,
            speed,
            glyph,
            intensity,
        });
    }

    fn update(&mut self, dt: f32) {
        self.spawn();
        let h = self.height as f32;
        for d in &mut self.drops {
            d.row += d.speed * dt;
        }
        self.drops.retain(|d| d.row < h + 1.0);
    }

    fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.drops.retain(|d| d.col < width);
    }
}

// Color ramps per kind, dim -> bright. Muted, calming palette.
fn tonal_color(i: f32) -> Color {
    // muted blue -> soft purple
    if i < 0.55 {
        Color::Indexed(60) // slate blue, dim
    } else if i < 0.8 {
        Color::Indexed(61) // medium blue-violet
    } else {
        Color::Indexed(98) // soft purple
    }
}

fn bilateral_color(i: f32) -> Color {
    // cyan -> white, bright
    if i < 0.85 {
        Color::Indexed(44) // cyan
    } else if i < 0.95 {
        Color::Indexed(51) // bright cyan
    } else {
        Color::Indexed(15) // white flash
    }
}

fn noise_color(i: f32) -> Color {
    // dim grays
    if i < 0.35 {
        Color::Indexed(236)
    } else if i < 0.5 {
        Color::Indexed(239)
    } else {
        Color::Indexed(242)
    }
}

struct RainWidget<'a> {
    app: &'a App,
}

impl<'a> Widget for RainWidget<'a> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for d in &self.app.drops {
            let row = d.row.floor();
            if row < 0.0 {
                continue;
            }
            let y = row as u16;
            if d.col >= area.width || y >= area.height {
                continue;
            }
            let (color, modifier) = match d.kind {
                Kind::Tonal => (tonal_color(d.intensity), Modifier::empty()),
                Kind::Bilateral => (bilateral_color(d.intensity), Modifier::BOLD),
                Kind::Noise => (noise_color(d.intensity), Modifier::DIM),
            };
            let cell = &mut buf[(area.x + d.col, area.y + y)];
            cell.set_char(d.glyph);
            cell.set_style(Style::default().fg(color).add_modifier(modifier));

            // Tonal streams leave a faint fading trail above the head.
            if matches!(d.kind, Kind::Tonal) && y > 0 {
                let trail_y = y - 1;
                let tcell = &mut buf[(area.x + d.col, area.y + trail_y)];
                if tcell.symbol() == " " {
                    tcell.set_char('╵');
                    tcell.set_style(Style::default().fg(Color::Indexed(59)));
                }
            }
        }
    }
}

fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(RainWidget { app }, area);

    // Quiet footer hint, dim so it does not distract.
    if area.height > 1 {
        let hint = Line::from(vec![Span::styled(
            " rainfall  ·  q / esc to quit ",
            Style::default()
                .fg(Color::Indexed(238))
                .add_modifier(Modifier::DIM),
        )]);
        let footer = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        f.render_widget(Paragraph::new(hint), footer);
    }
}

fn main() -> io::Result<()> {
    let mut terminal = setup()?;
    let size = terminal.size()?;
    let mut app = App::new(size.width, size.height);

    let mut last = Instant::now();
    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;

        // Track terminal resizes.
        let size = terminal.size()?;
        if size.width != app.width || size.height != app.height {
            app.resize(size.width, size.height);
        }

        app.update(dt);
        terminal.draw(|f| render(f, &app))?;

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

    teardown(terminal)
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
