//! Diagnostic: master level with each gesture held at full amount.
//! `cargo test --release gesture_level_probe -- --ignored --nocapture`

use super::*;

const RATE: f32 = 44_100.0;
const SECONDS: f32 = 14.0;
const SKIP_SECONDS: f32 = 3.0;
/// Master level is scaled down so the output clamp never engages; the
/// readout is scaled back up to the level the song asked for.
const PROBE_SCALE: f32 = 1.0 / 16.0;

fn render(song: &SongState, held: &[GestureKind]) -> (f32, f32) {
    let mut snapshot = LiveSessionSnapshot::from_song(song);
    snapshot.gestures = GestureState::default();
    for kind in held {
        snapshot.gestures.lanes[*kind as usize] = GestureEnvelope {
            amount: 1.0,
            at_seconds: 0.0,
            held: true,
            restored: false,
        };
    }
    snapshot.controls.master.level *= PROBE_SCALE;
    let mut engine = FluidEngine::new(
        RATE,
        LiveSession::new(snapshot),
        no_morph(),
        Arc::new(FluidTelemetry::default()),
    );
    engine.reseed(7);
    let (mut peak, mut energy, mut count) = (0.0_f32, 0.0_f64, 0_u64);
    for frame in 0..(SECONDS * RATE) as usize {
        let (l, r) = engine.next_stereo();
        let (l, r) = (l / PROBE_SCALE, r / PROBE_SCALE);
        if frame as f32 >= SKIP_SECONDS * RATE {
            peak = peak.max(l.abs()).max(r.abs());
            energy += f64::from(l * l + r * r);
            count += 2;
        }
    }
    (peak, (energy / count as f64).sqrt() as f32)
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

#[test]
#[ignore = "diagnostic render, prints a level table"]
fn gesture_level_probe() {
    use GestureKind::*;
    let cases: [(&str, &[GestureKind]); 6] = [
        ("Bloom", &[Bloom]),
        ("Submerge", &[Submerge]),
        ("Echo", &[Echo]),
        ("Thin", &[Thin]),
        ("Lift", &[Lift]),
        ("Bl+Ec+Su", &[Bloom, Echo, Submerge]),
    ];
    let mut worst = [f32::MIN; 6];
    for (index, song) in decode_auto_states().iter().enumerate() {
        let (base_peak, base_rms) = render(song, &[]);
        let mut line = format!("song {index:2} base {:+6.1} dBFS |", db(base_peak));
        for (case, (name, held)) in cases.iter().enumerate() {
            let (peak, rms) = render(song, held);
            let dp = db(peak) - db(base_peak);
            worst[case] = worst[case].max(dp);
            line += &format!(
                " {name} pk{dp:+5.1} rms{:+5.1}{}",
                db(rms) - db(base_rms),
                if peak > 0.95 { " CLAMP" } else { "" }
            );
        }
        println!("{line}");
    }
    println!("worst peak delta per case: {worst:?}");
}
