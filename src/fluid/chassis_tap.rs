//! Opt-in Apple Silicon chassis-tap input and the platform-neutral tempo fit.
//!
//! The HID callback only recognizes a physical impulse and sends an `Instant`
//! to the UI thread. It never touches the live session or audio callback.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const MIN_INTERVAL: Duration = Duration::from_millis(250); // 240 BPM
const MAX_INTERVAL: Duration = Duration::from_secs(2); // 30 BPM
const HISTORY: usize = 5;

/// The live-only tap history. BPM becomes stable after three taps, using the
/// median of the recent intervals so one slightly early or late tap is benign.
#[derive(Default)]
pub(crate) struct TapTempo {
    taps: VecDeque<Instant>,
}

impl TapTempo {
    pub(crate) fn push(&mut self, tap: Instant) -> Option<(usize, f32)> {
        if let Some(previous) = self.taps.back() {
            let interval = tap.saturating_duration_since(*previous);
            if interval > MAX_INTERVAL {
                self.taps.clear();
            } else if interval < MIN_INTERVAL {
                return None;
            }
        }
        self.taps.push_back(tap);
        while self.taps.len() > HISTORY {
            self.taps.pop_front();
        }
        if self.taps.len() < 3 {
            return None;
        }
        let mut intervals = self
            .taps
            .iter()
            .zip(self.taps.iter().skip(1))
            .map(|(a, b)| b.saturating_duration_since(*a).as_secs_f32())
            .collect::<Vec<_>>();
        intervals.sort_by(f32::total_cmp);
        let middle = intervals.len() / 2;
        let seconds = if intervals.len().is_multiple_of(2) {
            (intervals[middle - 1] + intervals[middle]) * 0.5
        } else {
            intervals[middle]
        };
        Some((self.taps.len(), (60.0 / seconds).clamp(30.0, 240.0)))
    }
}

pub(crate) struct ChassisTapReceiver {
    receiver: Receiver<Instant>,
    #[cfg(target_os = "macos")]
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(target_os = "macos")]
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ChassisTapReceiver {
    pub(crate) fn start() -> Result<Self, ChassisTapError> {
        platform::start()
    }

    pub(crate) fn drain(&self, tempo: &mut TapTempo) -> Vec<(usize, f32)> {
        self.receiver
            .try_iter()
            .filter_map(|tap| tempo.push(tap))
            .collect()
    }
}

#[cfg(target_os = "macos")]
impl Drop for ChassisTapReceiver {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug)]
pub(crate) struct ChassisTapError(String);

impl std::fmt::Display for ChassisTapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ChassisTapError {}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub(super) fn start() -> Result<ChassisTapReceiver, ChassisTapError> {
        Err(ChassisTapError(
            "--chassis-tap requires macOS on an Apple Silicon MacBook".into(),
        ))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    // The generated IOKit bindings are intentionally isolated in this FFI-only module.
    #![allow(unsafe_op_in_unsafe_fn)]
    use super::*;
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use core_foundation_sys::base::{CFRelease, CFTypeRef, kCFAllocatorDefault};
    use core_foundation_sys::dictionary::{
        CFDictionaryCreate, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
    };
    use core_foundation_sys::number::{CFNumberCreate, kCFNumberSInt32Type};
    use core_foundation_sys::runloop::{
        CFRunLoopGetCurrent, CFRunLoopRunInMode, kCFRunLoopDefaultMode,
    };
    use core_foundation_sys::set::{CFSetGetCount, CFSetGetValues};
    use io_kit_sys::CFSTR;
    use io_kit_sys::hid::base::IOHIDDeviceRef;
    use io_kit_sys::hid::device::{
        IOHIDDeviceClose, IOHIDDeviceConformsTo, IOHIDDeviceOpen,
        IOHIDDeviceRegisterInputReportCallback, IOHIDDeviceScheduleWithRunLoop,
        IOHIDDeviceUnscheduleFromRunLoop,
    };
    use io_kit_sys::hid::manager::{
        IOHIDManagerClose, IOHIDManagerCopyDevices, IOHIDManagerCreate, IOHIDManagerOpen,
        IOHIDManagerSetDeviceMatching, kIOHIDManagerOptionNone,
    };
    use io_kit_sys::ret::{IOReturn, kIOReturnSuccess};
    use io_kit_sys::{
        IOIteratorNext, IOObjectRelease, IORegistryEntrySetCFProperty,
        IOServiceGetMatchingServices, IOServiceMatching, kIOMasterPortDefault,
    };

    const VENDOR_PAGE: u32 = 0xff00;
    const ACCELEROMETER_USAGE: u32 = 3;
    const REPORT_BYTES: usize = 22;
    const TAP_JERK: f64 = 3_500.0;
    const COOLDOWN: Duration = Duration::from_millis(120);

    struct CallbackContext {
        sender: Sender<Instant>,
        previous_axes: Option<[i32; 3]>,
        last_tap: Option<Instant>,
    }

    pub(super) fn start() -> Result<ChassisTapReceiver, ChassisTapError> {
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let ready_for_start = ready_sender.clone();
        let worker = std::thread::spawn(move || {
            if let Err(error) = unsafe { run_sensor(sender, worker_stop, ready_for_start) } {
                let _ = ready_sender.send(Err(error));
            }
        });
        ready_receiver.recv().map_err(|_| {
            ChassisTapError("chassis sensor worker stopped during startup".into())
        })??;
        Ok(ChassisTapReceiver {
            receiver,
            stop,
            worker: Some(worker),
        })
    }

    unsafe fn run_sensor(
        sender: Sender<Instant>,
        stop: Arc<AtomicBool>,
        ready: mpsc::SyncSender<Result<(), ChassisTapError>>,
    ) -> Result<(), ChassisTapError> {
        wake_spu_drivers();
        let manager = IOHIDManagerCreate(kCFAllocatorDefault, kIOHIDManagerOptionNone);
        if manager.is_null() {
            return Err(ChassisTapError(
                "could not create the Apple HID manager".into(),
            ));
        }
        let matching = usage_page_matching()?;
        IOHIDManagerSetDeviceMatching(manager, matching);
        CFRelease(matching as CFTypeRef);
        let manager_open = IOHIDManagerOpen(manager, kIOHIDManagerOptionNone);
        if manager_open != kIOReturnSuccess {
            CFRelease(manager as CFTypeRef);
            return Err(ChassisTapError(format!(
                "the chassis sensor denied ordinary-user access (IOKit {manager_open}); no system permission prompt is available"
            )));
        }
        let devices = IOHIDManagerCopyDevices(manager);
        let count = if !devices.is_null() {
            CFSetGetCount(devices)
        } else {
            0
        };
        if count == 0 {
            if !devices.is_null() {
                CFRelease(devices as CFTypeRef);
            }
            IOHIDManagerClose(manager, kIOHIDManagerOptionNone);
            CFRelease(manager as CFTypeRef);
            return Err(ChassisTapError(
                "no Apple Silicon chassis accelerometer was found".into(),
            ));
        }
        let mut raw_devices = vec![ptr::null(); count as usize];
        CFSetGetValues(devices, raw_devices.as_mut_ptr());
        CFRelease(devices as CFTypeRef);
        let accelerometers = raw_devices
            .into_iter()
            .map(|device| device as IOHIDDeviceRef)
            .filter(|&device| IOHIDDeviceConformsTo(device, VENDOR_PAGE, ACCELEROMETER_USAGE) != 0)
            .collect::<Vec<_>>();
        if accelerometers.is_empty() {
            IOHIDManagerClose(manager, kIOHIDManagerOptionNone);
            CFRelease(manager as CFTypeRef);
            return Err(ChassisTapError(
                "no Apple Silicon chassis accelerometer was found".into(),
            ));
        }
        let context = Box::into_raw(Box::new(CallbackContext {
            sender,
            previous_axes: None,
            last_tap: None,
        }));
        let run_loop = CFRunLoopGetCurrent();
        let mut opened = Vec::new();
        let mut buffers = Vec::new();
        for device in accelerometers {
            if IOHIDDeviceOpen(device, kIOHIDManagerOptionNone) != kIOReturnSuccess {
                continue;
            }
            let mut buffer = Box::new([0; REPORT_BYTES]);
            IOHIDDeviceRegisterInputReportCallback(
                device,
                buffer.as_mut_ptr(),
                REPORT_BYTES as isize,
                Some(report),
                context.cast(),
            );
            IOHIDDeviceScheduleWithRunLoop(device, run_loop, kCFRunLoopDefaultMode);
            buffers.push(buffer);
            opened.push(device);
        }
        if opened.is_empty() {
            drop(Box::from_raw(context));
            IOHIDManagerClose(manager, kIOHIDManagerOptionNone);
            CFRelease(manager as CFTypeRef);
            return Err(ChassisTapError(
                "the chassis accelerometer denied ordinary-user access; no system permission prompt is available"
                    .into(),
            ));
        }
        let _ = ready.send(Ok(()));
        while !stop.load(Ordering::Acquire) {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 1);
        }
        for device in &opened {
            IOHIDDeviceUnscheduleFromRunLoop(*device, run_loop, kCFRunLoopDefaultMode);
            IOHIDDeviceClose(*device, kIOHIDManagerOptionNone);
        }
        drop(buffers);
        IOHIDManagerClose(manager, kIOHIDManagerOptionNone);
        drop(Box::from_raw(context));
        CFRelease(manager as CFTypeRef);
        Ok(())
    }

    unsafe fn usage_page_matching()
    -> Result<core_foundation_sys::dictionary::CFDictionaryRef, ChassisTapError> {
        let page = VENDOR_PAGE as i32;
        let value = CFNumberCreate(
            kCFAllocatorDefault,
            kCFNumberSInt32Type,
            (&page as *const i32).cast(),
        );
        if value.is_null() {
            return Err(ChassisTapError(
                "could not create HID matching value".into(),
            ));
        }
        let key = CFSTR(c"PrimaryUsagePage".as_ptr());
        let keys = [key.cast::<c_void>()];
        let values = [value.cast::<c_void>()];
        let dictionary = CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        CFRelease(value as CFTypeRef);
        if dictionary.is_null() {
            Err(ChassisTapError(
                "could not create HID matching dictionary".into(),
            ))
        } else {
            Ok(dictionary)
        }
    }

    unsafe fn wake_spu_drivers() {
        let matching = IOServiceMatching(c"AppleSPUHIDDriver".as_ptr());
        if matching.is_null() {
            return;
        }
        let mut iterator = 0;
        if IOServiceGetMatchingServices(kIOMasterPortDefault, matching, &mut iterator)
            != kIOReturnSuccess
        {
            return;
        }
        loop {
            let service = IOIteratorNext(iterator);
            if service == 0 {
                break;
            }
            for (key, value) in [
                (c"SensorPropertyReportingState", 1i32),
                (c"SensorPropertyPowerState", 1),
                (c"ReportInterval", 1_000),
            ] {
                let number = CFNumberCreate(
                    kCFAllocatorDefault,
                    kCFNumberSInt32Type,
                    (&value as *const i32).cast(),
                );
                if !number.is_null() {
                    IORegistryEntrySetCFProperty(service, CFSTR(key.as_ptr()), number as CFTypeRef);
                    CFRelease(number as CFTypeRef);
                }
            }
            IOObjectRelease(service);
        }
        IOObjectRelease(iterator);
    }

    unsafe extern "C" fn report(
        context: *mut c_void,
        result: IOReturn,
        _: *mut c_void,
        _: u32,
        _: u32,
        report: *mut u8,
        length: isize,
    ) {
        if result != kIOReturnSuccess || length < REPORT_BYTES as isize || report.is_null() {
            return;
        }
        let bytes = std::slice::from_raw_parts(report, length as usize);
        let axes = [6, 10, 14].map(|offset| {
            i32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("length checked"),
            )
        });
        let state = &mut *context.cast::<CallbackContext>();
        let now = Instant::now();
        if let Some(previous) = state.previous_axes {
            let jerk = axes
                .iter()
                .zip(previous)
                .map(|(next, before)| {
                    let delta = f64::from(*next - before);
                    delta * delta
                })
                .sum::<f64>()
                .sqrt();
            if jerk >= TAP_JERK
                && state
                    .last_tap
                    .is_none_or(|last| now.duration_since(last) >= COOLDOWN)
            {
                state.last_tap = Some(now);
                let _ = state.sender.send(now);
            }
        }
        state.previous_axes = Some(axes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_recent_taps_rejects_bounce_and_follows_tempo() {
        let start = Instant::now();
        let mut tempo = TapTempo::default();
        assert_eq!(tempo.push(start), None);
        assert_eq!(tempo.push(start + Duration::from_millis(500)), None);
        assert_eq!(
            tempo.push(start + Duration::from_millis(1_000)),
            Some((3, 120.0))
        );
        assert_eq!(tempo.push(start + Duration::from_millis(1_050)), None);
        let (_, bpm) = tempo.push(start + Duration::from_millis(1_500)).unwrap();
        assert!((bpm - 120.0).abs() < 0.01);
    }

    #[test]
    fn a_long_pause_starts_a_fresh_tap_phrase() {
        let start = Instant::now();
        let mut tempo = TapTempo::default();
        tempo.push(start);
        tempo.push(start + Duration::from_millis(500));
        assert_eq!(tempo.push(start + Duration::from_secs(4)), None);
        assert_eq!(tempo.push(start + Duration::from_millis(4_500)), None);
        assert_eq!(tempo.push(start + Duration::from_secs(5)), Some((3, 120.0)));
    }
}
