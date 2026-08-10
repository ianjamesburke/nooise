use super::*;

// ============================================================
// Fluid visualizer: chords drive the field colour, kicks
// spawn ripples. Driven entirely by live audio-thread telemetry.
// ============================================================

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
}

impl RippleField {
    pub(crate) fn new() -> Self {
        Self {
            t: 0.0,
            ripples: Vec::new(),
            last_kick: 0,
            hue: hue_for_chord(0),
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

    /// Liquid field value in 0..1 at normalized coords, with ripple distortion.
    pub(crate) fn field(&self, nx: f32, ny: f32) -> f32 {
        let z = self.t * 0.5;
        let mut v = 0.0;
        v += (nx * 6.0 + z).sin() * (ny * 5.0 - z * 0.7).cos();
        v += ((nx * 3.3 - ny * 4.1) + z * 1.3).sin() * 0.7;
        v += (nx * 11.0 + ny * 9.0 - z * 0.4).sin() * 0.35;
        v += ((nx + ny) * 7.5 + (z * 0.9).sin() * 2.0).cos() * 0.5;

        for &(cx, cy, age) in &self.ripples {
            let dx = nx - cx;
            let dy = ny - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let front = age * RIPPLE_SPEED;
            let fade = (1.0 - age / RIPPLE_LIFETIME).max(0.0);
            // small, tight ripple rising from the bottom edge
            let ring = (-((dist - front) * 12.0).powi(2)).exp();
            v += (dist * 34.0 - age * 9.0).sin() * ring * fade * 1.6;
        }

        (v / 3.0).tanh() * 0.5 + 0.5
    }
}

pub(crate) fn fluid_hsv(h: f32, s: f32, v: f32) -> Color {
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
    Color::Rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

pub(crate) struct FluidWidget<'a> {
    pub(crate) fluid: &'a RippleField,
}

impl Widget for FluidWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let w = area.width.max(1) as f32;
        let h = area.height.max(1) as f32;
        let base = self.fluid.hue;

        for y in 0..area.height {
            for x in 0..area.width {
                let nx = x as f32 / w;
                let ny = y as f32 / h;
                let v = self.fluid.field(nx, ny);

                // edge vignette
                let edge_x = (nx.min(1.0 - nx) * 2.0).min(1.0);
                let edge_y = (ny.min(1.0 - ny) * 2.0).min(1.0);
                let vig = (edge_x.min(edge_y) * 1.4).clamp(0.2, 1.0);

                let hue = base + (v - 0.5) * 45.0;
                let sat = (0.5 + v * 0.3).clamp(0.0, 1.0);
                let val = ((0.12 + v * 0.8) * vig).clamp(0.0, 1.0);

                let gi = ((v * (FLUID_GRADIENT.len() - 1) as f32).round() as usize)
                    .min(FLUID_GRADIENT.len() - 1);
                buf[(area.x + x, area.y + y)]
                    .set_char(FLUID_GRADIENT[gi])
                    .set_style(Style::default().fg(fluid_hsv(hue, sat, val)));
            }
        }
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
