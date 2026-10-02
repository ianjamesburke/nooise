#!/usr/bin/env swift

import Foundation
import IOKit
import IOKit.hid

let vendorPage = 0xff00
let accelerometerUsage = 3
let gyroscopeUsage = 9
let reportBytes = 22
let probeSeconds = 20.0
let tapJerkThreshold = 3_500.0 // Q16 units, roughly 0.05 g between samples.

func property<T>(_ device: IOHIDDevice, _ key: CFString, as _: T.Type) -> T? {
    IOHIDDeviceGetProperty(device, key) as? T
}

func iokitMessage(_ result: IOReturn) -> String {
    guard let message = mach_error_string(result) else {
        return "IOKit error \(result)"
    }
    return String(cString: message)
}

/// The SPU leaves its IMU asleep until a client requests reports. These are
/// runtime-only driver properties; the probe does not write NVRAM or install
/// anything. Each return code tells us whether this account has that access.
func wakeSPUDrivers() -> [IOReturn] {
    guard let matching = IOServiceMatching("AppleSPUHIDDriver") else { return [] }
    var iterator: io_iterator_t = 0
    guard IOServiceGetMatchingServices(kIOMainPortDefault, matching, &iterator) == kIOReturnSuccess else {
        return []
    }
    defer { IOObjectRelease(iterator) }

    var results: [IOReturn] = []
    while case let service = IOIteratorNext(iterator), service != 0 {
        for (key, value) in [
            ("SensorPropertyReportingState", 1),
            ("SensorPropertyPowerState", 1),
            ("ReportInterval", 1_000),
        ] {
            results.append(IORegistryEntrySetCFProperty(
                service,
                key as CFString,
                NSNumber(value: value)
            ))
        }
        IOObjectRelease(service)
    }
    return results
}

let matching: [[String: Any]] = [accelerometerUsage, gyroscopeUsage].map { usage in
    [
        kIOHIDPrimaryUsagePageKey as String: vendorPage,
        kIOHIDPrimaryUsageKey as String: usage,
        kIOHIDTransportKey as String: "SPU",
    ]
}

let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))

IOHIDManagerSetDeviceMatchingMultiple(manager, matching as CFArray)
let managerResult = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
guard managerResult == kIOReturnSuccess else {
    fputs("Could not enumerate HID devices: \(iokitMessage(managerResult)).\n", stderr)
    exit(1)
}
defer { IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone)) }

let devices = (IOHIDManagerCopyDevices(manager) as? Set<IOHIDDevice>) ?? []
guard !devices.isEmpty else {
    print("No Apple Silicon chassis IMU was found (usage page 0xff00, usages 3/9).")
    print("This Mac is unsupported by this experimental probe; no permission change will help.")
    exit(2)
}

print("Found \(devices.count) Apple SPU chassis sensor\(devices.count == 1 ? "" : "s").")
print("Running as uid \(getuid()). Testing an ordinary, non-sudo HID open...\n")

var opened: [(IOHIDDevice, String)] = []
for device in devices {
    let usage = property(device, kIOHIDPrimaryUsageKey as CFString, as: NSNumber.self)?.intValue
    let kind = usage == accelerometerUsage ? "accelerometer" : "gyroscope"
    let model = property(device, kIOHIDProductKey as CFString, as: String.self) ?? "Apple SPU HID device"
    let result = IOHIDDeviceOpen(device, IOOptionBits(kIOHIDOptionsTypeNone))

    if result == kIOReturnSuccess {
        print("  ✓ \(kind) (\(model)): open succeeded without sudo")
        opened.append((device, kind))
    } else {
        print("  ✗ \(kind) (\(model)): \(iokitMessage(result)) [\(result)]")
    }
}

guard !opened.isEmpty else {
    print("\nThe sensor exists, but this account cannot open it as an ordinary user.")
    print("Next test: sudo swift scripts/chassis_tap_probe.swift")
    print("The probe never changes system permissions; sudo is only used for that one process.")
    exit(3)
}

let accelerometers = opened.filter { $0.1 == "accelerometer" }
guard !accelerometers.isEmpty else {
    print("\nThe gyroscope opened, but the accelerometer did not. Tap feedback needs the accelerometer.")
    exit(4)
}

print("\nListening for \(Int(probeSeconds)) seconds. Tap the chassis or desk beside it.")
print("Each ● is one detected chassis impulse; keyboard typing should remain much quieter.")

let wakeResults = wakeSPUDrivers()
let wakeFailures = wakeResults.filter { $0 != kIOReturnSuccess }
if wakeFailures.isEmpty, !wakeResults.isEmpty {
    print("Sensor reporting enabled without sudo.\n")
} else if let failure = wakeFailures.first {
    print("Sensor reporting was denied: \(iokitMessage(failure)) [\(failure)].")
    print("Try once more with: sudo swift scripts/chassis_tap_probe.swift\n")
} else {
    print("Could not locate the SPU driver to request live reports.\n")
}

var buffers: [UnsafeMutablePointer<UInt8>] = []
var previousAxes: [Int32]?
var reportCount = 0
var lastTap = Date.distantPast
let callback: IOHIDReportCallback = { _, result, _, _, _, report, length in
    guard result == kIOReturnSuccess, length >= reportBytes else { return }
    let values = [6, 10, 14].map { offset -> Int32 in
        // The 32-bit axes begin at byte 6, which is not naturally aligned.
        // Assemble their little-endian representation byte-by-byte instead
        // of loading an Int32 through a potentially unaligned pointer.
        let raw = UInt32(report[offset])
            | UInt32(report[offset + 1]) << 8
            | UInt32(report[offset + 2]) << 16
            | UInt32(report[offset + 3]) << 24
        return Int32(bitPattern: raw)
    }
    reportCount += 1
    defer { previousAxes = values }
    guard let previousAxes else { return }
    let jerkSquared = zip(values, previousAxes).reduce(0.0) { sum, pair in
        let change = Double(Int64(pair.0) - Int64(pair.1))
        return sum + change * change
    }
    let jerk = sqrt(jerkSquared)
    let now = Date()
    guard jerk >= tapJerkThreshold, now.timeIntervalSince(lastTap) >= 0.12 else { return }
    lastTap = now
    print(String(format: "● tap impulse %.3f g", jerk / 65_536.0))
}

for (device, _) in accelerometers {
    let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: reportBytes)
    buffers.append(buffer)
    IOHIDDeviceRegisterInputReportCallback(device, buffer, reportBytes, callback, nil)
    IOHIDDeviceScheduleWithRunLoop(device, CFRunLoopGetCurrent(), CFRunLoopMode.defaultMode.rawValue)
}

RunLoop.current.run(until: Date().addingTimeInterval(probeSeconds))

for (device, _) in accelerometers {
    IOHIDDeviceUnscheduleFromRunLoop(device, CFRunLoopGetCurrent(), CFRunLoopMode.defaultMode.rawValue)
}
for buffer in buffers {
    buffer.deallocate()
}
for (device, _) in opened {
    IOHIDDeviceClose(device, IOOptionBits(kIOHIDOptionsTypeNone))
}

if reportCount == 0 {
    print("\nNo reports arrived. The device opened, but live reporting needs more access on this macOS build.")
    print("Try once more with: sudo swift scripts/chassis_tap_probe.swift")
} else {
    print("\nProbe finished after \(reportCount) accelerometer reports. This Mac can provide chassis-tap feedback.")
}
