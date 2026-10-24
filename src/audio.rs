//! Audio backend plumbing: the dedicated cpal output thread, the stream it
//! rebuilds when output routing changes, and the sample callback that
//! pulls stereo frames from a [`StereoEngine`].

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, Stream, StreamConfig, SupportedBufferSize};

/// Frames per output callback. Left to the OS, CoreAudio hands over whatever
/// buffer the device was last set to (512 on a stock Mac, more if another app
/// raised it), and every played note waits that long. 256 frames is under 6 ms
/// at 44.1 kHz and well inside the engine's per-sample budget.
const LIVE_BUFFER_FRAMES: u32 = 256;

/// Device routing is live application configuration, never musical state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum OutputSelection {
    #[default]
    SystemDefault,
    Device(String),
}

impl fmt::Display for OutputSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SystemDefault => f.write_str("System Default"),
            Self::Device(name) => f.write_str(name),
        }
    }
}

/// Shared only by the coordinator and audio owner; callbacks never lock it.
#[derive(Clone)]
pub(crate) struct OutputRouting(Arc<Mutex<RoutingState>>);

struct RoutingState {
    requested: OutputSelection,
    revision: u64,
    devices: Result<Vec<String>, AudioStartError>,
    refresh_requested: bool,
    status: Option<Result<String, AudioStartError>>,
    notice: Option<Result<String, AudioStartError>>,
}

impl OutputRouting {
    pub(crate) fn new(selection: OutputSelection) -> Self {
        Self(Arc::new(Mutex::new(RoutingState {
            requested: selection,
            revision: 0,
            devices: Ok(Vec::new()),
            refresh_requested: false,
            status: None,
            notice: None,
        })))
    }

    pub(crate) fn update_devices(&self, devices: Result<Vec<String>, AudioStartError>) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).devices = devices;
    }

    fn take_refresh_request(&self) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut state.refresh_requested)
    }

    pub(crate) fn choices(&self) -> Result<Vec<OutputSelection>, AudioStartError> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.refresh_requested = true;
        let mut choices = vec![OutputSelection::SystemDefault];
        choices.extend(
            state
                .devices
                .clone()?
                .into_iter()
                .map(OutputSelection::Device),
        );
        Ok(choices)
    }

    pub(crate) fn request(&self, selection: OutputSelection) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.requested = selection;
        state.revision += 1;
        state.status = None;
        state.notice = None;
    }

    pub(crate) fn requested(&self) -> (OutputSelection, u64) {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (state.requested.clone(), state.revision)
    }

    fn report(&self, revision: u64, status: Result<String, AudioStartError>) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.revision == revision && state.status.as_ref() != Some(&status) {
            state.notice = Some(status.clone());
            state.status = Some(status);
        }
    }

    pub(crate) fn take_notice(&self) -> Option<Result<String, AudioStartError>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .notice
            .take()
    }
}

pub(crate) fn output_device_names() -> Result<Vec<String>, AudioStartError> {
    let mut names = cpal::default_host()
        .output_devices()
        .map_err(AudioStartError::backend)?
        .map(|device| device.name().map_err(AudioStartError::backend))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

fn unique_device_index(names: &[String], name: &str) -> Result<usize, AudioStartError> {
    let mut matches = names
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.as_str() == name);
    let index = matches
        .next()
        .ok_or_else(|| AudioStartError::MissingDevice(name.into()))?
        .0;
    if matches.next().is_some() {
        return Err(AudioStartError::AmbiguousDevice(name.into()));
    }
    Ok(index)
}

fn find_output_device(selection: &OutputSelection) -> Result<cpal::Device, AudioStartError> {
    let host = cpal::default_host();
    match selection {
        OutputSelection::SystemDefault => host
            .default_output_device()
            .ok_or(AudioStartError::NoOutputDevice),
        OutputSelection::Device(name) => {
            let devices = host
                .output_devices()
                .map_err(AudioStartError::backend)?
                .collect::<Vec<_>>();
            let names = devices
                .iter()
                .map(|d| d.name().map_err(AudioStartError::backend))
                .collect::<Result<Vec<_>, _>>()?;
            let index = unique_device_index(&names, name)?;
            devices
                .into_iter()
                .nth(index)
                .ok_or_else(|| AudioStartError::MissingDevice(name.clone()))
        }
    }
}

pub(crate) trait StereoEngine: Send + 'static {
    /// Called once per output buffer before its first frame, with how long
    /// the buffer waits between this callback and the speaker.
    fn begin_buffer(&mut self, _output_latency: Duration) {}
    fn next_stereo(&mut self) -> (f32, f32);
}

/// Why playback never started.
///
/// The audio thread reports its startup outcome back across a channel, so this
/// has to be `Send` and cloneable for routing notices; backend variants retain
/// the backend message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AudioStartError {
    /// The host offered no default output device.
    NoOutputDevice,
    /// The device's native sample format is one nooise cannot write.
    UnsupportedSampleFormat(SampleFormat),
    /// The backend refused to open, configure, or play the stream.
    Backend(String),
    MissingDevice(String),
    AmbiguousDevice(String),
    /// The audio thread died before reporting either outcome.
    ThreadExited,
}

impl AudioStartError {
    fn backend(error: impl Error + Send + Sync + 'static) -> Self {
        Self::Backend(error.to_string())
    }
}

impl fmt::Display for AudioStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOutputDevice => write!(f, "no default output audio device found"),
            Self::UnsupportedSampleFormat(format) => {
                write!(f, "unsupported sample format: {format:?}")
            }
            Self::Backend(error) => write!(f, "{error}"),
            Self::MissingDevice(name) => write!(
                f,
                "output device {name:?} is unavailable; use --audio-devices to list devices"
            ),
            Self::AmbiguousDevice(name) => write!(
                f,
                "multiple output devices are named {name:?}; rename them in system audio settings"
            ),
            Self::ThreadExited => {
                write!(f, "audio thread exited before starting the stream")
            }
        }
    }
}

impl Error for AudioStartError {}

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
    routing: OutputRouting,
    engine_factory: impl Fn(f32) -> E + Send + 'static,
) -> Result<AudioOutput, AudioStartError>
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
            .spawn(move || run_audio_thread(app_id, routing, engine_factory, stop, init_tx))
            .map_err(AudioStartError::backend)?
    };

    match init_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error),
        Err(_) => return Err(AudioStartError::ThreadExited),
    }

    Ok(AudioOutput {
        stop,
        watcher: Some(watcher),
    })
}

struct RunningStream<E> {
    stream: Stream,
    engine: Arc<Mutex<E>>,
    sample_rate: u32,
    device_name: String,
}

fn run_audio_thread<E: StereoEngine>(
    app_id: String,
    routing: OutputRouting,
    engine_factory: impl Fn(f32) -> E,
    stop: Arc<AtomicBool>,
    init_tx: mpsc::Sender<Result<(), AudioStartError>>,
) {
    let mut device_changes = match DeviceChanges::new() {
        Ok(changes) => changes,
        Err(error) => {
            let _ = init_tx.send(Err(error));
            return;
        }
    };
    routing.update_devices(output_device_names());
    let needs_rebuild = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicU64::new(0));
    let mut generation = 1;
    let (mut selection, mut revision) = routing.requested();
    let mut running = match open_stream(
        &selection,
        &engine_factory,
        None,
        &needs_rebuild,
        &active,
        generation,
    ) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = init_tx.send(Err(error));
            return;
        }
    };
    active.store(generation, Ordering::Release);
    println!(
        "running {app_id} at {} Hz on {}",
        running.sample_rate, running.device_name
    );
    routing.report(revision, Ok(format!("Output: {}", running.device_name)));
    let _ = init_tx.send(Ok(()));
    let mut retry = false;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(250));
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let changed = device_changes.changed();
        let refresh = routing.take_refresh_request();
        if changed || refresh {
            let devices = output_device_names();
            // Some backends stop callbacks on unplug without an error callback.
            // An explicit selection must reconnect to that device, never default.
            if let (OutputSelection::Device(name), Ok(names)) = (&selection, &devices)
                && unique_device_index(names, name).is_err()
            {
                retry = true;
            }
            routing.update_devices(devices);
        }
        let (requested, requested_revision) = routing.requested();
        let default_changed = (changed || refresh)
            && requested == OutputSelection::SystemDefault
            && default_device_name().as_deref() != Some(running.device_name.as_str());
        let errored = needs_rebuild.swap(false, Ordering::Relaxed);
        if requested == selection
            && requested_revision == revision
            && !default_changed
            && !errored
            && !retry
        {
            continue;
        }
        generation += 1;
        match open_stream(
            &requested,
            &engine_factory,
            Some(&running),
            &needs_rebuild,
            &active,
            generation,
        ) {
            Ok(next) => {
                // Serialize the handover with an in-flight old callback. The new
                // callback is silent until activated; the old one is silent after.
                let guard = running.engine.lock().unwrap_or_else(|e| e.into_inner());
                active.store(generation, Ordering::Release);
                drop(guard);
                let rate_changed = next.sample_rate != running.sample_rate;
                running = next;
                selection = requested;
                revision = requested_revision;
                retry = false;
                routing.report(
                    revision,
                    Ok(format!(
                        "Output: {}{}",
                        running.device_name,
                        if rate_changed {
                            " (sample rate changed; audio engine restarted)"
                        } else {
                            ""
                        }
                    )),
                );
            }
            Err(error) => {
                routing.report(requested_revision, Err(error));
                retry = true;
                // Keep the working stream and retry an unavailable selection.
                thread::sleep(Duration::from_millis(750));
            }
        }
    }
    drop(running.stream);
}

fn default_device_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|device| device.name().ok())
}

fn reusable_engine<E>(
    current: Option<(u32, &Arc<Mutex<E>>)>,
    sample_rate: u32,
    factory: &impl Fn(f32) -> E,
) -> Arc<Mutex<E>> {
    match current.filter(|(rate, _)| *rate == sample_rate) {
        Some((_, engine)) => Arc::clone(engine),
        None => Arc::new(Mutex::new(factory(sample_rate as f32))),
    }
}

fn open_stream<E: StereoEngine>(
    selection: &OutputSelection,
    engine_factory: &impl Fn(f32) -> E,
    current: Option<&RunningStream<E>>,
    needs_rebuild: &Arc<AtomicBool>,
    active: &Arc<AtomicU64>,
    generation: u64,
) -> Result<RunningStream<E>, AudioStartError> {
    let device = find_output_device(selection)?;
    let device_name = device.name().map_err(AudioStartError::backend)?;
    let supported = device
        .default_output_config()
        .map_err(AudioStartError::backend)?;
    let sample_format = supported.sample_format();
    let buffer_size = live_buffer_size(supported.buffer_size());
    let mut config: StreamConfig = supported.into();
    config.buffer_size = buffer_size;
    let sample_rate = config.sample_rate.0;
    let engine = reusable_engine(
        current.map(|current| (current.sample_rate, &current.engine)),
        sample_rate,
        engine_factory,
    );
    let callback = SharedEngine {
        engine: Arc::clone(&engine),
        active: Arc::clone(active),
        generation,
    };
    let stream = match sample_format {
        SampleFormat::F32 => {
            build_stream(&device, &config, callback, Arc::clone(needs_rebuild), |s| s)?
        }
        SampleFormat::I16 => build_stream(
            &device,
            &config,
            callback,
            Arc::clone(needs_rebuild),
            to_i16,
        )?,
        SampleFormat::U16 => build_stream(
            &device,
            &config,
            callback,
            Arc::clone(needs_rebuild),
            to_u16,
        )?,
        other => return Err(AudioStartError::UnsupportedSampleFormat(other)),
    };
    stream.play().map_err(AudioStartError::backend)?;
    Ok(RunningStream {
        stream,
        engine,
        sample_rate,
        device_name,
    })
}

struct SharedEngine<E> {
    engine: Arc<Mutex<E>>,
    active: Arc<AtomicU64>,
    generation: u64,
}

impl<E: StereoEngine> SharedEngine<E> {
    fn render<T: Copy>(
        &self,
        data: &mut [T],
        channels: usize,
        latency: Duration,
        convert: &impl Fn(f32) -> T,
    ) {
        // Never wait on another callback or the handover owner.
        let Ok(mut engine) = self.engine.try_lock() else {
            data.fill(convert(0.0));
            return;
        };
        if self.active.load(Ordering::Acquire) != self.generation {
            data.fill(convert(0.0));
            return;
        }
        engine.begin_buffer(latency);
        for frame in data.chunks_mut(channels) {
            let (left, right) = engine.next_stereo();
            write_frame(frame, convert(left), convert(right));
        }
    }
}

/// `LIVE_BUFFER_FRAMES` held inside the device's advertised range, so the
/// request can never be refused. A device that advertises no range is left
/// on its own default and says so in the startup line.
fn live_buffer_size(supported: &SupportedBufferSize) -> BufferSize {
    match *supported {
        SupportedBufferSize::Range { min, max } => {
            BufferSize::Fixed(LIVE_BUFFER_FRAMES.clamp(min, max))
        }
        SupportedBufferSize::Unknown => BufferSize::Default,
    }
}

fn build_stream<E, T, C>(
    device: &cpal::Device,
    config: &StreamConfig,
    callback: SharedEngine<E>,
    needs_rebuild: Arc<AtomicBool>,
    convert: C,
) -> Result<Stream, AudioStartError>
where
    E: StereoEngine,
    T: cpal::SizedSample + Send + 'static,
    C: Fn(f32) -> T + Send + 'static,
{
    let channels = config.channels as usize;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let timestamp = info.timestamp();
                callback.render(
                    data,
                    channels,
                    timestamp
                        .playback
                        .duration_since(&timestamp.callback)
                        .unwrap_or_default(),
                    &convert,
                );
            },
            move |error| audio_error(error, &needs_rebuild),
            None,
        )
        .map_err(AudioStartError::backend)
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

#[cfg(test)]
mod routing_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn picker_refresh_requests_are_coalesced_and_preserve_frozen_choices() {
        let routing = OutputRouting::new(OutputSelection::SystemDefault);
        routing.update_devices(Ok(vec!["First".into()]));
        assert!(!routing.take_refresh_request());
        let frozen = routing.choices().unwrap();
        routing.choices().unwrap();
        assert!(routing.take_refresh_request());
        assert!(!routing.take_refresh_request());
        routing.update_devices(Ok(vec!["Second".into()]));
        assert_eq!(
            frozen,
            vec![
                OutputSelection::SystemDefault,
                OutputSelection::Device("First".into())
            ]
        );
        assert_eq!(
            routing.choices().unwrap()[1],
            OutputSelection::Device("Second".into())
        );
    }

    #[test]
    fn explicit_names_never_fall_back_or_pick_an_ambiguous_device() {
        let names = vec!["Speakers".into(), "BlackHole 2ch".into(), "Speakers".into()];
        assert_eq!(unique_device_index(&names, "BlackHole 2ch"), Ok(1));
        assert_eq!(
            unique_device_index(&names, "Missing"),
            Err(AudioStartError::MissingDevice("Missing".into()))
        );
        assert_eq!(
            unique_device_index(&names, "Speakers"),
            Err(AudioStartError::AmbiguousDevice("Speakers".into()))
        );
    }

    struct CountingEngine(u32);
    impl StereoEngine for CountingEngine {
        fn next_stereo(&mut self) -> (f32, f32) {
            self.0 += 1;
            (self.0 as f32, -(self.0 as f32))
        }
    }

    #[test]
    fn stream_handover_preserves_engine_and_only_active_callback_advances_it() {
        let builds = Cell::new(0);
        let factory = |_| {
            builds.set(builds.get() + 1);
            CountingEngine(0)
        };
        let engine = reusable_engine(None, 48_000, &factory);
        let active = Arc::new(AtomicU64::new(1));
        let old = SharedEngine {
            engine: Arc::clone(&engine),
            active: Arc::clone(&active),
            generation: 1,
        };
        let replacement = reusable_engine(Some((48_000, &engine)), 48_000, &factory);
        assert!(Arc::ptr_eq(&engine, &replacement));
        let next = SharedEngine {
            engine: replacement,
            active: Arc::clone(&active),
            generation: 2,
        };
        let mut samples = [0.0; 4];
        old.render(&mut samples, 2, Duration::ZERO, &|s| s);
        assert_eq!(samples, [1.0, -1.0, 2.0, -2.0]);
        next.render(&mut samples, 2, Duration::ZERO, &|s| s);
        assert_eq!(samples, [0.0; 4]);
        active.store(2, Ordering::Release);
        old.render(&mut samples, 2, Duration::ZERO, &|s| s);
        assert_eq!(samples, [0.0; 4]);
        next.render(&mut samples, 2, Duration::ZERO, &|s| s);
        assert_eq!(samples, [3.0, -3.0, 4.0, -4.0]);
        assert_eq!(builds.get(), 1);
        let changed_rate = reusable_engine(Some((48_000, &engine)), 44_100, &factory);
        assert!(!Arc::ptr_eq(&engine, &changed_rate));
        assert_eq!(builds.get(), 2);
    }

    #[test]
    fn callback_never_waits_for_the_handover_lock() {
        let engine = Arc::new(Mutex::new(CountingEngine(0)));
        let callback = SharedEngine {
            engine: Arc::clone(&engine),
            active: Arc::new(AtomicU64::new(1)),
            generation: 1,
        };
        let guard = engine.lock().unwrap();
        let mut samples = [1.0; 4];
        callback.render(&mut samples, 2, Duration::ZERO, &|s| s);
        assert_eq!(samples, [0.0; 4]);
        assert_eq!(guard.0, 0);
    }

    #[test]
    fn catalog_snapshots_survive_refresh_and_repeated_requests_get_new_revisions() {
        let routing = OutputRouting::new(OutputSelection::SystemDefault);
        routing.update_devices(Ok(vec!["First".into()]));
        let snapshot = routing.choices().unwrap();
        routing.update_devices(Ok(vec!["Second".into()]));
        assert_eq!(
            snapshot,
            vec![
                OutputSelection::SystemDefault,
                OutputSelection::Device("First".into())
            ]
        );
        assert_eq!(
            routing.choices().unwrap()[1],
            OutputSelection::Device("Second".into())
        );
        routing.report(0, Err(AudioStartError::MissingDevice("First".into())));
        assert!(routing.take_notice().unwrap().is_err());
        routing.request(OutputSelection::SystemDefault);
        assert_eq!(routing.requested().1, 1);
        routing.request(OutputSelection::SystemDefault);
        assert_eq!(routing.requested().1, 2);
        routing.report(2, Ok("Output: Speakers".into()));
        assert_eq!(routing.take_notice(), Some(Ok("Output: Speakers".into())));
    }
}

/// Notification-only CoreAudio adapter. No callback queries or allocates.
#[cfg(target_os = "macos")]
mod device_notifications {
    use super::*;
    use std::ffi::c_void;

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }
    const SYSTEM: u32 = 1;
    const ADDRESSES: [Address; 2] = [
        Address {
            selector: u32::from_be_bytes(*b"dev#"),
            scope: u32::from_be_bytes(*b"glob"),
            element: 0,
        },
        Address {
            selector: u32::from_be_bytes(*b"dOut"),
            scope: u32::from_be_bytes(*b"glob"),
            element: 0,
        },
    ];
    static REVISION: AtomicU64 = AtomicU64::new(0);
    type Listener = unsafe extern "C" fn(u32, u32, *const Address, *mut c_void) -> i32;
    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectAddPropertyListener(
            object: u32,
            address: *const Address,
            listener: Listener,
            data: *mut c_void,
        ) -> i32;
        fn AudioObjectRemovePropertyListener(
            object: u32,
            address: *const Address,
            listener: Listener,
            data: *mut c_void,
        ) -> i32;
    }
    unsafe extern "C" fn notify(_: u32, _: u32, _: *const Address, _: *mut c_void) -> i32 {
        REVISION.fetch_add(1, Ordering::Relaxed);
        0
    }
    pub(super) struct DeviceChanges {
        registered: usize,
        revision: u64,
    }
    impl DeviceChanges {
        pub(super) fn new() -> Result<Self, AudioStartError> {
            let mut changes = Self {
                registered: 0,
                revision: REVISION.load(Ordering::Relaxed),
            };
            for address in &ADDRESSES {
                // Static callback and addresses; no client data is dereferenced.
                let status = unsafe {
                    AudioObjectAddPropertyListener(SYSTEM, address, notify, std::ptr::null_mut())
                };
                if status != 0 {
                    return Err(AudioStartError::Backend(format!(
                        "CoreAudio device notifications: {status}"
                    )));
                }
                changes.registered += 1;
            }
            Ok(changes)
        }
        pub(super) fn changed(&mut self) -> bool {
            let revision = REVISION.load(Ordering::Relaxed);
            let changed = revision != self.revision;
            self.revision = revision;
            changed
        }
    }
    impl Drop for DeviceChanges {
        fn drop(&mut self) {
            for address in &ADDRESSES[..self.registered] {
                // Same registration identity; late callbacks only touch a static atomic.
                unsafe {
                    AudioObjectRemovePropertyListener(
                        SYSTEM,
                        address,
                        notify,
                        std::ptr::null_mut(),
                    );
                }
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn notifications_invalidate_once_without_backend_queries() {
            let mut changes = DeviceChanges {
                registered: 0,
                revision: REVISION.load(Ordering::Relaxed),
            };
            assert!(!changes.changed());
            // The callback ignores arguments and touches only static storage.
            unsafe {
                notify(SYSTEM, 0, std::ptr::null(), std::ptr::null_mut());
            }
            assert!(changes.changed());
            assert!(!changes.changed());
        }
    }
}
#[cfg(target_os = "macos")]
use device_notifications::DeviceChanges;

/// Other backends refresh on picker requests and recover through stream errors.
#[cfg(not(target_os = "macos"))]
struct DeviceChanges;
#[cfg(not(target_os = "macos"))]
impl DeviceChanges {
    fn new() -> Result<Self, AudioStartError> {
        Ok(Self)
    }
    fn changed(&mut self) -> bool {
        false
    }
}
