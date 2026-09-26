// dupcheck.swift -- host-side check for duplicate HID input across transports.
// Taps every pico-numpad (vendor 0x2e8a) HID report via IOHIDManager, tags it
// USB or Bluetooth Low Energy, and fails if the same key signature arrives on
// both within a short window. Compared by signature, not raw bytes, so a framing
// difference cannot mask a real duplicate. Needs macOS Input Monitoring; with it
// missing the callbacks never fire, so a zero-event run is INCONCLUSIVE.
//
// Usage: `dupcheck [seconds]` (default 30) | `dupcheck --selftest`
// Exit: 0 = no duplicates, 1 = duplicates found, 2 = nothing captured.

import Foundation
import IOKit.hid

let vendorFilter: UInt32 = 0x2E8A // Raspberry Pi / pico-numpad
let dupWindowMs = 150.0
let reportCapacity = 64

struct Event {
    let t: Double // ms since start
    let transport: String // "USB", "Bluetooth Low Energy", ...
    let reportID: UInt32
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
    case inconclusive(String)

    var code: Int32 {
        switch self {
        case .pass: return 0
        case .fail: return 1
        case .inconclusive: return 2
        }
    }
}

/// The whole point of the tool, kept free of IOKit so it can be self-tested.
/// Pre: `events` is everything captured in the window.
/// Post: PASS only when both transports were observed; one-transport traffic is
/// INCONCLUSIVE, since it cannot have detected a cross-transport duplicate.
func evaluate(_ events: [Event]) -> Verdict {
    let transports = Set(events.map { $0.transport })
    if events.isEmpty {
        return .inconclusive("no reports were captured at all")
    }
    if transports.count < 2 {
        let seen = transports.first!
        // BLE silence is what USB priority looks like, and the host cannot tell
        // that apart from a dead link on its own.
        let usbHint = "this is what USB priority looks like, but it is not proof of no duplication: confirm the BLE link is up in the RTT log ('connected on host slot N'), then unplug USB and type -- reports should then appear on Bluetooth Low Energy, which proves this tool can see BLE at all"
        let bleHint = "confirm the USB link is connected and enumerated"
        let hint = seen == "USB" ? usbHint : bleHint
        return .inconclusive("only \(seen) was captured -- \(hint)")
    }
    let presses = events.filter { !isRelease($0) }
    if presses.isEmpty {
        return .inconclusive("both transports reported, but no key press was observed "
            + "(only empty/release reports) -- nothing was actually tested")
    }
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
    case .inconclusive(let reason):
        print("  VERDICT: INCONCLUSIVE -- \(reason).")
        print("  This is not a pass. Check that both transports are connected and reporting,")
        print("  and that Input Monitoring is granted to your terminal app (System Settings >")
        print("  Privacy & Security > Input Monitoring); after granting it, quit and reopen")
        print("  the terminal and run this again.")
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

/// Feed the detector synthetic events so the verdict logic is verified without
/// anyone pressing keys.
func selftest() -> Int32 {
    func ev(_ t: Double, _ transport: String, _ bytes: [UInt8], _ reportID: UInt32 = 0) -> Event {
        Event(t: t, transport: transport, reportID: reportID, bytes: bytes, sig: keySignature(bytes))
    }
    let keyA: [UInt8] = [0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0x00, 0x00] // '1'
    let release: [UInt8] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    // Same key, but the BLE report carries an explicit report ID of 0x01.
    let keyAWithReportID: [UInt8] = [0x01, 0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0x00, 0x00]
    let keyB: [UInt8] = [0x00, 0x00, 0x1f, 0x00, 0x00, 0x00, 0x00, 0x00] // '2'

    var failures = 0
    func check(_ name: String, _ events: [Event], expect: String) {
        let verdict: String
        switch evaluate(events) {
        case .pass: verdict = "pass"
        case .fail: verdict = "fail"
        case .inconclusive: verdict = "inconclusive"
        }
        let ok = (verdict == expect)
        print("  [\(ok ? "ok" : "FAIL")] \(name) (got \(verdict), want \(expect))")
        if !ok { failures += 1 }
    }

    check("same key on both transports inside the window", [ev(100, "USB", keyA), ev(120, "BLE", keyA)],
          expect: "fail")
    check("same key with a different report ID on the other transport",
          [ev(100, "USB", keyA, 1), ev(120, "BLE", keyAWithReportID, 2)], expect: "fail")
    check("same key on both transports outside the window",
          [ev(100, "USB", keyA), ev(700, "BLE", keyA)], expect: "pass")
    check("different keys on the two transports",
          [ev(100, "USB", keyA), ev(110, "BLE", keyB)], expect: "pass")
    check("releases on both transports are not duplicates",
          [ev(100, "USB", keyA), ev(105, "BLE", keyB), ev(200, "USB", release), ev(205, "BLE", release)],
          expect: "pass")

    check("traffic on USB only", [ev(100, "USB", keyA), ev(200, "USB", keyB)], expect: "inconclusive")
    check("traffic on BLE only", [ev(100, "BLE", keyA), ev(200, "BLE", keyB)], expect: "inconclusive")
    check("one report on one transport only", [ev(100, "USB", keyA)], expect: "inconclusive")
    check("both transports live but only releases observed",
          [ev(100, "USB", release), ev(105, "BLE", release)], expect: "inconclusive")
    check("no events at all", [], expect: "inconclusive")

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

    func record(_ transport: String, _ reportID: UInt32, _ bytes: [UInt8]) {
        lock.lock()
        let t = Date().timeIntervalSince(start) * 1000.0
        let event = Event(t: t, transport: transport, reportID: reportID,
                          bytes: bytes, sig: keySignature(bytes))
        storage.append(event)
        lock.unlock()
        print(String(format: "[%7.1f ms] %-22@ id=%u %@",
                    t, transport as NSString, reportID, hex(bytes)))
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
// Context objects and report buffers go to IOKit as raw pointers, so strong
// references are kept here to outlive the registration.
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
    recorder.record(info.transport, reportID, bytes)
}

func register(_ device: IOHIDDevice) {
    // The synchronous and asynchronous passes can both see the same device; a
    // double registration would double every recorded report.
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
