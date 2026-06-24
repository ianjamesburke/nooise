use std::error::Error;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

pub(crate) trait StereoEngine: Send + 'static {
    fn next_stereo(&mut self) -> (f32, f32);
}

pub(crate) fn start_stream<E>(
    experiment_id: &str,
    engine_factory: impl FnOnce(f32) -> E,
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
        "running {experiment_id} at {} Hz on {}",
        sample_rate as u32,
        device.name()?
    );

    let stream = match sample_format {
        SampleFormat::F32 => {
            build_f32_stream(&device, &stream_config, engine_factory(sample_rate))?
        }
        SampleFormat::I16 => {
            build_i16_stream(&device, &stream_config, engine_factory(sample_rate))?
        }
        SampleFormat::U16 => {
            build_u16_stream(&device, &stream_config, engine_factory(sample_rate))?
        }
        other => return Err(format!("unsupported sample format: {other:?}").into()),
    };

    stream.play()?;
    Ok(stream)
}

pub(crate) fn run_engine<E>(
    experiment_id: &str,
    engine_factory: impl FnOnce(f32) -> E,
) -> Result<(), Box<dyn Error>>
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
        "running {experiment_id} at {} Hz on {}",
        sample_rate as u32,
        device.name()?
    );
    println!("press Ctrl+C to stop");

    let stream = match sample_format {
        SampleFormat::F32 => {
            build_f32_stream(&device, &stream_config, engine_factory(sample_rate))?
        }
        SampleFormat::I16 => {
            build_i16_stream(&device, &stream_config, engine_factory(sample_rate))?
        }
        SampleFormat::U16 => {
            build_u16_stream(&device, &stream_config, engine_factory(sample_rate))?
        }
        other => return Err(format!("unsupported sample format: {other:?}").into()),
    };

    stream.play()?;

    loop {
        thread::park_timeout(Duration::from_secs(60));
    }
}

fn build_f32_stream<E>(
    device: &cpal::Device,
    config: &StreamConfig,
    engine: E,
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
        audio_error,
        None,
    )?)
}

fn build_i16_stream<E>(
    device: &cpal::Device,
    config: &StreamConfig,
    engine: E,
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
        audio_error,
        None,
    )?)
}

fn build_u16_stream<E>(
    device: &cpal::Device,
    config: &StreamConfig,
    engine: E,
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
        audio_error,
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

fn audio_error(error: cpal::StreamError) {
    eprintln!("audio stream error: {error}");
}
