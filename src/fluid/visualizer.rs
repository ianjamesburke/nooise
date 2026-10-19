//! The ripple visualizer: chords drive the field colour, kicks spawn ripples.
//!
//! Driven entirely by live audio-thread telemetry. Presentation only — this
//! module never reaches back into the engine.

use std::cell::RefCell;

use super::*;

pub(crate) const FLUID_GRADIENT: &[char] = &[' ', '·', '∙', '•', '●', '◉', '⬤'];
pub(crate) const RIPPLE_LIFETIME: f32 = 3.0;
pub(crate) const RIPPLE_SPEED: f32 = 0.42; // normalized units / s

/// One chord = one hue. Cycles with the pad engine's 5-chord table.
pub(crate) fn hue_for_chord(index: u64) -> f32 {
    const HUES: [f32; 5] = [205.0, 270.0, 325.0, 158.0, 38.0];
    HUES[(index % HUES.len() as u64) as usize]
}

/// The visualizer's own animation state: field time, the live ripples kicks
/// have spawned, and the current chord hue. Nothing here reaches the audio
/// engine — it is presentation state driven by telemetry.
pub(crate) struct RippleField {
    t: f32,
    ripples: Vec<(f32, f32, f32)>, // (cx, cy, age) in 0..1 field coords
    last_kick: u64,
    hue: f32,
    cache: RefCell<Option<BackgroundFrame>>,
}

impl RippleField {
    pub(crate) fn new() -> Self {
        Self {
            t: 0.0,
            ripples: Vec::new(),
            last_kick: 0,
            hue: hue_for_chord(0),
            cache: RefCell::new(None),
        }
    }

    pub(crate) fn tick(&mut self, dt: f32, telemetry: &FluidTelemetry) {
        self.t += dt;

        // kick pulses -> ripples (golden-angle scatter so they don't stack)
        let kick = telemetry.kick_pulse.load(Ordering::Relaxed);
        if kick > self.last_kick {
            let new = (kick - self.last_kick).min(4);
            for k in 0..new {
                let n = (self.last_kick + k + 1) as f32;
                // Kick ripples originate along the bottom edge and radiate up,
                // keeping them clear of the centered control panel.
                let cx = (n * 0.618_034).fract();
                let cy = 0.92 + (n * 0.381_966).fract() * 0.06;
                self.ripples.push((cx.clamp(0.06, 0.94), cy, 0.0));
            }
            self.last_kick = kick;
        }

        for r in &mut self.ripples {
            r.2 += dt;
        }
        self.ripples.retain(|r| r.2 < RIPPLE_LIFETIME);
    }
}

/// Separable wave factors: trig work grows with width + height, not area.
struct WaveAxis {
    first: f32,
    second: (f32, f32),
    third: (f32, f32),
    fourth: (f32, f32),
}

impl WaveAxis {
    fn column(nx: f32, z: f32) -> Self {
        Self {
            first: (nx * 6.0 + z).sin(),
            second: (nx * 3.3 + z * 1.3).sin_cos(),
            third: (nx * 11.0 - z * 0.4).sin_cos(),
            fourth: (nx * 7.5 + (z * 0.9).sin() * 2.0).sin_cos(),
        }
    }

    fn row(ny: f32, z: f32) -> Self {
        Self {
            first: (ny * 5.0 - z * 0.7).cos(),
            second: (ny * 4.1).sin_cos(),
            third: (ny * 9.0).sin_cos(),
            fourth: (ny * 7.5).sin_cos(),
        }
    }

    fn value(&self, row: &Self) -> f32 {
        self.first * row.first
            + (self.second.0 * row.second.1 - self.second.1 * row.second.0) * 0.7
            + (self.third.0 * row.third.1 + self.third.1 * row.third.0) * 0.35
            + (self.fourth.1 * row.fourth.1 - self.fourth.0 * row.fourth.0) * 0.5
    }
}

/// Beyond three Gaussian widths, the omitted amplitude is at most
/// 1.6 * exp(-9), below one RGB step even before the scrim.
struct ActiveRing {
    cx: f32,
    cy: f32,
    age: f32,
    front: f32,
    inner_squared: f32,
    outer_squared: f32,
    amplitude: f32,
}

impl ActiveRing {
    fn new(cx: f32, cy: f32, age: f32) -> Self {
        let front = age * RIPPLE_SPEED;
        Self {
            cx,
            cy,
            age,
            front,
            inner_squared: (front - 0.25).max(0.0).powi(2),
            outer_squared: (front + 0.25).powi(2),
            amplitude: (1.0 - age / RIPPLE_LIFETIME).max(0.0) * 1.6,
        }
    }

    fn value(&self, nx: f32, ny: f32) -> f32 {
        let distance_squared = (nx - self.cx).powi(2) + (ny - self.cy).powi(2);
        if distance_squared < self.inner_squared || distance_squared > self.outer_squared {
            return 0.0;
        }
        let distance = distance_squared.sqrt();
        (distance * 34.0 - self.age * 9.0).sin()
            * (-((distance - self.front) * 12.0).powi(2)).exp()
            * self.amplitude
    }
}

const IDLE_BACKGROUND_SECONDS: f32 = 0.1;

struct BackgroundFrame {
    area: Rect,
    panel: Rect,
    stopped: bool,
    time: f32,
    hue: f32,
    cells: Vec<(char, Color)>,
}

/// The rung of a `len`-step glyph ladder a 0..1 level lands on, rounded to
/// nearest; the fluid gradient and the modulator lane glyphs share it.
pub(crate) fn ladder_index(level: f32, len: usize) -> usize {
    let last = len.saturating_sub(1);
    ((level.clamp(0.0, 1.0) * last as f32).round() as usize).min(last)
}

pub(crate) fn fluid_hsv(h: f32, s: f32, v: f32) -> Color {
    let h = h.rem_euclid(360.0);
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
    Color::Rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub(crate) struct FluidWidget<'a> {
    pub(crate) fluid: &'a RippleField,
    pub(crate) panel: Rect,
    pub(crate) stopped: bool,
}

impl FluidWidget<'_> {
    fn background(&self, area: Rect) -> BackgroundFrame {
        let w = area.width.max(1) as f32;
        let h = area.height.max(1) as f32;
        let z = self.fluid.t * 0.5;
        let columns: Vec<_> = (0..area.width)
            .map(|x| WaveAxis::column(x as f32 / w, z))
            .collect();
        let rings: Vec<_> = self
            .fluid
            .ripples
            .iter()
            .map(|&(cx, cy, age)| ActiveRing::new(cx, cy, age))
            .collect();
        let mut cells = Vec::with_capacity(area.width as usize * area.height as usize);
        for y in 0..area.height {
            let ny = y as f32 / h;
            let row = WaveAxis::row(ny, z);
            for (x, column) in columns.iter().enumerate() {
                let nx = x as f32 / w;
                let covered = self.panel.contains((area.x + x as u16, area.y + y).into());
                let mut wave = column.value(&row);
                // The panel erases glyphs and strongly dims the field. Keep its
                // moving base tint, but skip the hidden kick detail and glyphs.
                if !covered {
                    wave += rings.iter().map(|ring| ring.value(nx, ny)).sum::<f32>();
                }
                let v = (wave / 3.0).tanh() * 0.5 + 0.5;
                let edge_x = (nx.min(1.0 - nx) * 2.0).min(1.0);
                let edge_y = (ny.min(1.0 - ny) * 2.0).min(1.0);
                let vig = (edge_x.min(edge_y) * 1.4).clamp(0.2, 1.0);
                let hue = self.fluid.hue + (v - 0.5) * 45.0;
                let sat = (0.5 + v * 0.3).clamp(0.0, 1.0);
                let val = ((0.12 + v * 0.8) * vig).clamp(0.0, 1.0);
                let glyph = if covered {
                    ' '
                } else {
                    FLUID_GRADIENT[ladder_index(v, FLUID_GRADIENT.len())]
                };
                cells.push((glyph, quantize_rgb(fluid_hsv(hue, sat, val))));
            }
        }
        BackgroundFrame {
            area,
            panel: self.panel,
            stopped: self.stopped,
            time: self.fluid.t,
            hue: self.fluid.hue,
            cells,
        }
    }
}

impl Widget for FluidWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Memoization of presentation only. Input/session projection and the
        // foreground scheduler retain their normal cadence while stopped.
        let mut cache = self.fluid.cache.borrow_mut();
        let reusable = cache.as_ref().is_some_and(|frame| {
            frame.area == area
                && frame.panel == self.panel
                && frame.stopped == self.stopped
                && frame.hue == self.fluid.hue
                && (frame.time == self.fluid.t
                    || (self.stopped
                        && self.fluid.t >= frame.time
                        && self.fluid.t - frame.time < IDLE_BACKGROUND_SECONDS))
        });
        if !reusable {
            *cache = Some(self.background(area));
        }
        if let Some(frame) = cache.as_ref() {
            for (index, &(glyph, color)) in frame.cells.iter().enumerate() {
                let x = index % area.width as usize;
                let y = index / area.width as usize;
                buf[(area.x + x as u16, area.y + y as u16)]
                    .set_char(glyph)
                    .set_style(Style::default().fg(color));
            }
        }
    }
}

/// Four-code buckets suppress sub-visible truecolor jitter without changing
/// the shared HSV helper used by foreground controls.
fn quantize_rgb(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(r & !3, g & !3, b & !3),
        other => other,
    }
}

/// Multiply an RGB colour toward black; non-RGB passes through unchanged.
pub(crate) fn darken(c: Color, factor: f32) -> Color {
    if let Color::Rgb(r, g, b) = c {
        Color::Rgb(
            (r as f32 * factor) as u8,
            (g as f32 * factor) as u8,
            (b as f32 * factor) as u8,
        )
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn original_wave(nx: f32, ny: f32, time: f32) -> f32 {
        let z = time * 0.5;
        (nx * 6.0 + z).sin() * (ny * 5.0 - z * 0.7).cos()
            + ((nx * 3.3 - ny * 4.1) + z * 1.3).sin() * 0.7
            + (nx * 11.0 + ny * 9.0 - z * 0.4).sin() * 0.35
            + ((nx + ny) * 7.5 + (z * 0.9).sin() * 2.0).cos() * 0.5
    }

    #[test]
    fn separated_waves_preserve_the_original_field() {
        for time in [0.0, 1.5, 30.0, 300.0, 3600.0] {
            for y in 0..40 {
                for x in 0..120 {
                    let nx = x as f32 / 120.0;
                    let ny = y as f32 / 40.0;
                    let wave =
                        WaveAxis::column(nx, time * 0.5).value(&WaveAxis::row(ny, time * 0.5));
                    assert!((wave - original_wave(nx, ny, time)).abs() < 0.0005);
                }
            }
        }
    }

    #[test]
    fn ring_culling_keeps_visible_contributions_and_bounds_omitted_tails() {
        let mut culled = 0;
        for age in [0.0, 0.5, 1.0, 2.5, 2.99] {
            let ring = ActiveRing::new(0.4, 0.95, age);
            for y in 0..40 {
                for x in 0..120 {
                    let nx = x as f32 / 120.0;
                    let ny = y as f32 / 40.0;
                    let distance = ((nx - ring.cx).powi(2) + (ny - ring.cy).powi(2)).sqrt();
                    let original = (distance * 34.0 - age * 9.0).sin()
                        * (-((distance - age * RIPPLE_SPEED) * 12.0).powi(2)).exp()
                        * (1.0 - age / RIPPLE_LIFETIME).max(0.0)
                        * 1.6;
                    let actual = ring.value(nx, ny);
                    if actual == 0.0 {
                        culled += 1;
                    }
                    assert!((actual - original).abs() < 0.0002);
                }
            }
        }
        assert!(culled > 120 * 40 * 2);
    }

    fn draw(field: &RippleField, area: Rect, panel: Rect, stopped: bool) -> Buffer {
        let mut buffer = Buffer::empty(area);
        FluidWidget {
            fluid: field,
            panel,
            stopped,
        }
        .render(area, &mut buffer);
        buffer
    }

    #[test]
    fn panel_skips_ripples_and_glyphs_but_keeps_animated_tint() {
        let area = Rect::new(3, 7, 100, 40);
        let panel = Rect::new(20, 10, 60, 30);
        let mut field = RippleField::new();
        let base = draw(&field, area, panel, false);
        field.ripples.push((0.1, 0.5, 0.1));
        field.cache.replace(None);
        let rippled = draw(&field, area, panel, false);
        let mut outside_changes = 0;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if panel.contains((x, y).into()) {
                    assert_eq!(base[(x, y)], rippled[(x, y)]);
                    assert_eq!(rippled[(x, y)].symbol(), " ");
                } else if base[(x, y)] != rippled[(x, y)] {
                    outside_changes += 1;
                }
            }
        }
        assert!(outside_changes > 0);
        field.t = 1.0;
        let advanced = draw(&field, area, panel, false);
        assert_ne!(advanced[(40, 20)].fg, base[(40, 20)].fg);
    }

    #[test]
    fn stopped_background_is_cached_but_resumes_and_resizes_immediately() {
        let area = Rect::new(0, 0, 80, 24);
        let mut field = RippleField::new();
        let first = draw(&field, area, Rect::default(), true);
        field.t = 0.033;
        assert_eq!(first, draw(&field, area, Rect::default(), true));
        field.t = 0.066;
        assert_eq!(first, draw(&field, area, Rect::default(), true));
        field.t = 0.101;
        let next = draw(&field, area, Rect::default(), true);
        assert_ne!(first, next);
        field.t = 0.134;
        assert_ne!(next, draw(&field, area, Rect::default(), false));
        let resized = Rect::new(2, 4, 46, 11);
        assert_eq!(draw(&field, resized, resized, true).area, resized);
        assert_eq!(field.cache.borrow().as_ref().unwrap().area, resized);
        assert!(
            draw(&field, Rect::default(), Rect::default(), true)
                .content
                .is_empty()
        );
    }

    #[test]
    fn rgb_buckets_reduce_terminal_cell_changes() {
        let area = Rect::new(0, 0, 120, 40);
        let mut previous_raw = Vec::new();
        let mut previous_quantized = Vec::new();
        let mut raw_changes = 0;
        let mut quantized_changes = 0;
        for frame in 0..30 {
            let field = RippleField {
                t: frame as f32 * 0.033,
                ..RippleField::new()
            };
            let output = draw(&field, area, Rect::default(), false);
            let mut raw = Vec::new();
            for y in 0..area.height {
                for x in 0..area.width {
                    let nx = x as f32 / area.width as f32;
                    let ny = y as f32 / area.height as f32;
                    let v = (original_wave(nx, ny, field.t) / 3.0).tanh() * 0.5 + 0.5;
                    let vig = ((nx.min(1.0 - nx) * 2.0).min(ny.min(1.0 - ny) * 2.0) * 1.4)
                        .clamp(0.2, 1.0);
                    raw.push((
                        FLUID_GRADIENT[ladder_index(v, FLUID_GRADIENT.len())],
                        fluid_hsv(
                            field.hue + (v - 0.5) * 45.0,
                            0.5 + v * 0.3,
                            (0.12 + v * 0.8) * vig,
                        ),
                    ));
                }
            }
            let quantized: Vec<_> = output
                .content
                .iter()
                .map(|cell| (cell.symbol().chars().next().unwrap(), cell.fg))
                .collect();
            if frame > 0 {
                raw_changes += raw
                    .iter()
                    .zip(&previous_raw)
                    .filter(|(a, b)| a != b)
                    .count();
                quantized_changes += quantized
                    .iter()
                    .zip(&previous_quantized)
                    .filter(|(a, b)| a != b)
                    .count();
            }
            previous_raw = raw;
            previous_quantized = quantized;
        }
        eprintln!("background cell changes: original={raw_changes}, quantized={quantized_changes}");
        assert!(quantized_changes * 2 < raw_changes);
    }

    #[test]
    #[ignore = "release-mode visualizer timing probe"]
    fn visualizer_render_benchmark() {
        let area = Rect::new(0, 0, 160, 50);
        let panel = Rect::new(30, 2, 99, 46);
        let mut field = RippleField::new();
        field.ripples = vec![(0.3, 0.95, 0.3), (0.6, 0.93, 1.0), (0.9, 0.96, 2.0)];
        let mut buffer = Buffer::empty(area);
        let start = Instant::now();
        for frame in 0..300 {
            field.t = frame as f32 * 0.033;
            FluidWidget {
                fluid: &field,
                panel,
                stopped: false,
            }
            .render(area, &mut buffer);
            std::hint::black_box(&buffer);
        }
        eprintln!("optimized 300 frames: {:?}", start.elapsed());
        let start = Instant::now();
        for frame in 0..300 {
            for y in 0..area.height {
                for x in 0..area.width {
                    let nx = x as f32 / area.width as f32;
                    let ny = y as f32 / area.height as f32;
                    let mut wave = original_wave(nx, ny, frame as f32 * 0.033);
                    for &(cx, cy, age) in &field.ripples {
                        let dist = ((nx - cx).powi(2) + (ny - cy).powi(2)).sqrt();
                        wave += (dist * 34.0 - age * 9.0).sin()
                            * (-((dist - age * RIPPLE_SPEED) * 12.0).powi(2)).exp()
                            * (1.0 - age / RIPPLE_LIFETIME).max(0.0)
                            * 1.6;
                    }
                    let v = (wave / 3.0).tanh() * 0.5 + 0.5;
                    let vig = ((nx.min(1.0 - nx) * 2.0).min(ny.min(1.0 - ny) * 2.0) * 1.4)
                        .clamp(0.2, 1.0);
                    buffer[(x, y)]
                        .set_char(FLUID_GRADIENT[ladder_index(v, FLUID_GRADIENT.len())])
                        .set_style(Style::default().fg(fluid_hsv(
                            field.hue + (v - 0.5) * 45.0,
                            0.5 + v * 0.3,
                            (0.12 + v * 0.8) * vig,
                        )));
                }
            }
            std::hint::black_box(&buffer);
        }
        eprintln!("original 300 frames: {:?}", start.elapsed());
    }
}
