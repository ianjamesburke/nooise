use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

pub(crate) trait StereoEngine: Send + 'static {
    fn next_stereo(&mut self) -> (f32, f32);
}

const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Owns a dedicated audio thread that rebuilds the output stream whenever
/// the OS default output device changes (Bluetooth reconnect, headphone
/// unplug, AirPlay, ...) or the stream reports an error. cpal's `Stream`
/// isn't `Send` on every backend, so it lives and dies entirely on this one
/// thread; we never hand it to another thread. cpal also has no
/// cross-platform "default device changed" callback, so we poll.
pub(crate) struct AudioOutput {
    stop: Arc<AtomicBool>,
    watcher: Option<thread::JoinHandle<()>>,
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.watcher.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn start_stream<E>(
    app_id: &str,
    engine_factory: impl Fn(f32) -> E + Send + 'static,
) -> Result<AudioOutput, Box<dyn Error>>
where
    E: StereoEngine,
{
    let app_id = app_id.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let (init_tx, init_rx) = mpsc::channel();

    let watcher = {
        let stop = Arc::clone(&stop);
        thread::Builder::new()
            .name("nooise-audio".into())
            .spawn(move || run_audio_thread(app_id, engine_factory, stop, init_tx))?
    };

    match init_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(message)) => return Err(message.into()),
        Err(_) => return Err("audio thread exited before starting the stream".into()),
    }

    Ok(AudioOutput {
        stop,
        watcher: Some(watcher),
    })
}

/// `stream` is held only for RAII: dropping it stops playback. Reassigned
/// on rebuild, never read otherwise.
#[allow(unused_assignments)]
fn run_audio_thread<E>(
    app_id: String,
    engine_factory: impl Fn(f32) -> E,
    stop: Arc<AtomicBool>,
    init_tx: mpsc::Sender<Result<(), String>>,
) where
    E: StereoEngine,
{
    let needs_rebuild = Arc::new(AtomicBool::new(false));
    let mut _stream = match open_stream(&app_id, &engine_factory, Arc::clone(&needs_rebuild)) {
        Ok(stream) => {
            let _ = init_tx.send(Ok(()));
            stream
        }
        Err(err) => {
            let _ = init_tx.send(Err(err.to_string()));
            return;
        }
    };

    let mut current_device_name = default_device_name();
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(DEVICE_POLL_INTERVAL);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let device_changed = default_device_name() != current_device_name;
        let errored = needs_rebuild.swap(false, Ordering::Relaxed);
        if !device_changed && !errored {
            continue;
        }
        match open_stream(&app_id, &engine_factory, Arc::clone(&needs_rebuild)) {
            Ok(new_stream) => {
                _stream = new_stream;
                current_device_name = default_device_name();
            }
            Err(err) => eprintln!("failed to rebuild audio stream: {err}"),
        }
    }
}

fn default_device_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|device| device.name().ok())
}

fn open_stream<E>(
    app_id: &str,
    engine_factory: &impl Fn(f32) -> E,
    needs_rebuild: Arc<AtomicBool>,
) -> Result<Stream, Box<dyn Error>>
where
    E: StereoEngine,
{
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default output audio device found")?;
    let supported_config = device.default_output_config()?;
    let sample_format = supported_config.sample_format();
    let stream_config: StreamConfig = supported_config.into();
    let sample_rate = stream_config.sample_rate.0 as f32;

    println!(
        "running {app_id} at {} Hz on {}",
        sample_rate as u32,
        device.name()?
    );

    let engine = engine_factory(sample_rate);
    let stream = match sample_format {
        SampleFormat::F32 => build_f32_stream(&device, &stream_config, engine, needs_rebuild)?,
        SampleFormat::I16 => build_i16_stream(&device, &stream_config, engine, needs_rebuild)?,
        SampleFormat::U16 => build_u16_stream(&device, &stream_config, engine, needs_rebuild)?,
        other => return Err(format!("unsupported sample format: {other:?}").into()),
    };

    stream.play()?;
    Ok(stream)
}

fn build_f32_stream<E>(
    device: &cpal::Device,
    config: &StreamConfig,
    engine: E,
    needs_rebuild: Arc<AtomicBool>,
) -> Result<Stream, Box<dyn Error>>
where
    E: StereoEngine,
{
    let channels = config.channels as usize;
    let mut engine = engine;
    Ok(device.build_output_stream(
        config,
        move |data: &mut [f32], _| {
            for frame in data.chunks_mut(channels) {
                let (left, right) = engine.next_stereo();
                write_frame(frame, left, right);
            }
        },
        move |error| audio_error(error, &needs_rebuild),
        None,
    )?)
}

fn build_i16_stream<E>(
    device: &cpal::Device,
    config: &StreamConfig,
    engine: E,
    needs_rebuild: Arc<AtomicBool>,
) -> Result<Stream, Box<dyn Error>>
where
    E: StereoEngine,
{
    let channels = config.channels as usize;
    let mut engine = engine;
    Ok(device.build_output_stream(
        config,
        move |data: &mut [i16], _| {
            for frame in data.chunks_mut(channels) {
                let (left, right) = engine.next_stereo();
                write_frame(frame, to_i16(left), to_i16(right));
            }
        },
        move |error| audio_error(error, &needs_rebuild),
        None,
    )?)
}

fn build_u16_stream<E>(
    device: &cpal::Device,
    config: &StreamConfig,
    engine: E,
    needs_rebuild: Arc<AtomicBool>,
) -> Result<Stream, Box<dyn Error>>
where
    E: StereoEngine,
{
    let channels = config.channels as usize;
    let mut engine = engine;
    Ok(device.build_output_stream(
        config,
        move |data: &mut [u16], _| {
            for frame in data.chunks_mut(channels) {
                let (left, right) = engine.next_stereo();
                write_frame(frame, to_u16(left), to_u16(right));
            }
        },
        move |error| audio_error(error, &needs_rebuild),
        None,
    )?)
}

fn write_frame<T: Copy>(frame: &mut [T], left: T, right: T) {
    if let Some(sample) = frame.first_mut() {
        *sample = left;
    }
    if frame.len() > 1 {
        frame[1] = right;
    }
    for sample in frame.iter_mut().skip(2) {
        *sample = left;
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn to_u16(sample: f32) -> u16 {
    ((sample.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16
}

fn audio_error(error: cpal::StreamError, needs_rebuild: &Arc<AtomicBool>) {
    eprintln!("audio stream error: {error}");
    needs_rebuild.store(true, Ordering::Relaxed);
}
