// dupcheck.swift -- host-side check for duplicate HID input across transports.
//
// PLAN item: "Explicitly verify no duplicate input with USB and BLE both
// connected." The firmware routes USB ahead of BLE and is unit-tested for it
// (routing.rs: usb_wins_without_duplicate_ble_input), but that has never been
// verified end-to-end against a real host with both transports attached.
//
// This taps the HID input reports of every pico-numpad device on the host
// (vendor 0x2e8a) via IOHIDManager, tags each report with its transport
// (USB vs Bluetooth Low Energy), and reports whether the same key set arrived on
// both transports within a short window.
//
// Reports are compared by key signature (which modifiers/keycodes are asserted)
// rather than raw bytes, so a framing difference between the USB and BLE reports
// -- a different report ID, a different length -- cannot mask a real duplicate.
//
// Requires macOS Input Monitoring permission for the process that runs this
// (System Settings > Privacy & Security > Input Monitoring). Without it the
// callbacks never fire, so a zero-event run reports INCONCLUSIVE rather than
// passing.
//
// Usage:
//   dupcheck [seconds]      capture from the connected device(s), default 30
//   dupcheck --selftest     verify the duplicate-detection logic on synthetic
//                         events (no device, no permission needed)
//
// Exit: 0 = no duplicates, 1 = duplicates found, 2 = nothing captured.

import Foundation
import IOKit.hid

let vendorFilter: UInt32 = 0x2E8A // Raspberry Pi / pico-numpad
let dupWindowMs = 150.0
let reportCapacity = 64

struct Event {
    let t: Double // ms since start
    let transport: String
    let bytes: [UInt8]
    let sig: String
}

func hex(_ bytes: [UInt8]) -> String {
    bytes.map { String(format: "%02x", $0) }.joined(separator: " ")
}

/// Transport-independent identity of a keyboard report: which modifiers and which
/// keycodes are asserted. The boot-keyboard layout is
/// [modifiers, reserved, keycode x6]; a 9-byte report carries an explicit report
/// ID in byte 0.
func keySignature(_ bytes: [UInt8]) -> String {
    var b = bytes
    if b.count == 9 { b.removeFirst() }
    guard b.count >= 3 else { return "raw:" + hex(bytes) }
    let keys = Array(b.dropFirst(2)).filter { $0 != 0 }.sorted()
    return String(format: "mod=%02x keys=%@",
                 b[0],
                 keys.map { String(format: "%02x", $0) }.joined(separator: ","))
}

func isRelease(_ event: Event) -> Bool {
    !event.sig.hasPrefix("raw:") && event.sig == "mod=00 keys="
}

enum Verdict {
    case pass
    case fail([(Event, Event)])
    case inconclusive

    var code: Int32 {
        switch self {
        case .pass: return 0
        case .fail: return 1
        case .inconclusive: return 2
        }
    }
}

/// The whole point of the tool, kept free of IOKit so it can be self-tested.
func evaluate(_ events: [Event]) -> Verdict {
    if events.isEmpty { return .inconclusive }
    let presses = events.filter { !isRelease($0) }
    var duplicates: [(Event, Event)] = []
    for a in presses {
        for b in presses where a.transport != b.transport {
            if abs(a.t - b.t) <= dupWindowMs && a.sig == b.sig {
                duplicates.append((a, b))
            }
        }
    }
    return duplicates.isEmpty ? .pass : .fail(duplicates)
}

func report(_ events: [Event], _ verdict: Verdict) {
    print("\n--- summary ---")
    let grouped = Dictionary(grouping: events, by: { $0.transport })
    for (transport, group) in grouped.sorted(by: { $0.key < $1.key }) {
        print("  \(transport): \(group.count) report(s)")
    }
    print("  non-release reports: \(events.filter { !isRelease($0) }.count)")

    switch verdict {
    case .inconclusive:
        print("""
              VERDICT: INCONCLUSIVE -- nothing was captured.
              Grant Input Monitoring to your terminal app (System Settings >
              Privacy & Security > Input Monitoring), quit and reopen the
              terminal, then run this again.
              """)
    case .pass:
        print("  VERDICT: PASS -- no key set was delivered on both transports.")
    case .fail(let duplicates):
        print("  cross-transport duplicates within \(Int(dupWindowMs)) ms: \(duplicates.count)")
        print("  VERDICT: FAIL -- the same key set arrived on both transports:")
        for (a, b) in duplicates.prefix(10) {
            print(String(format: "    %@ @%.1f ms  <->  %@ @%.1f ms   %@",
                        a.transport, a.t, b.transport, b.t, a.sig))
        }
    }
}

// ---------------------------------------------------------------- selftest --

/// Feed the detector synthetic events so the verdict logic is verified even where
/// nobody can press keys. Each case asserts the verdict evaluate() produces.
func selftest() -> Int32 {
    func ev(_ t: Double, _ transport: String, _ bytes: [UInt8]) -> Event {
        Event(t: t, transport: transport, bytes: bytes, sig: keySignature(bytes))
    }
    let keyA: [UInt8] = [0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0x00, 0x00] // '1'
    let release: [UInt8] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    // Same key, but the BLE report carries an explicit report ID of 0x01.
    let keyAWithReportID: [UInt8] = [0x01, 0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0x00, 0x00]
    let keyB: [UInt8] = [0x00, 0x00, 0x1f, 0x00, 0x00, 0x00, 0x00, 0x00] // '2'

    var failures = 0
    func check(_ name: String, _ events: [Event], expectFail: Bool) {
        let verdict = evaluate(events)
        let failed: Bool
        switch verdict {
        case .fail: failed = true
        case .pass, .inconclusive: failed = false
        }
        let ok = (failed == expectFail)
        print("  [\(ok ? "ok" : "FAIL")] \(name)")
        if !ok { failures += 1 }
    }

    check("same key on both transports inside the window -> FAIL",
          [ev(100, "USB", keyA), ev(120, "Bluetooth Low Energy", keyA)], expectFail: true)
    check("same key with a different report ID on the other transport -> FAIL",
          [ev(100, "USB", keyA), ev(120, "Bluetooth Low Energy", keyAWithReportID)], expectFail: true)
    check("same key on both transports outside the window -> PASS",
          [ev(100, "USB", keyA), ev(700, "Bluetooth Low Energy", keyA)], expectFail: false)
    check("different keys on the two transports -> PASS",
          [ev(100, "USB", keyA), ev(110, "Bluetooth Low Energy", keyB)], expectFail: false)
    check("all traffic on one transport -> PASS",
          [ev(100, "USB", keyA), ev(200, "USB", keyB)], expectFail: false)
    check("releases on both transports are not duplicates -> PASS",
          [ev(100, "USB", release), ev(105, "Bluetooth Low Energy", release)], expectFail: false)
    check("no events at all -> not a FAIL", [], expectFail: false)
    switch evaluate([]) {
    case .inconclusive: print("  [ok] empty capture reports INCONCLUSIVE, not PASS")
    default: print("  [FAIL] empty capture did not report INCONCLUSIVE"); failures += 1
    }
    switch evaluate([ev(100, "USB", keyA)]) {
    case .pass: print("  [ok] single-transport capture reports PASS")
    default: print("  [FAIL] single-transport capture did not report PASS"); failures += 1
    }

    print(failures == 0 ? "selftest: PASS" : "selftest: \(failures) FAILURE(S)")
    return failures == 0 ? 0 : 1
}

// ------------------------------------------------------------------- main --

if CommandLine.arguments.contains("--selftest") {
    exit(selftest())
}

let seconds = CommandLine.arguments.count > 1 ? (Double(CommandLine.arguments[1]) ?? 30) : 30

final class Recorder {
    private let lock = NSLock()
    private var storage: [Event] = []
    private let start = Date()

    func record(_ transport: String, _ bytes: [UInt8]) {
        lock.lock()
        let t = Date().timeIntervalSince(start) * 1000.0
        let event = Event(t: t, transport: transport, bytes: bytes, sig: keySignature(bytes))
        storage.append(event)
        lock.unlock()
        print(String(format: "[%7.1f ms] %-22@ %@", t, transport as NSString, hex(bytes)))
    }

    var all: [Event] {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }
}

final class DevInfo {
    let transport: String
    init(_ transport: String) { self.transport = transport }
}

let recorder = Recorder()
// Context objects and report buffers are handed to IOKit as raw pointers, so they
// must outlive the registration; keep strong references here.
var contexts: [DevInfo] = []
var reportBuffers: [UnsafeMutablePointer<UInt8>] = []
var registered: Set<ObjectIdentifier> = []

func transportName(_ device: IOHIDDevice) -> String {
    if let t = IOHIDDeviceGetProperty(device, kIOHIDTransportKey as CFString) as? String {
        return t
    }
    if let product = IOHIDDeviceGetProperty(device, kIOHIDProductKey as CFString) as? String {
        return "unknown(\(product))"
    }
    return "unknown"
}

let reportCallback: IOHIDReportCallback = { ctx, result, _, reportType, reportID, report, reportLength in
    guard result == kIOReturnSuccess,
          reportType == kIOHIDReportTypeInput,
          let ctx = ctx,
          reportLength > 0
    else { return }
    let info = Unmanaged<DevInfo>.fromOpaque(ctx).takeUnretainedValue()
    let bytes = Array(UnsafeBufferPointer(start: report, count: Int(reportLength)))
    recorder.record("\(info.transport) [id \(reportID)]", bytes)
}

func register(_ device: IOHIDDevice) {
    // Both the synchronous CopyDevices pass and the asynchronous matching callback
    // can see the same device; registering it twice would double every recorded
    // report, so key the registration on device identity.
    let id = ObjectIdentifier(device)
    guard !registered.contains(id) else { return }
    registered.insert(id)

    let info = DevInfo(transportName(device))
    contexts.append(info)
    let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: reportCapacity)
    reportBuffers.append(buffer)
    IOHIDDeviceRegisterInputReportCallback(
        device, buffer, reportCapacity, reportCallback,
        Unmanaged.passUnretained(info).toOpaque())
    print("watching: \(info.transport)")
}

let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
IOHIDManagerSetDeviceMatching(manager, [kIOHIDVendorIDKey: vendorFilter] as CFDictionary)
// Registered before open so devices that connect later are picked up too.
IOHIDManagerRegisterDeviceMatchingCallback(manager, { _, _, _, device in register(device) }, nil)

guard IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone)) == kIOReturnSuccess else {
    FileHandle.standardError.write(
        "error: could not open IOHIDManager (Input Monitoring permission?)\n".data(using: .utf8)!)
    exit(2)
}

// Register the already-attached devices synchronously rather than waiting for the
// matching callback to fire on the run loop.
if let devices = IOHIDManagerCopyDevices(manager) as? Set<IOHIDDevice> {
    for device in devices { register(device) }
}

guard !contexts.isEmpty else {
    FileHandle.standardError.write("""
        error: no vendor 0x\(String(vendorFilter, radix: 16)) devices found.
        Connect the numpad over USB and/or BLE first.

        """.data(using: .utf8)!)
    exit(2)
}

IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), CFRunLoopMode.defaultMode.rawValue)
print("Recording for \(seconds)s. Type each key once, then hold a few keys and release.")
CFRunLoopRunInMode(.defaultMode, seconds, false)

let events = recorder.all
report(events, evaluate(events))
exit(evaluate(events).code)
