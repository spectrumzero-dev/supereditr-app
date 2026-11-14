// Live UI smoke tests for a built SuperEditr.app, run by scripts/ui-smoke.sh on a Mac with a
// display and the Accessibility and Screen Recording permissions.
//
// SuperEditr is started as a child of this tool, so macOS attributes its Accessibility use to the
// process running the tests, which must hold Accessibility and Screen Recording. Nothing here
// changes permissions. SuperEditr gets a throwaway HOME, so its
// settings never touch the user's.
//
//   smoke APP OUT_DIR       exit 0 pass, 1 fail, 78 environment can't run UI tests (warn, don't fail)

import AppKit
import ApplicationServices
import CoreGraphics
import CryptoKit
import Foundation
import Network

// MARK: Reporting

var failures: [String] = []
var cleanups: [() -> Void] = []
func log(_ line: String) { print(line); fflush(stdout) }
func finish(_ code: Int32) -> Never {
    cleanups.reversed().forEach { $0() }
    exit(code)
}
func check(_ ok: Bool, _ name: String, _ detail: @autoclosure () -> String = "") {
    log("\(ok ? "PASS" : "FAIL") \(name)\(ok ? "" : ": \(detail())")")
    if !ok { failures.append(name) }
}
/// SuperEditr's last line about how it applied a fix in the app: in place, partly in place, or the
/// whole selection and why. "none" before the first.
var captureLogPath = ""
func appliedPath(_ bundleID: String) -> String {
    let lines = (try? String(contentsOfFile: captureLogPath, encoding: .utf8))?.split(separator: "\n") ?? []
    let path = lines.last { $0.contains("\(bundleID): appl") }.map(String.init) ?? "none"
    return path.split(separator: " ", maxSplits: 1).last.map(String.init) ?? path  // without the timestamp
}
func skipEnvironment(_ reason: String) -> Never {
    log("SKIP \(reason)")
    finish(78)
}

// MARK: Events

let replayMark: Int64 = 0x5345_4454  // SuperEditr's marker on keys it replays ("SEDT")
let keyE: CGKeyCode = 14, keyA: CGKeyCode = 0, keyEscape: CGKeyCode = 53, keyTab: CGKeyCode = 48, keySpace: CGKeyCode = 49
let keyShift: CGKeyCode = 56, keyCommand: CGKeyCode = 55, keyRight: CGKeyCode = 124, keyUp: CGKeyCode = 126, keyReturn: CGKeyCode = 36, keyV: CGKeyCode = 9, keyR: CGKeyCode = 15, keyDown: CGKeyCode = 125, keyC: CGKeyCode = 8

/// Holds Shift down (true) or lets it go (false), as a separate key, the way a person does.
func shift(_ down: Bool) {
    guard let event = CGEvent(keyboardEventSource: nil, virtualKey: keyShift, keyDown: down) else { return }
    event.flags = down ? .maskShift : []
    event.post(tap: .cghidEventTap)
    usleep(25_000)
}

/// ⌘A the way people often type it: ⌘ comes up before A does.
func selectAllReleasingCommandFirst() {
    let steps: [(CGKeyCode, Bool, CGEventFlags)] =
        [(keyCommand, true, .maskCommand), (keyA, true, .maskCommand), (keyCommand, false, []), (keyA, false, [])]
    for (code, down, flags) in steps {
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else { continue }
        event.flags = flags
        event.post(tap: .cghidEventTap)
        usleep(25_000)
    }
}

func press(_ code: CGKeyCode, _ flags: CGEventFlags = []) {
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else { continue }
        event.flags = flags
        event.post(tap: .cghidEventTap)
        usleep(25_000)
    }
}

/// Runs the main run loop, so the key logger below receives events.
func pump(_ seconds: Double) { RunLoop.current.run(until: Date().addingTimeInterval(seconds)) }

/// Records key downs at the end of the session event chain, after SuperEditr's tap has run.
final class KeyLog {
    static var seen: [(code: Int64, replayed: Bool)] = []
    static func start() -> Bool {
        let callback: CGEventTapCallBack = { _, type, event, _ in
            if type == .keyDown {
                KeyLog.seen.append((event.getIntegerValueField(.keyboardEventKeycode),
                                    event.getIntegerValueField(.eventSourceUserData) == replayMark))
            }
            return Unmanaged.passUnretained(event)
        }
        guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap, options: .defaultTap,
                                          eventsOfInterest: CGEventMask(1 << CGEventType.keyDown.rawValue),
                                          callback: callback, userInfo: nil) else { return false }
        CFRunLoopAddSource(CFRunLoopGetMain(), CFMachPortCreateRunLoopSource(nil, tap, 0), .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        return true
    }
    static func eKeys() -> [Bool] { seen.filter { $0.code == Int64(keyE) }.map(\.replayed) }
}

// MARK: Windows

struct Window { let frame: CGRect }

/// A process's open windows, read through Accessibility. The window server's list
/// (CGWindowListCopyWindowInfo) can come back empty for CI jobs, while Accessibility,
/// which this test needs anyway, sees every window the app has on screen.
func windows(bundleID: String) -> [Window] {
    guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first else { return [] }
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(app.processIdentifier), kAXWindowsAttribute as CFString, &value) == .success,
          let list = value as? [AXUIElement] else { return [] }
    return list.compactMap { element in
        var position: CFTypeRef?, size: CFTypeRef?
        var point = CGPoint.zero, extent = CGSize.zero
        guard AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &position) == .success,
              AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &size) == .success,
              AXValueGetValue(position as! AXValue, .cgPoint, &point),
              AXValueGetValue(size as! AXValue, .cgSize, &extent) else { return nil }
        return Window(frame: CGRect(origin: point, size: extent))
    }
}

let superEditr = "app.supereditr"
/// SuperEditr’s suggestion panel: about 548 points wide (Settings is 900, the guide card 424).
func overlay() -> Window? { windows(bundleID: superEditr).first { (400...700).contains($0.frame.width) } }
/// The permission guide card's window, about 424 points wide: a 368-point card with 28 points around
/// it for its shadow.
func guideCard() -> Window? { windows(bundleID: superEditr).first { $0.frame.width < 470 } }

/// What the window server's list shows of an app's windows. SuperEditr finds System Settings this
/// way to place the guide card, so an empty list here explains a missing card.
func windowServerView(_ bundleID: String) -> String {
    let pids = Set(running(bundleID).map { Int($0.processIdentifier) })
    let own = Set(running(superEditr).map { Int($0.processIdentifier) })
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
        return "no window list"
    }
    let owner = { (window: [String: Any]) in window[kCGWindowOwnerPID as String] as? Int ?? -1 }
    // Ordinary app windows, front to back, leaving out SuperEditr's own as it does.
    let normal = list.filter { $0[kCGWindowLayer as String] as? Int == 0 && !own.contains(owner($0)) }
    let ahead = normal.prefix { !pids.contains(owner($0)) }.compactMap { $0[kCGWindowOwnerName as String] as? String }
    return "\(list.count) on-screen windows, \(normal.filter { pids.contains(owner($0)) }.count) of them \(bundleID)'s, in front of it: \(ahead)"
}

/// SuperEditr's windows as Accessibility reports them, for diagnosing a missing overlay.
func describeWindows() -> String {
    let frames = windows(bundleID: superEditr).map { "\($0.frame)" }
    return frames.isEmpty ? "none" : frames.joined(separator: "; ")
}

func waitFor<T>(_ seconds: Double, _ probe: () -> T?) -> T? {
    let end = Date().addingTimeInterval(seconds)
    repeat {
        if let value = probe() { return value }
        pump(0.1)
    } while Date() < end
    return nil
}

func screenshot(window: Window?, rect: CGRect? = nil, to path: String) {
    var arguments = ["-x", "-o"]
    if let area = window?.frame ?? rect { arguments += ["-R", "\(Int(area.minX)),\(Int(area.minY)),\(Int(area.width)),\(Int(area.height))"] }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    process.arguments = arguments + [path]
    try? process.run()
    process.waitUntilExit()
    log(process.terminationStatus == 0 ? "     screenshot: \(path)" : "     screenshot failed: \(path)")
}

// MARK: Apps

var appLog = FileHandle.nullDevice
func launch(_ app: String, home: String, environment extra: [String: String] = [:]) -> Process {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "\(app)/Contents/MacOS/supereditr")
    var environment = ProcessInfo.processInfo.environment
    environment["HOME"] = home
    extra.forEach { environment[$0] = $1 }
    process.environment = environment
    process.standardOutput = appLog
    process.standardError = appLog
    try! process.run()
    return process
}

func stop(_ process: Process) {
    if process.isRunning { process.terminate() }
    process.waitUntilExit()
}

func running(_ bundleID: String) -> [NSRunningApplication] {
    NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
}

/// When the test began. An app instance launched after this is one the test started, the only kind
/// it ever quits. Matching on launch time, not on the instances listed just after `open` returns,
/// also catches an instance that wasn't listed yet while the one quit before it was still exiting.
let testStart = Date()
/// Bundle IDs of the apps this test opened, quit again when it finishes.
var opened: Set<String> = []

/// Starts an app that isn't already running (with a file, if given), waits until it is frontmost,
/// and remembers it so it can be quit later. `newInstance` starts a separate copy even when the app
/// is already open, passing it `launchArguments`.
func openFront(_ app: String, bundleID: String, file: String? = nil, newInstance: Bool = false, launchArguments: [String] = []) -> Bool {
    // Only a process that appears after this launch is the test's; one already running never is.
    let before = Set(running(bundleID).map(\.processIdentifier))
    let open = Process()
    open.executableURL = URL(fileURLWithPath: "/usr/bin/open")
    open.arguments = (newInstance ? ["-n"] : []) + ["-a", app] + (file.map { [$0] } ?? [])
        + (launchArguments.isEmpty ? [] : ["--args"] + launchArguments)
    try? open.run()
    open.waitUntilExit()
    opened.insert(bundleID)
    // Success means an instance this launch started is in front, never a copy that was already open.
    func ourFront() -> Bool? {
        guard let app = NSWorkspace.shared.frontmostApplication, app.bundleIdentifier == bundleID,
              !before.contains(app.processIdentifier) else { return nil }
        return true
    }
    // A first launch in a fresh VM can be slow to come forward; then ask the new instance itself.
    var front = waitFor(10, ourFront)
    if front == nil {
        running(bundleID).filter { !before.contains($0.processIdentifier) }.forEach { $0.activate() }
        front = waitFor(10, ourFront)
    }
    if front == nil {
        let app = NSWorkspace.shared.frontmostApplication
        log("     openFront: \(bundleID) not in front; front is \(app?.bundleIdentifier ?? "none") pid \(app?.processIdentifier ?? -1), " +
            "running before \(before.sorted()), now \(running(bundleID).map(\.processIdentifier).sorted())")
    }
    return front ?? false
}

/// The running instances of an app launched since the test began, oldest first.
func launchedSinceStart(_ bundleID: String) -> [NSRunningApplication] {
    running(bundleID).filter { ($0.launchDate ?? .distantPast) >= testStart }
        .sorted { ($0.launchDate ?? .distantPast) < ($1.launchDate ?? .distantPast) }
}

/// Quits the instances of an app launched since the test began and waits for them to exit;
/// anything else is left alone.
func quitOpened(_ bundleID: String) {
    let ours = launchedSinceStart(bundleID)
    ours.forEach { $0.forceTerminate() }
    _ = waitFor(5) { ours.allSatisfy(\.isTerminated) ? true : nil }
}

/// An app's main window through Accessibility, to move or minimise it as the user would.
func mainWindow(_ bundleID: String) -> AXUIElement? {
    guard let app = running(bundleID).first else { return nil }
    var window: CFTypeRef?
    guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(app.processIdentifier),
                                        kAXMainWindowAttribute as CFString, &window) == .success,
          let window else { return nil }
    return (window as! AXUIElement)
}

/// Posts one mouse event at `point` with no modifier keys.
func mouse(_ type: CGEventType, at point: CGPoint) {
    guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else { return }
    event.flags = []
    event.post(tap: .cghidEventTap)
}

/// A process's CPU time so far, user and system, in seconds; nil when it can't be read.
func cpuSeconds(_ pid: pid_t) -> Double? {
    var info = rusage_info_v2()
    let read = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V2, $0) }
    }
    guard read == 0 else { return nil }
    var timebase = mach_timebase_info_data_t()
    mach_timebase_info(&timebase)
    return Double(info.ri_user_time + info.ri_system_time) * Double(timebase.numer) / Double(timebase.denom) / 1e9
}

/// The window server's ID for an app's on-screen window whose frame is `frame`, or for its first
/// window `matching`.
func serverWindow(_ bundleID: String, _ matching: (CGRect) -> Bool) -> CGWindowID? {
    guard let app = running(bundleID).first,
          let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]
    else { return nil }
    return list.first { info in
        guard (info[kCGWindowOwnerPID as String] as? Int32) == app.processIdentifier,
              let bounds = info[kCGWindowBounds as String] as? NSDictionary,
              let rect = CGRect(dictionaryRepresentation: bounds) else { return false }
        return matching(rect)
    }?[kCGWindowNumber as String] as? CGWindowID
}

/// One window's frame now, from the window server: where it is on screen, even mid-drag, when an
/// app's own Accessibility frame may lag.
func serverFrame(_ id: CGWindowID) -> CGRect? {
    guard let info = (CGWindowListCopyWindowInfo(.optionIncludingWindow, id) as? [[String: Any]])?.first,
          let bounds = info[kCGWindowBounds as String] as? NSDictionary else { return nil }
    return CGRect(dictionaryRepresentation: bounds)
}

/// What the panel did during and after a title-bar drag.
struct DragFollow {
    /// The window's frame before and after the drag, and the panel's.
    let window: (CGRect, CGRect), panel: (CGRect, CGRect)?
    /// How far the panel strayed from where it was while the pointer moved, in points.
    let movedDuringDrag: Double
    /// Seconds from the mouse-up until the panel reached its final place; nil if it never moved.
    let settled: Double?
    /// How long the glide took, first step to last, in seconds.
    let glide: Double
    /// The glide's steps, the distance between each place the panel was seen at, in points, and
    /// for each, the gap in milliseconds between the sample that saw it and the sample before.
    let steps: [Double], gaps: [Double]
    /// Whether every step went the same way as the whole glide.
    let monotonic: Bool
    /// SuperEditr's CPU use while the pointer moved, as a share of one core.
    let cpu: Double?

    var meanStep: Double { steps.isEmpty ? 0 : steps.reduce(0, +) / Double(steps.count) }
    var maxStep: Double { steps.max() ?? 0 }
    var summary: String {
        String(format: "panel strayed %.1f pt during the drag; settled %@ after mouse-up; glide %.0f ms in %d steps, mean %.1f pt, largest %.1f pt (%.2f× the mean)%@; CPU while dragging %@ of a core",
               movedDuringDrag, settled.map { String(format: "%.0f ms", $0 * 1000) } ?? "never", glide * 1000, steps.count, meanStep, maxStep,
               meanStep > 0 ? maxStep / meanStep : 0, monotonic ? "" : ", not monotonic",
               cpu.map { String(format: "%.0f%%", $0 * 100) } ?? "unknown")
    }
}

/// Drags an app's main window by its title bar with real mouse events, `by` points in `frames`
/// steps of one 60 Hz frame each, as a user would. Watches the panel through the drag and for a
/// second after it, through the window server, which shows where a window is on screen.
func dragTitleBar(_ bundleID: String, by offset: CGVector, frames: Int = 45, superEditr pid: pid_t) -> DragFollow? {
    guard let window = mainWindow(bundleID), let panelStart = overlay()?.frame else { return nil }
    let start = frame(of: window)
    let near = { (a: CGRect, b: CGRect) in abs(a.minX - b.minX) <= 2 && abs(a.minY - b.minY) <= 2 && abs(a.width - b.width) <= 2 }
    let panelID = serverWindow(superEditr) { near($0, panelStart) }
    let panelNow = { panelID.flatMap(serverFrame) ?? overlay()?.frame }
    // Clear of the window buttons on the left and of a title in the middle, which a click renames.
    let grab = CGPoint(x: start.minX + 90, y: start.minY + 12)
    let cpuStart = cpuSeconds(pid), began = Date()
    mouse(.leftMouseDown, at: grab)
    pump(0.05)
    var strayed = 0.0
    for step in 1...frames {
        let share = CGFloat(step) / CGFloat(frames)
        mouse(.leftMouseDragged, at: CGPoint(x: grab.x + offset.dx * share, y: grab.y + offset.dy * share))
        pump(1.0 / 60)
        if let panel = panelNow() { strayed = max(strayed, hypot(panel.minX - panelStart.minX, panel.minY - panelStart.minY)) }
    }
    let elapsed = Date().timeIntervalSince(began)
    let cpu = cpuStart.flatMap { start in cpuSeconds(pid).map { ($0 - start) / elapsed } }
    mouse(.leftMouseUp, at: CGPoint(x: grab.x + offset.dx, y: grab.y + offset.dy))
    // Every place the panel is seen at for a second, sampled every 4 ms.
    let released = Date()
    // Each new place, with when it was seen and how long since the sample before.
    var seen: [(CGPoint, TimeInterval)] = [(panelStart.origin, 0)], gaps: [Double] = []
    var lastSample = 0.0
    while Date().timeIntervalSince(released) < 1.0 {
        let now = Date().timeIntervalSince(released)
        if let panel = panelNow(), panel.origin != seen.last!.0 {
            seen.append((panel.origin, now))
            gaps.append((now - lastSample) * 1000)
        }
        lastSample = now
        usleep(4_000)
    }
    pump(0.1)
    let end = frame(of: window)
    let panelEnd = panelNow()
    let total = CGVector(dx: seen.last!.0.x - panelStart.minX, dy: seen.last!.0.y - panelStart.minY)
    var steps: [Double] = [], monotonic = true
    for (previous, next) in zip(seen, seen.dropFirst()) {
        let dx = next.0.x - previous.0.x, dy = next.0.y - previous.0.y
        steps.append(hypot(dx, dy))
        if dx * total.dx + dy * total.dy < -0.5 { monotonic = false }
    }
    let expected = CGPoint(x: panelStart.minX + end.minX - start.minX, y: panelStart.minY + end.minY - start.minY)
    let settled = seen.first { abs($0.0.x - expected.x) < 2 && abs($0.0.y - expected.y) < 2 }?.1
    let glide = seen.count > 1 ? seen.last!.1 - seen[1].1 : 0
    return DragFollow(window: (start, end), panel: panelEnd.map { (panelStart, $0) }, movedDuringDrag: strayed,
                      settled: settled, glide: glide, steps: steps, gaps: gaps, monotonic: monotonic, cpu: cpu)
}

/// The ease-out SuperEditr's glide follows (`follow::ease_out`): CSS's `ease-out`, a cubic Bézier
/// through (0.58, 1), for the share `t` of the glide's time gone.
func easeOut(_ t: Double) -> Double {
    let t = min(max(t, 0), 1)
    let x = { (s: Double) in 3 * (1 - s) * s * s * 0.58 + s * s * s }
    let y = { (s: Double) in 3 * (1 - s) * s * s + s * s * s }
    var (low, high) = (0.0, 1.0)
    for _ in 0..<40 {
        let mid = (low + high) / 2
        if x(mid) < t { low = mid } else { high = mid }
    }
    return y((low + high) / 2)
}

/// SuperEditr's log of its last glide: its distance, and each step's frame time, the time the
/// step was made (both in milliseconds from the glide's start) and distance, in points.
func lastGlideLog() -> (distance: Double, steps: [(frame: Double, made: Double, pt: Double)])? {
    let lines = ((try? String(contentsOfFile: captureLogPath, encoding: .utf8)) ?? "").split(separator: "\n")
    guard let line = lines.last(where: { $0.contains("follow: glide done") }),
          let distanceRange = line.range(of: #", [0-9.]+ pt in"#, options: .regularExpression),
          let stepsStart = line.range(of: "steps: ") else { return nil }
    let distance = Double(line[distanceRange].dropFirst(2).dropLast(6)) ?? 0
    let steps = line[stepsStart.upperBound...].split(separator: ",").compactMap { item -> (Double, Double, Double)? in
        let parts = item.trimmingCharacters(in: .whitespaces).split(separator: "|").compactMap { Double($0) }
        return parts.count == 3 ? (parts[0], parts[1], parts[2]) : nil
    }
    return (distance, steps.map { (frame: $0.0, made: $0.1, pt: $0.2) })
}

/// A drag toward the middle of the screen, so the panel needn't be kept on screen and moves the
/// full distance.
func towardMiddle(_ bundleID: String) -> CGVector {
    guard let window = mainWindow(bundleID), let screen = NSScreen.screens.first?.frame else { return CGVector(dx: 120, dy: 50) }
    let start = frame(of: window)
    return CGVector(dx: start.midX > screen.midX ? -120 : 120, dy: start.midY > screen.height / 2 ? -50 : 50)
}

/// The focused element of the frontmost app, through Accessibility.
func focusedElement() -> AXUIElement? {
    guard let front = NSWorkspace.shared.frontmostApplication else { return nil }
    var focused: CFTypeRef?
    guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(front.processIdentifier),
                                        kAXFocusedUIElementAttribute as CFString, &focused) == .success,
          let focused else { return nil }
    return (focused as! AXUIElement)
}

/// Presses ⌘A until the frontmost app reports a selection (a freshly opened window may not take
/// keys straight away). Returns whether text ended up selected.
func selectAll() -> Bool {
    for _ in 0..<5 {
        press(keyA, .maskCommand)
        pump(0.4)
        var selected: CFTypeRef?
        if let element = focusedElement(),
           AXUIElementCopyAttributeValue(element, kAXSelectedTextAttribute as CFString, &selected) == .success,
           let text = selected as? String, !text.isEmpty {
            return true
        }
        pump(0.6)
    }
    return false
}

// MARK: Settings window

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

/// An element's own words: its title, description, and string value.
func ownText(_ element: AXUIElement) -> String {
    [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute].compactMap { attribute(element, $0) as? String }.joined(separator: " ")
}

/// Its words and its descendants' to `levels` deep, as a web view splits a label into child text.
func allText(_ element: AXUIElement, levels: Int = 6) -> String {
    let children = levels > 0 ? (attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? []) : []
    return ([ownText(element)] + children.map { allText($0, levels: levels - 1) }).joined(separator: " ")
}

/// Every word in SuperEditr's windows, as Accessibility reads them.
func panelText() -> String {
    guard let app = running(superEditr).first,
          let list = attribute(AXUIElementCreateApplication(app.processIdentifier), kAXWindowsAttribute) as? [AXUIElement] else { return "" }
    return list.map { allText($0, levels: 40) }.joined(separator: " ")
}

/// The first element, depth first, that `match` accepts.
func findElement(_ root: AXUIElement, depth: Int = 0, _ match: (AXUIElement) -> Bool) -> AXUIElement? {
    if match(root) { return root }
    guard depth < 40, let children = attribute(root, kAXChildrenAttribute) as? [AXUIElement] else { return nil }
    for child in children {
        if let found = findElement(child, depth: depth + 1, match) { return found }
    }
    return nil
}

/// SuperEditr's Settings window (about 900 points wide), as an Accessibility element.
func settingsWindow() -> AXUIElement? {
    guard let app = running(superEditr).first,
          let list = attribute(AXUIElementCreateApplication(app.processIdentifier), kAXWindowsAttribute) as? [AXUIElement] else { return nil }
    return list.first { element in
        var size = CGSize.zero
        guard let value = attribute(element, kAXSizeAttribute) else { return false }
        AXValueGetValue(value as! AXValue, .cgSize, &size)
        return size.width > 700
    }
}

/// The tree's roles and words, indented, to show what Accessibility exposed when a check fails.
func dumpTree(_ element: AXUIElement, depth: Int = 0) -> String {
    let role = attribute(element, kAXRoleAttribute) as? String ?? "?"
    let line = String(repeating: "  ", count: depth) + role + " " + ownText(element)
    guard depth < 40, let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] else { return line }
    return ([line] + children.map { dumpTree($0, depth: depth + 1) }).joined(separator: "\n")
}

func frame(of element: AXUIElement) -> CGRect {
    var point = CGPoint.zero, size = CGSize.zero
    if let p = attribute(element, kAXPositionAttribute), let s = attribute(element, kAXSizeAttribute) {
        AXValueGetValue(p as! AXValue, .cgPoint, &point)
        AXValueGetValue(s as! AXValue, .cgSize, &size)
    }
    return CGRect(origin: point, size: size)
}

/// Clicks the middle of `element` with the mouse, for an item Accessibility can't press, such as a
/// file in an open panel. With no modifier keys: an event otherwise takes on modifiers from earlier
/// key events, and a Shift- or Control-click selects differently or opens a menu.
func click(_ element: AXUIElement) {
    let box = frame(of: element)
    for type in [CGEventType.leftMouseDown, .leftMouseUp] {
        guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: CGPoint(x: box.midX, y: box.midY),
                                  mouseButton: .left) else { continue }
        event.flags = []
        event.post(tap: .cghidEventTap)
        usleep(50_000)
    }
}

func button(in root: AXUIElement, containing label: String) -> AXUIElement? {
    findElement(root) { (attribute($0, kAXRoleAttribute) as? String) == kAXButtonRole && allText($0).contains(label) }
}

/// The name Finder shows for the app at `path`, as Settings → Privacy lists it.
func appName(_ path: String) -> String {
    let name = FileManager.default.displayName(atPath: path)
    return name.hasSuffix(".app") ? String(name.dropLast(4)) : name
}

func focusedValueSettable() -> Bool? {
    guard let front = NSWorkspace.shared.frontmostApplication else { return nil }
    let app = AXUIElementCreateApplication(front.processIdentifier)
    var focused: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute as CFString, &focused) == .success,
          let focused else { return nil }
    var settable = DarwinBoolean(false)
    AXUIElementIsAttributeSettable(focused as! AXUIElement, kAXValueAttribute as CFString, &settable)
    return settable.boolValue
}

// MARK: Main

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    log("usage: smoke APP OUT_DIR")
    exit(2)
}
let appPath = arguments[1], outDir = arguments[2]
try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)

// Environment checks: a locked screen, missing permissions, or another SuperEditr make the results
// meaningless, so they skip rather than fail.
if let session = CGSessionCopyCurrentDictionary() as? [String: Any], session["CGSSessionScreenIsLocked"] as? Bool == true {
    skipEnvironment("the screen is locked")
}
guard AXIsProcessTrusted() else { skipEnvironment("this process lacks Accessibility permission") }
// Versions before 0.1.2 have the identifier com.sghael.supereditr.
if !running(superEditr).isEmpty || !running("com.sghael.supereditr").isEmpty { skipEnvironment("another SuperEditr is running") }
// The test opens and force-quits these; never touch someone's open documents.
for (name, id) in [("TextEdit", "com.apple.TextEdit"), ("Terminal", "com.apple.Terminal")] where !running(id).isEmpty {
    skipEnvironment("\(name) is already open on this Mac")
}
guard KeyLog.start() else { skipEnvironment("couldn't watch key events") }

let work = NSTemporaryDirectory() + "supereditr-smoke-\(getpid())"
let home = work + "/home"
try? FileManager.default.createDirectory(atPath: home, withIntermediateDirectories: true)
cleanups.append { try? FileManager.default.removeItem(atPath: work) }
captureLogPath = home + "/Library/Logs/SuperEditr/capture.log"
let support = home + "/Library/Application Support/" + superEditr
// The settings folder before 0.1.2, where the settings below start, so every run checks that an
// update carries them over.
let oldSupport = home + "/Library/Application Support/com.sghael.supereditr"
let status = support + "/status.json"
let draft = work + "/draft.txt"
try! "Their going to love this, it work good and its realy fast.".write(toFile: draft, atomically: true, encoding: .utf8)
// Onboarded, so Settings doesn't open over the test; Terminal allowed, so check 4 exercises the
// read-only text rule rather than the excluded-apps list; and the diagnostic log on, for the Chrome
// check and the run's artifacts.
try! FileManager.default.createDirectory(atPath: oldSupport, withIntermediateDirectories: true)
try! #"{"onboarded": true, "excluded_apps": ["com.1password.1password"], "diagnostic_log": true}"#.write(toFile: oldSupport + "/settings.json", atomically: true, encoding: .utf8)

FileManager.default.createFile(atPath: outDir + "/supereditr.log", contents: nil)
appLog = FileHandle(forWritingAtPath: outDir + "/supereditr.log") ?? FileHandle.nullDevice
var app = launch(appPath, home: home)
cleanups.append { stop(app) }
cleanups.append { opened.forEach(quitOpened) }
let pid = app.processIdentifier
let started = waitFor(15) { () -> Bool? in
    guard let data = FileManager.default.contents(atPath: status),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
    return json["accessibility"] as? Bool
}
guard let trusted = started else {
    let state = app.isRunning ? "still running" : "exited with status \(app.terminationStatus)"
    check(false, "SuperEditr starts", "no status.json within 15 s; the app \(state)")
    finish(1)
}
let carried = FileManager.default.contents(atPath: support + "/settings.json").flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
check(carried?["onboarded"] as? Bool == true && FileManager.default.fileExists(atPath: oldSupport + "/settings.json"),
      "settings from before 0.1.2 carry over, and the old folder stays",
      "settings.json in \(support): \(String(describing: carried))")
if let data = FileManager.default.contents(atPath: status),
   let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
    log("     SuperEditr pid: launched \(pid), reported \(json["pid"] ?? "?")")
}
guard trusted else { skipEnvironment("SuperEditr isn't trusted for Accessibility when run from here") }
pump(1.5)  // shortcut registration and the keyboard tap start just after launch

// 1. Selecting text in an editable document shows a suggestion on its own.
check(openFront("TextEdit", bundleID: "com.apple.TextEdit", file: draft), "TextEdit opens the draft")
check(selectAll(), "the draft's text is selected")
let automatic = waitFor(6) { overlay() }
check(automatic != nil, "automatic suggestion for an editable selection", "no overlay within 6 s. SuperEditr windows: \(describeWindows())")
if automatic == nil { screenshot(window: nil, rect: NSScreen.screens.first?.frame, to: outDir + "/no-automatic-suggestion.png") }
if let automatic {
    screenshot(window: automatic, to: outDir + "/automatic.png")
    // The panel opens at the selection: inside the TextEdit window, not elsewhere on screen. The
    // panel's window extends 26 points past its card on each side for the shadow
    // (PAD_SIDE in src-tauri/src/overlay.rs), so a selection near the window's left edge puts the
    // window, though not the card, outside TextEdit's.
    if let editor = windows(bundleID: "com.apple.TextEdit").first?.frame {
        let cardX = automatic.frame.minX + 26
        let near = cardX >= editor.minX - 1 && cardX < editor.maxX
            && automatic.frame.minY >= editor.minY && automatic.frame.minY < editor.maxY
        check(near, "the panel opens at the selection", "panel \(automatic.frame), TextEdit window \(editor)")
    }
}
// Esc closes a suggestion that opened by itself and goes no further: the text may have been
// selected for something else, so TextEdit never sees the key and the selection stays.
let selectedText = { focusedElement().flatMap { attribute($0, kAXSelectedTextAttribute) as? String } ?? "" }
let selectedBefore = selectedText()
KeyLog.seen.removeAll()
press(keyEscape)
check(waitFor(3) { overlay() == nil ? true : nil } != nil, "Escape closes an automatic suggestion", "overlay still showing")
check(!KeyLog.seen.contains { $0.code == Int64(keyEscape) }, "that Escape never reaches TextEdit", "Escape reached TextEdit")
pump(0.5)
check(!selectedBefore.isEmpty && selectedText() == selectedBefore, "the text is still selected after Escape",
      "selected before: \(selectedBefore.count) characters, after: \(selectedText().count)")
// Any other key belongs to the app, and typing there closes the suggestion.
check(selectAll(), "the draft's text is selected again")
check(waitFor(6) { overlay() } != nil, "selecting the text again brings the suggestion back", "no overlay within 6 s")
KeyLog.seen.removeAll()
press(keyRight)
check(waitFor(3) { overlay() == nil ? true : nil } != nil, "typing elsewhere dismisses it", "overlay still showing")
check(KeyLog.seen.contains { $0.code == Int64(keyRight) }, "that key reaches TextEdit", "the arrow key never reached TextEdit")

// 1b. Extending a selection with Shift held shows nothing until Shift comes up, then one panel.
press(keyUp, .maskCommand)  // to the start of the text, nothing selected
pump(0.5)
shift(true)
var shownWhileHeld = false
for _ in 0..<5 {
    press(keyRight, [.maskShift, .maskAlternate])  // a word at a time
    pump(0.3)
    shownWhileHeld = shownWhileHeld || overlay() != nil
}
check(!shownWhileHeld, "no suggestion while Shift is still held", "a panel opened between steps")
shift(false)
let settled = waitFor(5) { overlay() }
check(settled != nil, "one suggestion once Shift comes up", "no overlay within 5 s")
press(keyEscape)
check(waitFor(3) { overlay() == nil ? true : nil } != nil, "Escape dismisses it", "overlay still showing")

// 1c. ⌘A counts as a selection even when ⌘ comes up before A.
press(keyUp, .maskCommand)  // nothing selected
pump(0.5)
selectAllReleasingCommandFirst()
let commandFirst = waitFor(5) { overlay() }
check(commandFirst != nil, "a suggestion after ⌘A with ⌘ let go first", "no overlay within 5 s")
press(keyEscape)
check(waitFor(3) { overlay() == nil ? true : nil } != nil, "Escape dismisses that one too", "overlay still showing")

// 1d. The menu-bar menu turns SuperEditr off in the app in front, and back on. The menu opens over
// TextEdit without bringing SuperEditr forward, so its item names TextEdit. Turned off, TextEdit's
// selections get no suggestion and ⌘E E says how to turn it back on.
let settingsFile = support + "/settings.json"
func excludedApps() -> [String] {
    guard let data = FileManager.default.contents(atPath: settingsFile),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return [] }
    return json["excluded_apps"] as? [String] ?? []
}
/// Clicks SuperEditr's menu-bar icon and chooses the item whose title starts with `prefix`. Returns
/// the item's title, or nil, with the menu closed again, when the icon or the item isn't there.
func chooseFromMenu(_ prefix: String, screenshotTo path: String? = nil) -> String? {
    guard let app = running(superEditr).first,
          let bar = attribute(AXUIElementCreateApplication(app.processIdentifier), "AXExtrasMenuBar"),
          let icon = (attribute(bar as! AXUIElement, kAXChildrenAttribute) as? [AXUIElement])?.first else {
        log("     SuperEditr has no menu-bar icon in Accessibility")
        return nil
    }
    // The icon attaches its menu only while handling a real click, so pressing it through
    // Accessibility opens nothing. The pointer goes back where it was afterwards.
    let pointer = CGEvent(source: nil)?.location
    let spot = frame(of: icon)
    for type in [CGEventType.leftMouseDown, .leftMouseUp] {
        CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: CGPoint(x: spot.midX, y: spot.midY), mouseButton: .left)?
            .post(tap: .cghidEventTap)
        usleep(50_000)
    }
    defer {
        if let pointer {
            CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: pointer, mouseButton: .left)?.post(tap: .cghidEventTap)
        }
    }
    let item = waitFor(3) {
        findElement(icon) { (attribute($0, kAXRoleAttribute) as? String) == kAXMenuItemRole
            && (attribute($0, kAXTitleAttribute) as? String ?? "").hasPrefix(prefix) }
    }
    guard let item else {
        log("     menu-bar icon at \(spot):\n\(dumpTree(icon))")
        press(keyEscape)
        return nil
    }
    if let path, let menu = attribute(item, kAXParentAttribute) {
        screenshot(window: Window(frame: frame(of: menu as! AXUIElement)), to: path)
    }
    let title = attribute(item, kAXTitleAttribute) as? String
    AXUIElementPerformAction(item, kAXPressAction as CFString)
    return title
}
let turnOff = chooseFromMenu("Turn ", screenshotTo: outDir + "/menu-turn-off.png")
check(turnOff == "Turn off in TextEdit", "the menu-bar menu offers to turn SuperEditr off in TextEdit", "item: \(turnOff ?? "none")")
let turnedOff = waitFor(5) { excludedApps().contains("com.apple.TextEdit") ? true : nil }
check(turnedOff != nil, "choosing it adds TextEdit to the excluded apps", "excluded_apps: \(excludedApps())")
press(keyUp, .maskCommand)  // nothing selected
pump(0.5)
check(selectAll(), "TextEdit's text is selected while SuperEditr is off there")
pump(3)
check(overlay() == nil, "no automatic suggestion in an app it's turned off in", "overlay appeared")
press(keyE, .maskCommand)
pump(0.1)
press(keyE, .maskCommand)
let offNotice = waitFor(5) { () -> String? in
    let text = panelText()
    return text.contains("turned off in TextEdit") ? text : nil
}
check(offNotice?.contains("menu bar") ?? false, "⌘E E says it's off in TextEdit and how to turn it on", "panel text: \(offNotice ?? panelText())")
press(keyEscape)
check(waitFor(3) { overlay() == nil ? true : nil } != nil, "Escape closes the message", "overlay still showing")
let turnOn = chooseFromMenu("Turn ")
check(turnOn == "Turn on in TextEdit", "the menu then offers to turn it back on", "item: \(turnOn ?? "none")")
let turnedOn = waitFor(5) { excludedApps().contains("com.apple.TextEdit") ? nil : true }
check(turnedOn != nil, "choosing that takes TextEdit off the excluded apps", "excluded_apps: \(excludedApps())")
press(keyUp, .maskCommand)
pump(0.5)
check(selectAll(), "TextEdit's text is selected with SuperEditr on again")
check(waitFor(6) { overlay() } != nil, "a selection gets a suggestion again", "no overlay within 6 s")
// A click in the reviewed window's text, which doesn't move the window, still closes the panel.
if let editor = mainWindow("com.apple.TextEdit") {
    let box = frame(of: editor)
    mouse(.leftMouseDown, at: CGPoint(x: box.midX, y: box.midY))
    usleep(50_000)
    mouse(.leftMouseUp, at: CGPoint(x: box.midX, y: box.midY))
}
check(waitFor(3) { overlay() == nil ? true : nil } != nil, "a click in the window's text closes that suggestion", "overlay still showing")
if overlay() != nil { press(keyEscape) }
// The click moved the caret; select the text again for the checks below, and dismiss the
// suggestion that brings.
check(selectAll(), "TextEdit's text is selected again")
if waitFor(6, { overlay() }) != nil { press(keyEscape) }
_ = waitFor(3) { overlay() == nil ? true : nil }

// 2. A single ⌘E still reaches the app, replayed after the double-press window.
pump(0.5)
KeyLog.seen.removeAll()
press(keyE, .maskCommand)
pump(1.0)
check(KeyLog.eKeys() == [true], "a single ⌘E reaches the app", "E key downs seen (replayed?): \(KeyLog.eKeys())")
check(overlay() == nil, "a single ⌘E doesn't open a review", "overlay showing")

// 3. ⌘E E opens the review and neither press reaches the app.
KeyLog.seen.removeAll()
press(keyE, .maskCommand)
pump(0.1)
press(keyE, .maskCommand)
let review = waitFor(5) { overlay() }
check(review != nil, "⌘E E opens a review", "no overlay within 5 s. SuperEditr windows: \(describeWindows())")
if review == nil { screenshot(window: nil, rect: NSScreen.screens.first?.frame, to: outDir + "/no-review.png") }
check(KeyLog.eKeys().isEmpty, "⌘E E reaches nothing", "E key downs seen: \(KeyLog.eKeys())")
if review != nil {
    pump(2.5)  // let the model's revision stream in for the screenshot
    screenshot(window: overlay(), to: outDir + "/review.png")
}

// 3b. Moving the reviewed window moves the panel with it, by as much. The window moves toward the
// middle of the screen, so the panel needn't be kept on screen and moves the full distance.
if let before = overlay(), let editor = mainWindow("com.apple.TextEdit"), let screen = NSScreen.screens.first?.frame {
    let start = frame(of: editor)
    let dx: CGFloat = start.midX > screen.midX ? -60 : 60
    let dy: CGFloat = start.midY > screen.height / 2 ? -40 : 40
    var to = CGPoint(x: start.minX + dx, y: start.minY + dy)
    let position = AXValueCreate(.cgPoint, &to)!
    let set = AXUIElementSetAttributeValue(editor, kAXPositionAttribute as CFString, position)
    let moved = frame(of: editor)
    let followed = waitFor(3) { () -> Window? in
        guard let now = overlay(), abs(now.frame.minX - before.frame.minX - (moved.minX - start.minX)) < 2,
              abs(now.frame.minY - before.frame.minY - (moved.minY - start.minY)) < 2 else { return nil }
        return now
    }
    check(set == .success && moved.origin != start.origin, "the test moves TextEdit's window",
          "set: \(set.rawValue), window \(start) then \(moved)")
    check(followed != nil, "the panel moves with its window",
          "window \(start) then \(moved); panel \(before.frame) then \(overlay().map { "\($0.frame)" } ?? "closed")")
    screenshot(window: overlay(), to: outDir + "/review-moved.png")
}

// 3c. Dragging the reviewed window by its title bar keeps the panel open. The panel stays where it
// is while the window moves, then glides to the same place by the window once the drag ends,
// smoothly: every step the same way, none much larger than the rest. The review's keys still go to
// the panel afterwards: Escape closes it and never reaches TextEdit.
if overlay() != nil {
    let offset = towardMiddle("com.apple.TextEdit")
    if let drag = dragTitleBar("com.apple.TextEdit", by: offset, superEditr: pid) {
        let (start, end) = drag.window
        check(end.origin != start.origin, "the title-bar drag moves TextEdit's window", "window \(start) then \(end)")
        check(drag.panel != nil, "the panel stays open through a title-bar drag", "panel closed")
        check(drag.movedDuringDrag < 1, "the panel stays still while the window moves", drag.summary)
        if let (before, after) = drag.panel {
            let followed = abs(after.minX - before.minX - (end.minX - start.minX)) < 2
                && abs(after.minY - before.minY - (end.minY - start.minY)) < 2
            check(followed, "the panel ends by the drag's offset", "window \(start) then \(end); panel \(before) then \(after)")
        }
        check((drag.settled ?? 9) <= 0.4, "it gets there within 400 ms of the mouse-up", drag.summary)
        log("NOTE title-bar drag: \(drag.summary)")
        // Smoothness, judged from both ends. SuperEditr's log says when each step was made and for
        // which frame: every step must come within 1.5 frames of the one before and sit on the
        // curve. On screen, an ease-out's first step is about 1.6× the mean at 60 Hz, and a dropped
        // frame shows as two steps in one, so a step over 2× the mean fails, unless this test's own
        // samples around it were more than 1.5 frames apart: then it was the test that missed one.
        var problems: [String] = []
        if let glide = lastGlideLog(), !glide.steps.isEmpty {
            let frameTimes = zip(glide.steps.dropFirst(), glide.steps).map { $0.frame - $1.frame }.sorted()
            let frame = frameTimes.isEmpty ? 1000.0 / 60 : frameTimes[frameTimes.count / 2]
            let made = glide.steps.map(\.made)
            let intervals = zip(made.dropFirst(), made).map { $0 - $1 }
            log("NOTE glide frame \(String(format: "%.1f", frame)) ms; SuperEditr's steps (frame ms | made ms | pt): " +
                glide.steps.map { String(format: "%.1f|%.1f|%.2f", $0.frame, $0.made, $0.pt) }.joined(separator: ", "))
            log("NOTE intervals between SuperEditr's steps (ms): \(intervals.map { String(format: "%.1f", $0) }.joined(separator: ", "))")
            for (index, interval) in intervals.enumerated() where interval > frame * 1.5 {
                problems.append(String(format: "step %d came %.1f ms after the one before", index + 2, interval))
            }
            var previous = 0.0
            for (index, step) in glide.steps.enumerated() {
                let share = easeOut(step.frame / 200)
                let expected = (share - previous) * glide.distance
                previous = share
                if abs(step.pt - expected) > 0.3 {
                    problems.append(String(format: "step %d moved %.2f pt, the curve %.2f pt", index + 1, step.pt, expected))
                }
            }
        } else {
            problems.append("SuperEditr logged no glide")
        }
        log("NOTE glide steps seen on screen (pt): \(drag.steps.map { String(format: "%.1f", $0) }.joined(separator: ", "))")
        log("NOTE this test's sample gap before each (ms): \(drag.gaps.map { String(format: "%.1f", $0) }.joined(separator: ", "))")
        for (index, step) in drag.steps.enumerated() where step > drag.meanStep * 2 {
            let gap = index < drag.gaps.count ? drag.gaps[index] : 0
            if gap > 1000.0 / 60 * 1.5 {
                log(String(format: "NOTE on-screen step %d (%.1f pt) is excused: this test's samples around it were %.1f ms apart", index + 1, step, gap))
            } else {
                problems.append(String(format: "on-screen step %d moved %.1f pt, %.2f× the mean, with samples %.1f ms apart", index + 1, step, step / drag.meanStep, gap))
            }
        }
        if !drag.monotonic { problems.append("the glide went back") }
        check(problems.isEmpty, "the glide is smooth", problems.joined(separator: "; "))
        let follow = ((try? String(contentsOfFile: captureLogPath, encoding: .utf8)) ?? "").split(separator: "\n")
            .filter { $0.contains("follow:") }.suffix(4)
        log("NOTE SuperEditr's log of the drag: \(follow.joined(separator: " | "))")
        screenshot(window: overlay(), to: outDir + "/review-dragged.png")
    } else {
        check(false, "the title-bar drag moves TextEdit's window", "no TextEdit window or panel")
    }
    if let idleStart = cpuSeconds(pid) {
        pump(2)
        if let idleEnd = cpuSeconds(pid) { log(String(format: "NOTE SuperEditr's CPU with the panel open and nothing moving: %.1f%% of a core", (idleEnd - idleStart) / 2 * 100)) }
    }
    KeyLog.seen.removeAll()
    press(keyEscape)
    check(waitFor(3) { overlay() == nil ? true : nil } != nil, "Escape still closes the panel after the drag", "overlay still showing")
    check(!KeyLog.seen.contains { $0.code == Int64(keyEscape) }, "that Escape never reaches TextEdit", "Escape reached TextEdit")
    // Open the review again for the next check.
    press(keyE, .maskCommand)
    pump(0.1)
    press(keyE, .maskCommand)
    check(waitFor(5) { overlay() } != nil, "⌘E E opens the review again", "no overlay within 5 s")
}

// 3d. During a review, a key that isn't the panel's goes where it was headed. ⌘⇧2 is not the
// panel's ⌘2 (switch style): it reaches TextEdit, as another app's own shortcut would reach that app.
// And once a launcher's search box has the keyboard, typing goes to the launcher. Spotlight stands
// in for any launcher: it takes the keyboard without coming to the front, so TextEdit stays the app
// in front. Either way the user has moved on, so the review closes.
let key2: CGKeyCode = 19, keyX: CGKeyCode = 7
/// The app that has the keyboard, as Accessibility reports it.
func keyboardApp() -> pid_t? {
    guard let app = attribute(AXUIElementCreateSystemWide(), kAXFocusedApplicationAttribute) else { return nil }
    var pid: pid_t = 0
    return AXUIElementGetPid(app as! AXUIElement, &pid) == .success ? pid : nil
}
func reopenReview() -> Bool {
    press(keyE, .maskCommand)
    pump(0.1)
    press(keyE, .maskCommand)
    return waitFor(5) { overlay() } != nil
}
if overlay() != nil {
    KeyLog.seen.removeAll()
    press(key2, [.maskCommand, .maskShift])
    check(waitFor(2) { KeyLog.seen.contains { $0.code == Int64(key2) } ? true : nil } != nil,
          "⌘⇧2 reaches TextEdit during a review", "⌘⇧2 never reached TextEdit")
    check(waitFor(3) { overlay() == nil ? true : nil } != nil, "a shortcut meant for the app closes the review", "overlay still showing")
    check(reopenReview(), "⌘E E opens the review after ⌘⇧2", "no overlay within 5 s")
    let textEdit = running("com.apple.TextEdit").first?.processIdentifier
    press(keySpace, .maskCommand)
    let launcher = waitFor(3) { () -> pid_t? in
        guard let pid = keyboardApp(), pid != textEdit else { return nil }
        return pid
    }
    if launcher == nil {
        log("NOTE ⌘Space gave the keyboard to nothing else (\(keyboardApp().map(String.init) ?? "nothing") has it), so the launcher check didn't run")
    } else {
        screenshot(window: nil, rect: NSScreen.screens.first?.frame, to: outDir + "/review-under-launcher.png")
        KeyLog.seen.removeAll()
        press(keyX)
        check(waitFor(2) { KeyLog.seen.contains { $0.code == Int64(keyX) } ? true : nil } != nil,
              "typing reaches a launcher opened over a review", "the key never left SuperEditr")
        check(waitFor(3) { overlay() == nil ? true : nil } != nil, "typing in the launcher closes the review", "overlay still showing")
        // Esc clears the launcher's box, then closes it.
        for _ in 0..<3 where keyboardApp() != textEdit {
            press(keyEscape)
            pump(0.5)
        }
        check(keyboardApp() == textEdit, "the launcher closes and TextEdit has the keyboard again",
              "keyboard with \(keyboardApp().map(String.init) ?? "nothing")")
        check(reopenReview(), "⌘E E opens the review after the launcher", "no overlay within 5 s")
    }
}

// 4. Switching to another app closes the panel: it belongs to the app it reviews. Then selecting
// terminal output, read-only text, shows nothing.
check(openFront("Terminal", bundleID: "com.apple.Terminal"), "Terminal opens")
check(waitFor(3) { overlay() == nil ? true : nil } != nil, "switching apps closes the panel", "panel still showing over Terminal")
quitOpened("com.apple.TextEdit")
pump(1.5)
check(selectAll(), "Terminal's text is selected")
if focusedValueSettable() == true { log("NOTE Terminal's text reports an editable value; check 4 proves less") }
pump(3)
check(overlay() == nil, "no automatic suggestion for terminal output", "overlay appeared")
quitOpened("com.apple.Terminal")

// 5. Recording a shortcut that's already Quick fix's (⌃⌥⇧E) reaches the recorder, which says why
// it can't be used, instead of running Quick fix.
let second = launch(appPath, home: home)  // a second launch opens Settings in the running one
_ = waitFor(5) { second.isRunning ? nil : true }
let settings = waitFor(8) { settingsWindow() }
check(settings != nil, "a second launch opens Settings", "SuperEditr windows: \(describeWindows())")
if let settings {
    running(superEditr).first?.activate()
    let shortcuts = waitFor(5) { button(in: settings, containing: "Shortcuts") }
    check(shortcuts != nil, "Settings shows its sidebar")
    if let shortcuts { AXUIElementPerformAction(shortcuts, kAXPressAction as CFString) }
    let recorder = waitFor(5) { button(in: settings, containing: "⌘E E") }
    check(recorder != nil, "Settings → Shortcuts shows the review shortcut")
    if let recorder {
        AXUIElementPerformAction(recorder, kAXPressAction as CFString)
        pump(0.5)
        press(keyE, [.maskControl, .maskAlternate, .maskShift])
        let explained = waitFor(4) { allText(settings, levels: 40).contains("already uses this for Quick fix") ? true : nil }
        check(explained != nil, "recording Quick fix's shortcut explains the conflict", "no explanation shown")
    }
    screenshot(window: nil, rect: frame(of: settings), to: outDir + "/shortcut-recorder.png")

    // Open at login is on by default: the first launch wrote a Launch Agent that starts this copy
    // of SuperEditr, in the throwaway HOME. The switch removes it and writes it again.
    let agent = home + "/Library/LaunchAgents/SuperEditr.plist"
    let binary = URL(fileURLWithPath: appPath + "/Contents/MacOS/supereditr").resolvingSymlinksInPath().path
    func agentStartsThisApp() -> Bool? {
        guard let plist = try? String(contentsOfFile: agent, encoding: .utf8) else { return nil }
        return plist.contains(binary) && plist.contains("RunAtLoad") ? true : nil
    }
    check(agentStartsThisApp() != nil, "Open at login is on by default", (try? String(contentsOfFile: agent, encoding: .utf8)) ?? "no \(agent)")
    let general = waitFor(5) { button(in: settings, containing: "General") }
    if let general { AXUIElementPerformAction(general, kAXPressAction as CFString) }
    let loginSwitch = waitFor(5) {
        findElement(settings) { (attribute($0, kAXRoleAttribute) as? String) == kAXCheckBoxRole && ownText($0).contains("Open at login") }
    }
    check(loginSwitch != nil, "Settings → General shows Open at login")
    if let loginSwitch {
        AXUIElementPerformAction(loginSwitch, kAXPressAction as CFString)
        let removed = waitFor(4) { FileManager.default.fileExists(atPath: agent) ? nil : true }
        check(removed != nil, "turning Open at login off removes the Launch Agent", "\(agent) still exists")
        AXUIElementPerformAction(loginSwitch, kAXPressAction as CFString)
        check(waitFor(4) { agentStartsThisApp() } != nil, "turning Open at login on adds the Launch Agent again", "no Launch Agent for \(binary)")
    }

    // The apps SuperEditr never reads are listed on Privacy, beside the Accessibility permission:
    // by name where the app is installed, by bundle ID where it isn't. Passwords, which comes with
    // macOS 15 and later, joined the list as a later default when SuperEditr loaded settings.json.
    let privacy = waitFor(5) { button(in: settings, containing: "Privacy") }
    if let privacy { AXUIElementPerformAction(privacy, kAXPressAction as CFString) }
    let listed = ["com.1password.1password", "com.apple.Passwords"].map { id in
        (id: id, shown: NSWorkspace.shared.urlForApplication(withBundleIdentifier: id).map { appName($0.path) } ?? id)
    }
    let excluded = waitFor(5) { () -> Bool? in
        let text = allText(settings, levels: 40)
        return text.contains("Password fields are never read") && listed.allSatisfy { text.contains($0.shown) } ? true : nil
    }
    check(excluded != nil, "Settings → Privacy lists the apps SuperEditr never reads",
          "no \(listed.map(\.shown)) beside the Accessibility permission")
    let privacyText = allText(settings, levels: 40)
    let unnamed = listed.filter { $0.shown != $0.id && privacyText.contains($0.id) }.map(\.id)
    check(unnamed.isEmpty, "Privacy names installed apps instead of showing bundle IDs", "still showing \(unnamed)")

    // Add app… opens a panel on /Applications. Choosing a system app there adds its bundle ID to
    // settings.json and lists it by name.
    let pickPath = ["/System/Applications/Chess.app", "/System/Applications/Calculator.app"].first {
        FileManager.default.fileExists(atPath: $0)
    }
    let addApp = waitFor(3) { button(in: settings, containing: "Add app") }
    check(addApp != nil, "Settings → Privacy offers Add app…")
    if let addApp, let pickPath, let pickID = Bundle(path: pickPath)?.bundleIdentifier {
        running(superEditr).first?.activate()
        AXUIElementPerformAction(addApp, kAXPressAction as CFString)
        func openSheet() -> AXUIElement? { findElement(settings) { (attribute($0, kAXRoleAttribute) as? String) == kAXSheetRole } }
        let sheet = waitFor(5) { openSheet() }
        check(sheet != nil, "Add app… opens a panel over Settings", "no sheet on the Settings window")
        if let sheet {
            pump(0.5)
            screenshot(window: nil, rect: frame(of: settings), to: outDir + "/add-app-panel.png")
            // Click the app in the panel's list, as a person does, which enables Add.
            let item = waitFor(3) {
                findElement(sheet) {
                    (attribute($0, kAXRoleAttribute) as? String) == kAXTextFieldRole
                        && ownText($0).trimmingCharacters(in: .whitespaces) == appName(pickPath)
                }
            }
            if let item { click(item) }
            pump(0.5)
            screenshot(window: nil, rect: frame(of: settings), to: outDir + "/add-app-chosen.png")
            let add = waitFor(5) { () -> AXUIElement? in
                findElement(sheet) {
                    (attribute($0, kAXRoleAttribute) as? String) == kAXButtonRole && attribute($0, kAXTitleAttribute) as? String == "Add"
                        && attribute($0, kAXEnabledAttribute) as? Bool == true
                }
            }
            if let add { AXUIElementPerformAction(add, kAXPressAction as CFString) }
            // The panel lists the app too, so the name counts only once the panel has closed.
            let saved = waitFor(5) { () -> Bool? in
                let json = (try? String(contentsOfFile: support + "/settings.json", encoding: .utf8)) ?? ""
                return json.contains("\"\(pickID)\"") && openSheet() == nil
                    && allText(settings, levels: 40).contains(appName(pickPath)) ? true : nil
            }
            check(saved != nil, "choosing \(appName(pickPath)) in the panel excludes it and lists it by name",
                  item == nil ? "the panel doesn't list \(appName(pickPath))"
                      : add == nil ? "clicking \(appName(pickPath)) didn't enable Add"
                      : "settings.json has no \(pickID), or Privacy doesn't show \(appName(pickPath))")
            if saved == nil {
                try? dumpTree(settings).write(toFile: outDir + "/add-app-accessibility.txt", atomically: true, encoding: .utf8)
                log("     Settings' Accessibility tree: \(outDir)/add-app-accessibility.txt")
            }
            // Never leave the panel up over the checks that follow.
            for _ in 0..<2 where openSheet() != nil {
                press(keyEscape)
                pump(0.5)
            }
        }
    }
    screenshot(window: nil, rect: frame(of: settings), to: outDir + "/settings-privacy.png")

    // Get Started's "Find my AI accounts" opens Models and scans there: one place finds AIs.
    let started = waitFor(5) { button(in: settings, containing: "Get Started") }
    if let started { AXUIElementPerformAction(started, kAXPressAction as CFString) }
    let find = waitFor(5) { button(in: settings, containing: "Find my AI accounts") }
    check(find != nil, "Get Started offers Find my AI accounts")
    if let find {
        AXUIElementPerformAction(find, kAXPressAction as CFString)
        // Only Models' scan button reads "Scanning…" or "Scan again" (headings come through
        // Accessibility in their styled capitals, so match the button, not page text).
        let scanned = waitFor(8) { () -> Bool? in
            let scanButton = button(in: settings, containing: "Scanning") ?? button(in: settings, containing: "Scan again")
            return scanButton == nil ? nil : true
        }
        check(scanned != nil, "Find my AI accounts opens Models and scans", "Models didn't show a scan")
        check(panelText().contains("Early access"), "Models labels the cloud AIs Early access")
        screenshot(window: nil, rect: frame(of: settings), to: outDir + "/find-accounts.png")
    }
    if shortcuts == nil || recorder == nil {
        try? dumpTree(settings).write(toFile: outDir + "/settings-accessibility.txt", atomically: true, encoding: .utf8)
        log("     Settings' Accessibility tree: \(outDir)/settings-accessibility.txt")
    }
}
stop(app)

// 6. Always keep: rejecting the rule-based checker's guess at a name ("Rhyan" → "Ryan") offers to add it
// to the word list, and A adds it. No Apple Intelligence (its helper path doesn't exist), so the
// rule-based checker answers on every Mac. Automatic suggestions are off: one would open without focus
// and race ⌘E E, and the keys below must reach the panel. The diagnostic log stays on for the
// Chrome check.
let named = work + "/named.txt"
try! "Rhyan has tennis lessons today.".write(toFile: named, atomically: true, encoding: .utf8)
try! #"{"onboarded": true, "excluded_apps": ["com.1password.1password"], "auto_popup": false, "diagnostic_log": true}"#
    .write(toFile: support + "/settings.json", atomically: true, encoding: .utf8)
app = launch(appPath, home: home, environment: ["SUPEREDITR_FM_HELPER": work + "/no-helper"])
pump(3)
check(openFront("TextEdit", bundleID: "com.apple.TextEdit", file: named), "TextEdit opens the named draft")
check(selectAll(), "the named draft's text is selected")
press(keyE, .maskCommand)
pump(0.1)
press(keyE, .maskCommand)
let keepPanel = waitFor(5) { overlay() }
check(keepPanel != nil, "⌘E E reviews the named draft", "no overlay within 5 s")
if keepPanel != nil {
    pump(1.5)  // the rule-based checker's result
    // The footer's "replace", not the tip line's "replaces". The web view's text can take a moment
    // to reach Accessibility.
    let footer = waitFor(3) { () -> String? in
        let text = panelText()
        return text.range(of: #"\breplace\b"#, options: .regularExpression) != nil ? text : nil
    }
    check(footer != nil, "the footer shows ⌥↩ replace", "panel text: \(panelText())")
    press(keyTab)
    pump(0.3)
    press(keySpace)
    pump(0.5)
    screenshot(window: overlay(), to: outDir + "/always-keep.png")
    press(keyA)
    pump(0.5)  // the confirmation row slides in below the footer
    screenshot(window: overlay(), to: outDir + "/always-keep-added.png")
    let saved = waitFor(3) { () -> [String]? in
        guard let data = FileManager.default.contents(atPath: support + "/settings.json"),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let words = json["words"] as? [String], words.contains("Rhyan") else { return nil }
        return words
    }
    check(saved != nil, "A adds the kept name to the word list", "settings.json has no \"Rhyan\" in words")
    press(keyEscape)
}
quitOpened("com.apple.TextEdit")

// 6a. With nothing to fix, the footer says so and offers no replace key.
let clean = work + "/clean.txt"
try! "We meet at noon.".write(toFile: clean, atomically: true, encoding: .utf8)
check(openFront("TextEdit", bundleID: "com.apple.TextEdit", file: clean), "TextEdit opens the clean draft")
check(selectAll(), "the clean draft's text is selected")
press(keyE, .maskCommand)
pump(0.1)
press(keyE, .maskCommand)
let cleanPanel = waitFor(5) { overlay() }
check(cleanPanel != nil, "⌘E E reviews the clean draft", "no overlay within 5 s")
if cleanPanel != nil {
    pump(1.5)  // the rule-based checker's result
    screenshot(window: overlay(), to: outDir + "/no-changes.png")
    let footer = waitFor(3) { () -> String? in
        let text = panelText()
        return text.contains("No changes") ? text : nil
    }
    check(footer.map { !$0.contains("⌥↩") } ?? false, "no changes hides the replace key", "panel text: \(footer ?? panelText())")
    // Minimising the reviewed window closes the panel, as Esc would.
    let minimised = mainWindow("com.apple.TextEdit").map {
        AXUIElementSetAttributeValue($0, kAXMinimizedAttribute as CFString, kCFBooleanTrue)
    }
    check(minimised == .success, "the test minimises TextEdit's window", "set: \(minimised.map { "\($0.rawValue)" } ?? "no window")")
    check(waitFor(3) { overlay() == nil ? true : nil } != nil, "minimising the window closes the panel", "panel still showing")
    if overlay() != nil { press(keyEscape) }
}
quitOpened("com.apple.TextEdit")

// 6a-2. Clicking the footer's ⌥↩ replace does what the key does: the fix goes into the document.
let clickDraft = work + "/click.txt"
try! "We meet tommorow at noon.".write(toFile: clickDraft, atomically: true, encoding: .utf8)
check(openFront("TextEdit", bundleID: "com.apple.TextEdit", file: clickDraft), "TextEdit opens the draft to fix with a click")
check(selectAll(), "the draft's text is selected")
press(keyE, .maskCommand)
pump(0.1)
press(keyE, .maskCommand)
let clickPanel = waitFor(5) { overlay() }
check(clickPanel != nil, "⌘E E reviews the draft to fix with a click", "no overlay within 5 s")
if clickPanel != nil {
    let replaceButton = waitFor(5) { () -> AXUIElement? in
        guard let app = running(superEditr).first,
              let list = attribute(AXUIElementCreateApplication(app.processIdentifier), kAXWindowsAttribute) as? [AXUIElement] else { return nil }
        return list.lazy.compactMap { button(in: $0, containing: "⌥↩") }.first
    }
    check(replaceButton != nil, "the footer's ⌥↩ replace is a button", "panel text: \(panelText())")
    if let replaceButton {
        click(replaceButton)
        let fixed = waitFor(5) { () -> String? in
            let value = focusedElement().flatMap { attribute($0, kAXValueAttribute) as? String } ?? ""
            return value.contains("tomorrow") ? value : nil
        }
        check(fixed != nil, "clicking ⌥↩ replace puts the fix in the document", "the document still holds the typo")
        check(waitFor(3) { overlay() == nil ? true : nil } != nil, "clicking ⌥↩ replace closes the panel", "panel still showing")
    }
    if overlay() != nil { press(keyEscape) }
}
quitOpened("com.apple.TextEdit")

// 6b. A selection holding a secret isn't reviewed: ⌘E E says why instead of opening a review. The
// token is assembled from pieces so the source holds no secret-shaped string.
let secretDraft = work + "/secret.txt"
try! "Deploy with \("ghp_" + "Ab3dE5gH7jK9mN1pQ3sT5vW7yZ9bC1dF2hJ4") before Friday."
    .write(toFile: secretDraft, atomically: true, encoding: .utf8)
check(openFront("TextEdit", bundleID: "com.apple.TextEdit", file: secretDraft), "TextEdit opens the draft with a token")
check(selectAll(), "the token draft's text is selected")
press(keyE, .maskCommand)
pump(0.1)
press(keyE, .maskCommand)
let refusal = waitFor(5) { () -> String? in
    let text = panelText()
    return text.contains("a GitHub token") ? text : nil
}
check(refusal != nil, "a selected token is refused, not reviewed", "no message naming a GitHub token within 5 s")
check(refusal.map { !$0.contains("Ab3dE5gH7jK9") } ?? true, "the refusal doesn't show the token")
screenshot(window: overlay(), to: outDir + "/secret-refused.png")
press(keyEscape)
quitOpened("com.apple.TextEdit")

// 6c. ↩ in the review writes the fix into a web page's text field. Browsers and Electron apps take
// the paste path rather than an Accessibility write. Checked in a plain text box, in an editable div,
// the kind of field chat apps use, and in the composer matrix's editors. Chrome runs as its own copy
// with a throwaway profile.
let chromeApp = "/Applications/Google Chrome.app", chrome = "com.google.Chrome"
if !FileManager.default.fileExists(atPath: chromeApp) {
    log("SKIP applying in a web page: Chrome isn't installed")
} else {
    let typo = "Their going to love this."
    let page = work + "/fields.html"
    // The composer matrix's editors, bundled into one script next to this file.
    let composerScript = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("composers/composers.bundle.js")
    check(FileManager.default.fileExists(atPath: composerScript.path), "the composer editors are there", composerScript.path)
    try! """
    <!doctype html><meta charset="utf-8"><title>Fields</title>
    <textarea id="control" autofocus rows="2" cols="60">Control field.</textarea>
    <textarea id="plain" rows="3" cols="60">\(typo)</textarea>
    <div id="rich" contenteditable="true" style="border:1px solid; width:30em; min-height:3em">\(typo)</div>
    <div id="paragraphs" contenteditable="true" style="border:1px solid; width:30em"><p>\(typo)</p><p><br></p><p>See you soon.</p></div>
    <div id="composers"></div>
    <script src="\(composerScript.absoluteString)"></script>
    """.write(toFile: page, atomically: true, encoding: .utf8)
    let chromeOpened = openFront(chromeApp, bundleID: chrome, file: page, newInstance: true,
                                 launchArguments: ["--user-data-dir=\(work)/chrome", "--no-first-run", "--no-default-browser-check"])
    check(chromeOpened, "Chrome opens the page with its fields")
    let captureLog = home + "/Library/Logs/SuperEditr/capture.log"
    func lastChromeRead() -> String {
        let lines = (try? String(contentsOfFile: captureLog, encoding: .utf8))?.split(separator: "\n") ?? []
        return lines.last { $0.contains("com.google.Chrome: read") }.map(String.init) ?? "none"
    }
    // Keys go only to the test's own Chrome; stop here rather than type into another one.
    if chromeOpened {
        // Chrome builds its Accessibility tree only once asked, as SuperEditr does when it reads. Asking
        // first, and waiting for the tree, lets this test read the fields itself, and skips the wait for
        // the tree in SuperEditr's first read, which has a check of its own below.
        if let ours = launchedSinceStart(chrome).last {
            let element = AXUIElementCreateApplication(ours.processIdentifier)
            for flag in ["AXManualAccessibility", "AXEnhancedUserInterface"] {
                AXUIElementSetAttributeValue(element, flag as CFString, kCFBooleanTrue)
            }
        }
        pump(2)
        // One review closed straight away makes sure the tree is there for the control below.
        _ = selectAll()
        press(keyE, .maskCommand)
        pump(0.1)
        press(keyE, .maskCommand)
        if waitFor(5, { overlay() }) != nil {
            press(keyEscape)
            _ = waitFor(3) { overlay() == nil ? true : nil }
        }
        // Control, without SuperEditr: does Chrome's window report focus here, and does a plain ⌘V land?
        // This tells a SuperEditr bug apart from a runner session that can't paste into Chrome at all.
        func chromeWindowFocused() -> String {
            guard let ours = launchedSinceStart(chrome).last,
                  let window = attribute(AXUIElementCreateApplication(ours.processIdentifier), kAXFocusedWindowAttribute) else { return "no window" }
            return "main \(attribute(window as! AXUIElement, kAXMainAttribute) as? Bool ?? false), focused \(attribute(window as! AXUIElement, kAXFocusedAttribute) as? Bool ?? false)"
        }
        log("NOTE control: Chrome's window before any paste: \(chromeWindowFocused())")
        for (how, toProcess) in [("a ⌘V keystroke", false), ("⌘V posted to Chrome's process", true)] {
            let marker = "Pasted by the test \(toProcess ? 2 : 1)."
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(marker, forType: .string)
            _ = selectAll()
            if toProcess, let ours = launchedSinceStart(chrome).last {
                for down in [true, false] {
                    let event = CGEvent(keyboardEventSource: nil, virtualKey: keyV, keyDown: down)
                    event?.flags = .maskCommand
                    event?.postToPid(ours.processIdentifier)
                    usleep(25_000)
                }
            } else {
                press(keyV, .maskCommand)
            }
            pump(1)
            let value = focusedElement().flatMap { attribute($0, kAXValueAttribute) as? String } ?? ""
            log("NOTE control: \(how) \(value.contains(marker) ? "landed" : "did not land"); window \(chromeWindowFocused())")
        }
        // The editable div is applied six times in a row, the first straight after the text box
        // and the last three with every CPU core busy, as on a loaded Mac. Back-to-back pastes were
        // dropped by Chrome (spectrumzero-dev/supereditr-app#10). Before each repeat the test puts
        // the typo back by reloading the page, so each of SuperEditr's pastes follows its own last
        // restore, the case Chrome drops.
        let runs: [(field: String, attempt: Int)] = [("text box", 1)] + (1...6).map { ("editable div", $0) }
        var load: [Process] = []
        defer { load.forEach { $0.terminate() } }
        // Keeps every CPU core busy until `load` is terminated.
        func loadCores() {
            for _ in 0..<ProcessInfo.processInfo.activeProcessorCount {
                let spin = Process()
                spin.executableURL = URL(fileURLWithPath: "/usr/bin/yes")
                spin.standardOutput = FileHandle.nullDevice
                try? spin.run()
                load.append(spin)
            }
            log("     loading \(load.count) CPU cores")
        }
        for (index, run) in runs.enumerated() {
            let field = run.field == "editable div" ? "editable div (apply \(run.attempt)\(run.attempt > 3 ? ", busy CPU" : ""))" : run.field
            if run.field == "editable div" && run.attempt == 4 {
                loadCores()
            }
            if run.attempt == 1 {
                press(keyTab)  // from the control field to the text box, then to the editable div
                pump(0.5)
            } else {
                // Reloading puts the typo back without touching the clipboard, so SuperEditr's next
                // paste is the first since its last restore. Focus starts in the control field.
                press(keyR, .maskCommand)
                pump(2.5)
                press(keyTab)
                pump(0.3)
                press(keyTab)
                pump(0.5)
            }
            check(selectAll() || selectAll(), "the \(field)'s text is selected")
            let start = focusedElement().flatMap { attribute($0, kAXValueAttribute) as? String } ?? ""
            check(start.contains(typo), "the \(field) starts with the typo", "it reads \"\(start)\"")
            press(keyE, .maskCommand)
            pump(0.1)
            press(keyE, .maskCommand)
            let panel = waitFor(5) { overlay() }
            check(panel != nil, "⌘E E reviews the \(field)", "no overlay within 5 s")
            guard panel != nil else { continue }
            pump(1.5)  // the rule-based checker's result
            let read = lastChromeRead()
            check(read.contains("via Accessibility"), "the \(field) is read through Accessibility", "last read: \(read)")
            screenshot(window: overlay(), to: outDir + "/web-\(index)-review.png")
            press(keyReturn)
            let notice = waitFor(3) { () -> String? in
                guard let app = running(superEditr).first,
                      let list = attribute(AXUIElementCreateApplication(app.processIdentifier), kAXWindowsAttribute) as? [AXUIElement] else { return nil }
                let text = list.map { allText($0, levels: 40) }.joined(separator: " ")
                return text.contains("confirm") ? text : nil
            }
            let value = focusedElement().flatMap { attribute($0, kAXValueAttribute) as? String } ?? ""
            check(notice == nil, "↩ applies in the \(field) without a warning", "SuperEditr said: \(notice ?? "")")
            check(!value.isEmpty && !value.contains(typo), "the \(field) holds the fix", "the field reads \"\(value)\"")
            screenshot(window: nil, rect: windows(bundleID: chrome).first?.frame, to: outDir + "/web-\(index)-applied.png")
            if overlay() != nil {
                log("NOTE the panel was still open after ↩ in the \(field)")
                press(keyEscape)
                _ = waitFor(3) { overlay() == nil ? true : nil }
            }
        }
        // The load is for those applies only: left on, it slows every later step, screenshots most.
        load.forEach { $0.terminate() }
        load.removeAll()
        // Paragraphs around an empty one, as in Claude's message box (ProseMirror): Chrome's own text
        // for the field leaves the empty paragraph out, and ⌘A's selected text can drop the break
        // between paragraphs. SuperEditr reads the field from its paragraphs, so the empty line
        // reaches the review and survives the fix.
        func byDOMIdentifier(_ element: AXUIElement, _ id: String, depth: Int = 0) -> AXUIElement? {
            if (attribute(element, "AXDOMIdentifier") as? String) == id { return element }
            guard depth < 40, let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] else { return nil }
            return children.lazy.compactMap { byDOMIdentifier($0, id, depth: depth + 1) }.first
        }
        let paragraphText = "\(typo)\n\nSee you soon."  // the typo, so the rule-based checker has a fix
        if let ours = launchedSinceStart(chrome).last,
           let field = byDOMIdentifier(AXUIElementCreateApplication(ours.processIdentifier), "paragraphs") {
            AXUIElementSetAttributeValue(field, kAXFocusedAttribute as CFString, kCFBooleanTrue)
            pump(0.5)
            check(selectAll(), "the paragraphs' text is selected")
            press(keyE, .maskCommand)
            pump(0.1)
            press(keyE, .maskCommand)
            let panel = waitFor(5) { overlay() }
            check(panel != nil, "⌘E E reviews the paragraphs", "no overlay within 5 s")
            if panel != nil {
                pump(1.5)  // the rule-based checker's result
                let read = lastChromeRead()
                check(read.contains("read \(paragraphText.count) characters"), "the empty paragraph is read",
                      "expected \(paragraphText.count) characters; last read: \(read)")
                screenshot(window: overlay(), to: outDir + "/web-paragraphs-review.png")
                press(keyReturn)
                _ = waitFor(3) { overlay() == nil ? true : nil }
                pump(1)
                // Chrome's own paste may keep the empty line as an empty paragraph or as two line breaks.
                let groups = (attribute(field, kAXChildrenAttribute) as? [AXUIElement]) ?? []
                let texts = groups.map { allText($0, levels: 4).trimmingCharacters(in: .whitespaces) }
                let value = attribute(field, kAXValueAttribute) as? String ?? ""
                let emptyLine = (texts.count == 3 && texts[1].isEmpty) || value.contains("\n\n")
                check(emptyLine && !value.contains(typo), "the fix keeps the empty line",
                      "the field reads \(String(reflecting: value)), paragraphs \(texts)")
                screenshot(window: nil, rect: windows(bundleID: chrome).first?.frame, to: outDir + "/web-paragraphs-applied.png")
            }
        } else {
            check(false, "the paragraphs field is found", "no element with DOM id \"paragraphs\"")
        }
        // The composer matrix: the same text in an editable div that pastes HTML, a Markdown composer
        // like Claude's desktop app (it pastes from the plain text alone, reads it as Markdown, and pads
        // code with zero-width spaces), and four open-source editors chat and issue-tracker composers are
        // built on: TipTap, Lexical, Slate (Discord's) and Quill (Signal Desktop's), in tests/ui/composers.
        // Each field holds two typos to fix, code at both ends of its first paragraph (one with a typo of
        // its own), bold, a link and an empty paragraph, and is reviewed once. The proofread must be
        // applied in place, one edit per change, and keep the code, bold and link as they were. The editable div is applied with every CPU core busy:
        // Chrome then updates its accessibility tree late after each edit, and the next edit must
        // wait for it (it failed whenever two test VMs ran at once).
        let composerFields = [("code", "editable div"), ("markdown", "Markdown composer"), ("tiptap", "TipTap"), ("lexical", "Lexical"),
                              ("slate", "Slate"), ("quill", "Quill")]
        let codes = ["teh fix", "their going to work"], composerTypo = "Their going to love"
        let composerFixes = ["They're going to love", "really run"]
        let visible = { (text: String) in String(text.unicodeScalars.filter { !["\u{200B}", "\u{2060}", "\u{FEFF}"].contains($0) }) }
        func chromeElement(_ id: String) -> AXUIElement? {
            guard let ours = launchedSinceStart(chrome).last else { return nil }
            return byDOMIdentifier(AXUIElementCreateApplication(ours.processIdentifier), id)
        }
        // What the editor's DOM holds, from the page's description of it: "code=a|b; bold=c; …".
        func composerState(_ id: String) -> [String: String] {
            let text = chromeElement("\(id)-state").map { allText($0, levels: 4) } ?? ""
            return Dictionary(text.split(separator: ";").compactMap { pair -> (String, String)? in
                let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
                guard parts.count == 2 else { return nil }
                return (parts[0].trimmingCharacters(in: .whitespaces), parts[1].trimmingCharacters(in: .whitespaces))
            }, uniquingKeysWith: { $1 })
        }
        // Selects all of the field's text and opens a review, waiting until the review has a fix to
        // apply. A partial selection fails: ⌘E E would then review the sentence alone, and the code and
        // the empty paragraph around it would pass untouched.
        func reviewField(_ field: AXUIElement) -> Bool {
            AXUIElementSetAttributeValue(field, kAXFocusedAttribute as CFString, kCFBooleanTrue)
            _ = selectAll() || selectAll()
            let selected = visible(focusedElement().flatMap { attribute($0, kAXSelectedTextAttribute) as? String } ?? "")
            guard ([composerTypo, "See you soon."] + codes).allSatisfy(selected.contains) else {
                log("     the whole field isn't selected: \(String(reflecting: selected))")
                return false
            }
            press(keyE, .maskCommand)
            pump(0.1)
            press(keyE, .maskCommand)
            guard waitFor(5, { overlay() }) != nil else { return false }
            return waitFor(4) { panelText().range(of: #"\b\d+ changes?\b"#, options: .regularExpression) != nil ? true : nil } != nil
        }
        var matrix: [String] = []
        for (id, editor) in composerFields {
            let name = "the \(editor) field"
            guard let field = chromeElement(id) else {
                check(false, "\(name) is found", "no element with DOM id \"\(id)\"")
                matrix.append("\(editor): not found")
                continue
            }
            var row: [String] = []
            func expect(_ ok: Bool, _ short: String, _ what: String, _ detail: @autoclosure () -> String) {
                check(ok, "\(name): \(what)", detail())
                row.append("\(short) \(ok ? "ok" : "FAIL")")
            }
            let opened = reviewField(field)
            expect(opened, "review", "⌘E E opens a review of the whole field with a fix",
                   "no review of the whole field with a fix within 9 s; panel text: \(panelText())")
            if opened {
                let read = lastChromeRead()
                expect(read.contains("via Accessibility"), "read", "it is read through Accessibility", "last read: \(read)")
                let lines = ((try? String(contentsOfFile: captureLog, encoding: .utf8)) ?? "").split(separator: "\n")
                let found = lines.last { $0.contains("code in the field") }.map(String.init) ?? "none"
                expect(found.contains("2 pieces"), "code found", "both code spans are found", "last code line: \(found)")
                log("NOTE \(name): \(lines.last { $0.contains("code placed") }.map(String.init) ?? "no code placed")")
                screenshot(window: overlay(), to: outDir + "/composer-\(id)-review.png")
                if id == "code" {
                    loadCores()
                }
                press(keyReturn)
                _ = waitFor(3) { overlay() == nil ? true : nil }
                // The page describes the field once the editor has taken the paste.
                let value = waitFor(3) { () -> String? in
                    let value = attribute(field, kAXValueAttribute) as? String ?? ""
                    return !value.isEmpty && !value.contains(composerTypo) ? value : nil
                }
                load.forEach { $0.terminate() }
                load.removeAll()
                // The page describes the field a frame after it changes: wait until it no longer has the typo.
                let state = waitFor(3) { () -> [String: String]? in
                    let state = composerState(id)
                    return state["typo"] == "0" ? state : nil
                } ?? composerState(id)
                expect(value != nil, "fixed", "↩ writes the fix", "the field reads \(String(reflecting: attribute(field, kAXValueAttribute) as? String ?? ""))")
                expect(composerFixes.allSatisfy(visible(value ?? "").contains), "both fixes", "both changes are applied",
                       "the field reads \(String(reflecting: value ?? ""))")
                let path = appliedPath("com.google.Chrome")
                expect(path.contains("applied in place: 2 edits"), "in place", "the proofread is applied in place", "the log says: \(path)")
                expect(codes.allSatisfy(visible(value ?? "").contains), "code kept", "the code is left as written",
                       "the field reads \(String(reflecting: value ?? ""))")
                expect(state["code"] == codes.joined(separator: "|"), "code format", "the code is still formatted as code",
                       "the page reports \(state)")
                expect(state["paragraphs"] == "3" && state["empty"] == "1", "empty paragraph", "the empty paragraph survives",
                       "the page reports \(state)")
                expect(state["bold"] == "today", "bold", "bold survives the fix", "the page reports \(state)")
                expect(state["links"] == "the notes", "link", "the link survives the fix", "the page reports \(state)")
                screenshot(window: nil, rect: windows(bundleID: chrome).first?.frame, to: outDir + "/composer-\(id)-applied.png")
                if overlay() != nil {
                    press(keyEscape)
                    _ = waitFor(3) { overlay() == nil ? true : nil }
                }
            }
            matrix.append("\(editor): \(row.joined(separator: ", "))")
        }
        log("     composer matrix:")
        matrix.forEach { log("       \($0)") }
        // The formats matrix: a document with every common structure (a heading, bold, italic, a link and
        // code in a paragraph, a bulleted list with a nested item, a numbered list, a quote, a code block, a
        // line break inside a paragraph) in each editor, with typos in the prose and in the code. Each
        // field must get its fixes and keep every structure and its code exactly. The text SuperEditr
        // reviewed (the panel) and the field's Accessibility tree go to the artifacts, to show how Chrome
        // exposes each structure.
        let formatFields = [("formats-div", "editable div"), ("formats-markdown", "Markdown composer"),
                            ("formats-tiptap", "TipTap"), ("formats-lexical", "Lexical"),
                            ("formats-slate", "Slate"), ("formats-quill", "Quill")]
        func axDump(_ element: AXUIElement, depth: Int = 0) -> String {
            let role = attribute(element, kAXRoleAttribute) as? String ?? "?"
            let subrole = attribute(element, kAXSubroleAttribute) as? String ?? ""
            let value = (attribute(element, kAXValueAttribute) as? String).map { " " + String(reflecting: $0) } ?? ""
            let line = String(repeating: "  ", count: depth) + role + (subrole.isEmpty ? "" : "/" + subrole) + value
            guard depth < 30, let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] else { return line }
            return ([line] + children.map { axDump($0, depth: depth + 1) }).joined(separator: "\n")
        }
        // "headings=1 items=3 …; text=…" → the structure, and the text.
        func formatsState(_ id: String) -> (structure: String, text: String) {
            let raw = chromeElement("\(id)-state").map { allText($0, levels: 4) } ?? ""
            let parts = raw.components(separatedBy: "; text=")
            return (parts.first ?? "", parts.count > 1 ? parts[1] : "")
        }
        var formatsRows: [String] = []
        for (id, editor) in formatFields {
            let name = "the \(editor) formats field"
            guard let field = chromeElement(id) else {
                check(false, "\(name) is found", "no element with DOM id \"\(id)\"")
                continue
            }
            let before = formatsState(id)
            try? axDump(field).write(toFile: outDir + "/formats-\(id)-accessibility.txt", atomically: true, encoding: .utf8)
            // How Chrome's selection offsets line up with its text: select each word where it is in
            // the field's value and record what Chrome selects. SuperEditr places edits this way.
            AXUIElementSetAttributeValue(field, kAXFocusedAttribute as CFString, kCFBooleanTrue)
            let fieldValue = (attribute(field, kAXValueAttribute) as? String ?? "") as NSString
            var probes: [String] = []
            for word in ["Weekly", "teh", "Apples", "Ripe", "Plums", "First", "Secend", "realy", "let", "two", "See"] {
                let at = fieldValue.range(of: word)
                guard at.location != NSNotFound else { probes.append("\(word): not in the value"); continue }
                var range = CFRange(location: at.location, length: at.length)
                if let value = AXValueCreate(.cfRange, &range) {
                    AXUIElementSetAttributeValue(field, kAXSelectedTextRangeAttribute as CFString, value)
                }
                pump(0.1)
                let got = attribute(field, kAXSelectedTextAttribute) as? String ?? "nil"
                probes.append("\(word)@\(at.location): \(got == word ? "ok" : "got \(String(reflecting: got))")")
            }
            log("NOTE \(name) selection offsets: \(probes.joined(separator: ", "))")
            AXUIElementSetAttributeValue(field, kAXFocusedAttribute as CFString, kCFBooleanTrue)
            _ = selectAll() || selectAll()
            let selected = visible(focusedElement().flatMap { attribute($0, kAXSelectedTextAttribute) as? String } ?? "")
            guard selected.contains("Weekly notes") && selected.contains("you soon.") else {
                check(false, "\(name): the whole field is selected", "selected \(String(reflecting: selected))")
                formatsRows.append("\(editor): not selected")
                continue
            }
            press(keyE, .maskCommand)
            pump(0.1)
            press(keyE, .maskCommand)
            let opened = waitFor(5, { overlay() }) != nil
                && waitFor(4) { panelText().range(of: #"\b\d+ changes?\b"#, options: .regularExpression) != nil ? true : nil } != nil
            check(opened, "\(name): ⌘E E opens a review with a fix", "panel text: \(panelText())")
            guard opened else {
                formatsRows.append("\(editor): no review")
                press(keyEscape)
                continue
            }
            let read = lastChromeRead()
            try? "\(read)\n\n\(panelText())".write(toFile: outDir + "/formats-\(id)-review.txt", atomically: true, encoding: .utf8)
            // Every line break read: the whole field, plus the empty paragraph when Chrome's own text
            // leaves it out. Slate puts a zero-width character in an empty paragraph, so there Chrome's
            // text already has the empty line.
            let hasEmptyLine = (fieldValue as String).components(separatedBy: "\n").contains { visible($0).isEmpty }
            let counts = read.range(of: #"read (\d+) characters of (\d+)"#, options: .regularExpression)
                .map { String(read[$0]).split(separator: " ").compactMap { Int($0) } }
            check(counts.map { $0[0] == $0[1] + (hasEmptyLine ? 0 : 1) } ?? false, "\(name): it is read with every line break, the empty one included",
                  "last read: \(read)")
            screenshot(window: overlay(), to: outDir + "/formats-\(id)-review.png")
            press(keyReturn)
            _ = waitFor(3) { overlay() == nil ? true : nil }
            let after = waitFor(3) { () -> (structure: String, text: String)? in
                let state = formatsState(id)
                return ["Secend", "realy", "you you"].contains(where: state.text.contains) ? nil : state
            } ?? formatsState(id)
            let fixed = !["Secend", "realy", "you you"].contains(where: after.text.contains)
            check(fixed, "\(name): the fixes land", "the field holds \(String(reflecting: after.text))")
            check(after.structure == before.structure, "\(name): every structure and the code survive the fix",
                  "before \(before.structure); after \(after.structure)")
            let path = appliedPath("com.google.Chrome")
            log("NOTE \(name): \(path)")
            formatsRows.append("\(editor): \(fixed ? "fixed" : "NOT fixed"), \(after.structure == before.structure ? "structure kept" : "STRUCTURE LOST"), \(path.contains("applied in place:") ? "in place" : "whole")")
            screenshot(window: nil, rect: windows(bundleID: chrome).first?.frame, to: outDir + "/formats-\(id)-applied.png")
            if overlay() != nil {
                press(keyEscape)
                _ = waitFor(3) { overlay() == nil ? true : nil }
            }
        }
        log("     formats matrix:")
        formatsRows.forEach { log("       \($0)") }
        // ⌥↩ pressed again while the first one applies used to hang SuperEditr: applying turns the
        // panel's shortcuts off while the main thread handles the next press. With the typo put back in
        // the last field, press it three times in a row, then check that ⌘E E still opens a review.
        if let reset = chromeElement("reset-lexical") { AXUIElementPerformAction(reset, kAXPressAction as CFString) }
        if let field = waitFor(3, { () -> AXUIElement? in
               guard let field = chromeElement("lexical"), (attribute(field, kAXValueAttribute) as? String)?.contains(composerTypo) == true else { return nil }
               return field
           }), reviewField(field) {
            for _ in 0..<3 {
                press(keyReturn, .maskAlternate)
                usleep(15_000)
            }
            _ = waitFor(3) { overlay() == nil ? true : nil }
            _ = selectAll()
            press(keyE, .maskCommand)
            pump(0.1)
            press(keyE, .maskCommand)
            check(waitFor(5, { overlay() }) != nil, "⌘E E still opens a review after ⌥↩ pressed three times quickly",
                  "no overlay within 5 s: SuperEditr may be stuck")
            press(keyEscape)
            _ = waitFor(3) { overlay() == nil ? true : nil }
        } else {
            check(false, "⌘E E reviews the field before the ⌥↩ presses", "no review of the whole field with a fix within 9 s")
        }
        // How many tries each paste needed, from SuperEditr's log, to show how often the retry helps.
        let pastes = ((try? String(contentsOfFile: captureLog, encoding: .utf8)) ?? "").split(separator: "\n")
            .filter { $0.contains("com.google.Chrome: pasted") }
        let tries = pastes.compactMap { line in line.range(of: #"\((\d) attempts?\)"#, options: .regularExpression).map { String(line[$0]) } }
        log("NOTE Chrome pastes by attempts: \(Dictionary(grouping: tries, by: { $0 }).mapValues(\.count).sorted { $0.key < $1.key })")
    }
    quitOpened(chrome)

    // 6c-1. The first review in a Chromium app that has just started. Its text can't be read until
    // SuperEditr asks, and then only after about 2 s, so that first read has to wait: giving up falls
    // back to a read that can't replace anything. Nothing here asks for the tree first. Chrome stands in
    // for any app on the same runtime that SuperEditr has no name for, such as the Codex app: they
    // answer to the same flag, and the same code sends it (has_chromium_runtime has unit tests).
    let chromeArguments = { (profile: String) in ["--user-data-dir=\(work)/\(profile)", "--no-first-run", "--no-default-browser-check"] }
    let firstPage = work + "/first-review.html"
    try! """
    <!doctype html><meta charset="utf-8"><title>First review</title>
    <textarea autofocus rows="3" cols="60">\(typo)</textarea>
    """.write(toFile: firstPage, atomically: true, encoding: .utf8)
    let firstOpened = openFront(chromeApp, bundleID: chrome, file: firstPage, newInstance: true, launchArguments: chromeArguments("chrome-first"))
    check(firstOpened, "Chrome opens the page for the first review")
    if firstOpened {
        pump(3)  // long enough for the page to load, with no tree yet
        press(keyA, .maskCommand)
        pump(0.5)
        press(keyE, .maskCommand)
        pump(0.1)
        press(keyE, .maskCommand)
        // The review opens once the tree is there, so this waits longer than the others.
        let panel = waitFor(10) { overlay() }
        check(panel != nil, "⌘E E reviews a field in a Chromium app that has just started", "no overlay within 10 s")
        if panel != nil {
            pump(1.5)  // the rule-based checker's result
            let read = lastChromeRead()
            check(read.contains("via Accessibility"), "the first review reads the field through Accessibility", "last read: \(read)")
            let replaceKey = waitFor(3) { () -> String? in
                let text = panelText()
                return text.contains("⌥↩") && text.range(of: #"\breplace\b"#, options: .regularExpression) != nil ? text : nil
            }
            check(replaceKey != nil, "the first review offers ⌥↩ replace", "panel text: \(panelText())")
            screenshot(window: overlay(), to: outDir + "/first-review.png")
            press(keyReturn, .maskAlternate)
            let fixed = waitFor(5) { () -> String? in
                let value = focusedElement().flatMap { attribute($0, kAXValueAttribute) as? String } ?? ""
                return !value.isEmpty && !value.contains(typo) ? value : nil
            }
            check(fixed != nil, "⌥↩ replaces the text in the first review", "the field still holds the typo")
            if overlay() != nil {
                press(keyEscape)
                _ = waitFor(3) { overlay() == nil ? true : nil }
            }
        }
    }
    quitOpened(chrome)

    // 6c-2. ⌥↩ means replace and nothing else. In text that can't be replaced, it changes nothing:
    // the panel stays and says why, the clipboard is left alone, and ⌘C copies the fix.
    let readOnlyPage = work + "/read-only.html"
    try! """
    <!doctype html><meta charset="utf-8"><title>Read only</title>
    <p>\(typo)</p>
    """.write(toFile: readOnlyPage, atomically: true, encoding: .utf8)
    let readOnlyOpened = openFront(chromeApp, bundleID: chrome, file: readOnlyPage, newInstance: true, launchArguments: chromeArguments("chrome-read-only"))
    check(readOnlyOpened, "Chrome opens the read-only page")
    if readOnlyOpened {
        pump(3)
        press(keyA, .maskCommand)
        pump(0.5)
        press(keyE, .maskCommand)
        pump(0.1)
        press(keyE, .maskCommand)
        let panel = waitFor(10) { overlay() }
        check(panel != nil, "⌘E E reviews read-only text", "no overlay within 10 s")
        if panel != nil {
            pump(1.5)  // the rule-based checker's result
            let saysWhy = waitFor(3) { () -> String? in
                let text = panelText()
                return text.contains("replace text here") ? text : nil
            }
            check(saysWhy != nil, "the panel says the text can't be replaced", "panel text: \(panelText())")
            check(saysWhy.map { !$0.contains("⌥↩") && $0.contains("⌘C") } ?? false, "the panel offers ⌘C and not ⌥↩", "panel text: \(saysWhy ?? "")")
            screenshot(window: overlay(), to: outDir + "/read-only.png")
            let untouched = "On the clipboard before ⌥↩."
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(untouched, forType: .string)
            press(keyReturn, .maskAlternate)
            pump(1)
            check(NSPasteboard.general.string(forType: .string) == untouched, "⌥↩ leaves the clipboard alone",
                  "the clipboard holds \"\(NSPasteboard.general.string(forType: .string) ?? "")\"")
            check(overlay() != nil, "⌥↩ leaves the panel open when it can't replace")
            check(panelText().contains("Nothing replaced"), "⌥↩ says nothing was replaced", "panel text: \(panelText())")
            screenshot(window: overlay(), to: outDir + "/read-only-pressed.png")
            KeyLog.seen.removeAll()
            press(keyC, .maskCommand)
            let first = waitFor(3) { () -> String? in
                let text = NSPasteboard.general.string(forType: .string) ?? ""
                return text != untouched && !text.isEmpty ? text : nil
            }
            pump(0.5)  // for anything else that writes after the first change
            let settled = NSPasteboard.general.string(forType: .string) ?? ""
            let panelNow = overlay() != nil ? panelText() : ""
            log("NOTE ⌘C in a review: clipboard first \"\(first ?? "unchanged")\", then \"\(settled)\"; panel \(overlay() != nil ? "open" : "closed")")
            check(!KeyLog.seen.contains { $0.code == Int64(keyC) }, "⌘C in a review never reaches the app", "the app received ⌘C")
            check(!settled.isEmpty && settled != untouched && !settled.contains(typo), "⌘C copies the fix", "the clipboard holds \"\(settled)\"")
            check(panelNow.contains("Copied"), "⌘C says it copied", "panel text: \(panelNow)")
            if overlay() != nil {
                // The panel's other ⌘ shortcuts go the same way. Here ⌘R tries again, and doesn't reload the page.
                KeyLog.seen.removeAll()
                press(keyR, .maskCommand)
                pump(0.5)
                check(!KeyLog.seen.contains { $0.code == Int64(keyR) }, "⌘R in a review never reaches the app", "the app received ⌘R")
                press(keyEscape)
                _ = waitFor(3) { overlay() == nil ? true : nil }
            }
        }
    }
    quitOpened(chrome)
}
// 6c-3. A web view inside another app can hold an editable box that doesn't say it is editable: one
// with a button's role reports no settable value and no editable ancestor. The app's Cut menu item
// says the selection can be edited, so a review there replaces the text, and read-only text in the
// same web view stays copy-only. The app is built here: a window with a web view and File and Edit
// menus, whose title follows the box's text.
let webBox = work + "/WebBox.app", webBoxID = "test.supereditr.webbox"
let webBoxBuilt: Bool = {
    let source = work + "/webbox.swift"
    try! #"""
    import AppKit
    import WebKit

    let readOnly = CommandLine.arguments.contains("read-only")
    let page = """
    <!doctype html><meta charset="utf-8"><title>loading</title>
    <p id="fixed">Their going to the store tommorow.</p>
    <div id="box" contenteditable="true" role="button" style="border:1px solid; padding:6px">We meet tommorow at noon.</div>
    <script>
    const box = document.getElementById("box");
    const show = () => { document.title = "box: " + box.textContent; };
    box.addEventListener("input", show);
    const target = \(readOnly ? "document.getElementById(\"fixed\")" : "box");
    if (target === box) box.focus();
    const range = document.createRange();
    range.selectNodeContents(target);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    show();
    </script>
    """

    final class Delegate: NSObject, NSApplicationDelegate {
        var window: NSWindow!
        var web: WKWebView!
        var timer: Timer?
        func applicationDidFinishLaunching(_ note: Notification) {
            let bar = NSMenu()
            let appItem = NSMenuItem()
            appItem.submenu = NSMenu()
            appItem.submenu!.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
            bar.addItem(appItem)
            let fileItem = NSMenuItem()
            fileItem.submenu = NSMenu(title: "File")
            fileItem.submenu!.addItem(withTitle: "Close", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
            bar.addItem(fileItem)
            let editItem = NSMenuItem()
            let edit = NSMenu(title: "Edit")
            edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
            edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
            edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
            edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
            editItem.submenu = edit
            bar.addItem(editItem)
            NSApp.mainMenu = bar
            window = NSWindow(contentRect: NSRect(x: 200, y: 300, width: 560, height: 220),
                              styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
            web = WKWebView(frame: window.contentView!.bounds)
            web.autoresizingMask = [.width, .height]
            window.contentView!.addSubview(web)
            window.title = "loading"
            window.makeKeyAndOrderFront(nil)
            window.makeFirstResponder(web)
            NSApp.activate(ignoringOtherApps: true)
            web.loadHTMLString(page, baseURL: nil)
            timer = Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { [weak self] _ in
                self?.web.evaluateJavaScript("document.title") { title, _ in
                    if let title = title as? String { self?.window.title = title }
                }
            }
        }
    }

    let app = NSApplication.shared
    let delegate = Delegate()
    app.delegate = delegate
    app.setActivationPolicy(.regular)
    app.run()
    """#.write(toFile: source, atomically: true, encoding: .utf8)
    try! FileManager.default.createDirectory(atPath: webBox + "/Contents/MacOS", withIntermediateDirectories: true)
    try! """
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0"><dict>
    <key>CFBundleIdentifier</key><string>\(webBoxID)</string>
    <key>CFBundleName</key><string>WebBox</string>
    <key>CFBundleExecutable</key><string>WebBox</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    </dict></plist>
    """.write(toFile: webBox + "/Contents/Info.plist", atomically: true, encoding: .utf8)
    let build = Process()
    build.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
    build.arguments = ["swiftc", "-O", source, "-o", webBox + "/Contents/MacOS/WebBox"]
    try? build.run()
    build.waitUntilExit()
    return build.terminationStatus == 0
}()
check(webBoxBuilt, "the test builds an app with a web view")
if webBoxBuilt {
    /// The web view app's window title: "box: " and the box's text, once the page has loaded.
    func boxText() -> String { mainWindow(webBoxID).flatMap { attribute($0, kAXTitleAttribute) as? String } ?? "" }
    func lastWebBoxRead() -> String {
        let log = home + "/Library/Logs/SuperEditr/capture.log"
        let lines = (try? String(contentsOfFile: log, encoding: .utf8))?.split(separator: "\n") ?? []
        return lines.last { $0.contains("\(webBoxID): read") }.map(String.init) ?? "none"
    }
    func reviewWebBox(_ what: String) -> Bool {
        press(keyE, .maskCommand)
        pump(0.1)
        press(keyE, .maskCommand)
        let panel = waitFor(10) { overlay() }
        check(panel != nil, "⌘E E reviews \(what)", "no overlay within 10 s; last read: \(lastWebBoxRead())")
        if panel != nil { pump(1.5) }  // the rule-based checker's result
        return panel != nil
    }

    // The box, selected by the page.
    let boxOpened = openFront(webBox, bundleID: webBoxID, newInstance: true)
    check(boxOpened, "the web view app opens with its box selected")
    if boxOpened {
        check(waitFor(10) { boxText().contains("tommorow") ? true : nil } != nil, "the web view's page loads", "window title: \(boxText())")
        pump(1)
        // What this check stands on: the box must hide that it is editable, as such an app's does.
        // A web view builds its Accessibility tree once asked, so the first answers can be empty.
        if let box = waitFor(5, { focusedElement() }) {
            var settable = DarwinBoolean(false)
            AXUIElementIsAttributeSettable(box, kAXValueAttribute as CFString, &settable)
            let hidden = !settable.boolValue && attribute(box, "AXEditableAncestor") == nil
            check(hidden, "the box doesn't say it is editable",
                  "role \(attribute(box, kAXRoleAttribute) as? String ?? "none"), value settable \(settable.boolValue), editable ancestor \(attribute(box, "AXEditableAncestor") != nil)")
        } else {
            check(false, "the web view app has a focused element")
        }
        if reviewWebBox("the box that doesn't say it is editable") {
            let offered = waitFor(3) { () -> String? in
                let text = panelText()
                return text.contains("⌥↩") && !text.contains("replace text here") ? text : nil
            }
            check(offered != nil, "the review offers ⌥↩ replace there", "panel text: \(panelText()); last read: \(lastWebBoxRead())")
            screenshot(window: overlay(), to: outDir + "/web-view-box-review.png")
            press(keyReturn, .maskAlternate)
            let fixed = waitFor(5) { boxText().contains("tomorrow") ? boxText() : nil }
            check(fixed != nil, "⌥↩ replaces the text in the box", "the box holds: \(boxText())")
            log("NOTE the web view's box: \(lastWebBoxRead())")
            if overlay() != nil {
                log("NOTE after replacing in the web view's box the panel says: \(panelText())")
                press(keyEscape)
                _ = waitFor(3) { overlay() == nil ? true : nil }
            }
        }
    }
    quitOpened(webBoxID)

    // Read-only text in the same page, selected by the page.
    let fixedOpened = openFront(webBox, bundleID: webBoxID, newInstance: true, launchArguments: ["read-only"])
    check(fixedOpened, "the web view app opens with its read-only text selected")
    if fixedOpened {
        check(waitFor(10) { boxText().contains("tommorow") ? true : nil } != nil, "the web view's page loads again", "window title: \(boxText())")
        pump(1)
        if reviewWebBox("read-only text in the web view") {
            let saysWhy = waitFor(3) { () -> String? in
                let text = panelText()
                return text.contains("replace text here") ? text : nil
            }
            check(saysWhy != nil, "read-only text in the web view stays copy-only", "panel text: \(panelText()); last read: \(lastWebBoxRead())")
            press(keyEscape)
            _ = waitFor(3) { overlay() == nil ? true : nil }
        }
    }
    quitOpened(webBoxID)
}

// 6d. ↩ in a review applies the fix and never reaches the app: the panel doesn't take keyboard
// focus, and SuperEditr's keyboard tap takes ↩ from the app instead. In a chat app, ↩ reaching the
// app would send the message. The second time, the instruction box (/) takes focus first and esc
// closes it, which must give the keys back to the review.
let keySlash: CGKeyCode = 44
let enterTypo = "Their going to love this."
for (index, how) in ["directly", "after closing the instruction box"].enumerated() {
    let enterDraft = work + "/enter-\(index).txt"
    try! enterTypo.write(toFile: enterDraft, atomically: true, encoding: .utf8)
    check(openFront("TextEdit", bundleID: "com.apple.TextEdit", file: enterDraft), "TextEdit opens a draft for ↩ \(how)")
    check(selectAll(), "the draft for ↩ \(how) is selected")
    press(keyE, .maskCommand)
    pump(0.1)
    press(keyE, .maskCommand)
    let enterPanel = waitFor(5) { overlay() }
    check(enterPanel != nil, "⌘E E reviews the draft for ↩ \(how)", "no overlay within 5 s")
    guard enterPanel != nil else { quitOpened("com.apple.TextEdit"); continue }
    pump(1.5)  // the rule-based checker's result
    if index == 1 {
        press(keySlash)
        pump(0.5)
        screenshot(window: overlay(), to: outDir + "/instruct.png")
        press(keyEscape)
        pump(0.5)
        check(overlay() != nil, "esc closes the instruction box, not the review", "the panel closed")
    }
    KeyLog.seen.removeAll()
    press(keyReturn)
    check(waitFor(3) { overlay() == nil ? true : nil } != nil, "↩ \(how) applies and closes the review", "panel still showing")
    pump(0.5)
    let value = focusedElement().map { attribute($0, kAXValueAttribute) as? String ?? "" } ?? ""
    check(!KeyLog.seen.contains { $0.code == Int64(keyReturn) }, "↩ \(how) never reaches TextEdit", "TextEdit received ↩")
    check(!value.contains("\n"), "↩ \(how) adds no new line", "TextEdit reads \"\(value)\"")
    check(!value.isEmpty && !value.contains("Their going"), "↩ \(how) writes the fix", "TextEdit reads \"\(value)\"")
    check(NSWorkspace.shared.frontmostApplication?.bundleIdentifier == "com.apple.TextEdit", "TextEdit stays in front",
          "front: \(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "none")")
    quitOpened("com.apple.TextEdit")
}

// 6e. A proofread with two changes is applied in place in TextEdit: two Accessibility writes, each
// replacing only the word it fixes, and the text around them as it was.
let twoTypos = work + "/two-typos.txt"
try! "Their going to love this, it is realy fast.".write(toFile: twoTypos, atomically: true, encoding: .utf8)
check(openFront("TextEdit", bundleID: "com.apple.TextEdit", file: twoTypos), "TextEdit opens the draft with two typos")
check(selectAll(), "the draft with two typos is selected")
press(keyE, .maskCommand)
pump(0.1)
press(keyE, .maskCommand)
let twoPanel = waitFor(5) { overlay() }
check(twoPanel != nil, "⌘E E reviews the draft with two typos", "no overlay within 5 s")
if twoPanel != nil {
    _ = waitFor(4) { panelText().range(of: #"\b\d+ changes?\b"#, options: .regularExpression) != nil ? true : nil }
    press(keyReturn)
    _ = waitFor(3) { overlay() == nil ? true : nil }
    let fixed = "They're going to love this, it is really fast."
    let value = waitFor(3) { () -> String? in
        let value = focusedElement().flatMap { attribute($0, kAXValueAttribute) as? String } ?? ""
        return value == fixed ? value : nil
    }
    check(value != nil, "both fixes land in TextEdit", "TextEdit reads \(String(reflecting: focusedElement().flatMap { attribute($0, kAXValueAttribute) as? String } ?? ""))")
    let path = appliedPath("com.apple.TextEdit")
    check(path.contains("applied in place: 2 edits"), "TextEdit applies the proofread in place", "the log says: \(path)")
}
quitOpened("com.apple.TextEdit")

// 6f. With nothing selected, ⌘E E selects the sentence before the caret, visibly, and reviews it.
let caretDraft = work + "/caret.txt"
try! "We met at noon. Their going to love this, it work good.".write(toFile: caretDraft, atomically: true, encoding: .utf8)
check(openFront("TextEdit", bundleID: "com.apple.TextEdit", file: caretDraft), "TextEdit opens the caret draft")
// ⌘↓ puts the caret at the end; a freshly opened window may not take keys straight away.
let atEnd = waitFor(4) { () -> Bool? in
    press(keyDown, .maskCommand)
    pump(0.3)
    guard let element = focusedElement(), let value = attribute(element, kAXSelectedTextRangeAttribute) else { return nil }
    var range = CFRange()
    guard AXValueGetValue(value as! AXValue, .cfRange, &range), range.length == 0, range.location > 0 else { return nil }
    return true
}
check(atEnd != nil, "the caret sits at the end of the caret draft with nothing selected")
press(keyE, .maskCommand)
pump(0.1)
press(keyE, .maskCommand)
let sentencePanel = waitFor(5) { overlay() }
check(sentencePanel != nil, "⌘E E with nothing selected opens a review", "no overlay within 5 s. SuperEditr windows: \(describeWindows())")
let selectedSentence = focusedElement().flatMap { attribute($0, kAXSelectedTextAttribute) as? String }
check(selectedSentence == "Their going to love this, it work good.", "⌘E E selects the sentence before the caret",
      "TextEdit's selection: \(selectedSentence.map { "\"\($0)\"" } ?? "none")")
if sentencePanel != nil {
    pump(1.5)  // the rule-based checker's result
    screenshot(window: overlay(), to: outDir + "/sentence-before-caret.png")
    let reviewed = panelText()
    check(reviewed.contains("love this") && !reviewed.contains("We met at noon"), "the review covers only that sentence",
          "panel text: \(reviewed)")
    press(keyEscape)
}
quitOpened("com.apple.TextEdit")

// 6g. With no AI, Settings says what that means and offers to set one up: Get Started's badge, and
// a line on Styles, where only Proofread works. Apple Intelligence's helper is missing here, so
// nothing built in can review either, and neither page offers to turn it on.
let noAiLaunch = launch(appPath, home: home)  // a second launch opens Settings in the running one
_ = waitFor(5) { noAiLaunch.isRunning ? nil : true }
if let settings = waitFor(8, { settingsWindow() }) {
    running(superEditr).first?.activate()
    func open(_ page: String) {
        if let item = waitFor(5, { button(in: settings, containing: page) }) { AXUIElementPerformAction(item, kAXPressAction as CFString) }
    }
    func showsModels() -> Bool { waitFor(5) { button(in: settings, containing: "Scan") == nil ? nil : true } != nil }
    open("Get Started")
    let badge = waitFor(5) { () -> AXUIElement? in
        allText(settings, levels: 40).contains("No AI set up") ? button(in: settings, containing: "Set up AI") : nil
    }
    check(badge != nil, "with no AI, Get Started says so and offers Set up AI", "page text: \(allText(settings, levels: 40))")
    screenshot(window: nil, rect: frame(of: settings), to: outDir + "/no-ai-get-started.png")
    if let badge {
        AXUIElementPerformAction(badge, kAXPressAction as CFString)
        check(showsModels(), "Get Started's Set up AI opens Models", "no scan button on the page")
    }
    open("Styles")
    let notice = waitFor(5) { () -> AXUIElement? in
        allText(settings, levels: 40).contains("Styles other than Proofread need an AI") ? button(in: settings, containing: "Set up AI") : nil
    }
    check(notice != nil, "with no AI, Styles says only Proofread works and offers Set up AI", "page text: \(allText(settings, levels: 40))")
    check(button(in: settings, containing: "Turn on Apple Intelligence") == nil, "Styles offers Apple Intelligence only where it can run")
    screenshot(window: nil, rect: frame(of: settings), to: outDir + "/no-ai-styles.png")
    if let notice {
        AXUIElementPerformAction(notice, kAXPressAction as CFString)
        check(showsModels(), "Styles' Set up AI opens Models", "no scan button on the page")
    }
} else {
    check(false, "a second launch opens Settings for the no-AI checks", "SuperEditr windows: \(describeWindows())")
}
stop(app)
// What SuperEditr logged about each read and write (app IDs, timings and lengths, no text).
try? FileManager.default.copyItem(atPath: home + "/Library/Logs/SuperEditr/capture.log", toPath: outDir + "/capture.log")

// 7. The Accessibility guide card, when this build has it, sits beside System Settings' window, off
// the list; with no room on the screen, over the window's top.
let binary = (try? Data(contentsOf: URL(fileURLWithPath: "\(appPath)/Contents/MacOS/supereditr"))) ?? Data()
if binary.range(of: Data("SUPEREDITR_PREVIEW_GUIDE".utf8)) == nil {
    log("SKIP guide card: not in this build")
} else if !running("com.apple.systempreferences").isEmpty {
    log("SKIP guide card: System Settings is already open on this Mac")
} else {
    app = launch(appPath, home: home, environment: ["SUPEREDITR_PREVIEW_GUIDE": "1"])
    let settingsApp = waitFor(10) { running("com.apple.systempreferences").first }
    if settingsApp != nil { opened.insert("com.apple.systempreferences") }
    let settingsWindow = settingsApp == nil ? nil : waitFor(10) { windows(bundleID: "com.apple.systempreferences").first { $0.frame.height > 200 } }
    let card = waitFor(10) { guideCard() }
    check(settingsWindow != nil && card != nil, "the guide card appears with System Settings",
          "System Settings window: \(String(describing: settingsWindow?.frame)), card: \(String(describing: card?.frame)); "
            + "window server: \(windowServerView("com.apple.systempreferences"))")
    if var s = settingsWindow?.frame, var c = card?.frame {
        // Beside the window, the guide's whole window, shadow margin included, stays off the list.
        func placed() -> Bool {
            let card = c.insetBy(dx: 28, dy: 28)
            let beside = !c.intersects(s.insetBy(dx: 1, dy: 1)) && card.intersects(s.insetBy(dx: -60, dy: -60))
            let top = s.contains(card) && card.minY - s.minY < 60
            return beside || top
        }
        // The guide moves the card a moment after a window's frame changes, so a first look can catch
        // it on its way. Both frames are read again until it arrives.
        _ = waitFor(3) { () -> Bool? in
            s = windows(bundleID: "com.apple.systempreferences").first { $0.frame.height > 200 }?.frame ?? s
            c = guideCard()?.frame ?? c
            return placed() ? true : nil
        }
        check(placed(), "the guide card sits beside System Settings, off the list", "card \(c.insetBy(dx: 28, dy: 28)) by window \(s)")
        screenshot(window: nil, rect: s.union(c), to: outDir + "/guide.png")
    }
    stop(app)
    quitOpened("com.apple.systempreferences")
}

// 8. Check for Updates. A test build has no updater of its own, so a copy of this one gets the real
// updater with a feed served from here and a key made for this run. The update is the app itself,
// so its files change while its version doesn't.

/// A throwaway updater key, in the minisign format the updater reads. Signatures use minisign's
/// legacy mode ("Ed"), which signs a file's bytes rather than a hash of them.
struct UpdateKey {
    private let key = Curve25519.Signing.PrivateKey()
    private let id = Data((0..<8).map { _ in UInt8.random(in: .min ... .max) })

    /// What the updater takes as `pubkey`: a minisign public key file, in base64.
    var publicKey: String {
        let line = (Data("Ed".utf8) + id + key.publicKey.rawRepresentation).base64EncodedString()
        return Data("untrusted comment: minisign public key\n\(line)\n".utf8).base64EncodedString()
    }

    /// What latest.json's `signature` holds for `file`: a minisign signature file, in base64. The
    /// updater also requires the version, which the release tools write into the trusted comment.
    func signature(of file: Data, version: String) -> String {
        let signature = try! key.signature(for: file)
        let comment = "timestamp:\(Int(Date().timeIntervalSince1970))\tfile:update.tar.gz\tversion:\(version)"
        let global = try! key.signature(for: signature + Data(comment.utf8))
        let line = (Data("Ed".utf8) + id + signature).base64EncodedString()
        return Data("untrusted comment: signature\n\(line)\ntrusted comment: \(comment)\n\(global.base64EncodedString())\n".utf8).base64EncodedString()
    }
}

/// The update feed and download, on this Mac only. `pause` holds the feed's answer for that long,
/// and the download halfway for that long, so the window's in-between states can be seen.
final class UpdateFeed {
    enum Answer { case nothingNew, failure, offer(version: String, signature: String) }

    private let queue = DispatchQueue(label: "update-feed")
    private let archive: Data
    private var listener: NWListener?
    private var answer = Answer.nothingNew
    private var pause = 0.0

    init(archive: Data) { self.archive = archive }

    /// Starts listening and returns the port, or nil if it can't.
    func start() -> UInt16? {
        let parameters = NWParameters.tcp
        parameters.requiredInterfaceType = .loopback
        guard let listener = try? NWListener(using: parameters, on: .any) else { return nil }
        self.listener = listener
        listener.newConnectionHandler = { [weak self] connection in
            guard let self else { return }
            connection.start(queue: self.queue)
            connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { data, _, _, _ in
                let request = String(decoding: data ?? Data(), as: UTF8.self)
                self.respond(on: connection, path: request.split(separator: " ").dropFirst().first.map(String.init) ?? "")
            }
        }
        listener.start(queue: queue)
        // The port reads 0 until the listener is ready.
        return waitFor(5) { listener.port.flatMap { $0.rawValue == 0 ? nil : $0.rawValue } }
    }

    func stop() { listener?.cancel() }

    func next(_ answer: Answer, pause: Double = 0) {
        queue.sync {
            self.answer = answer
            self.pause = pause
        }
    }

    private func respond(on connection: NWConnection, path: String) {
        if path == "/update.tar.gz" {
            let half = archive.count / 2
            let head = Data("HTTP/1.1 200 OK\r\nContent-Length: \(archive.count)\r\nConnection: close\r\n\r\n".utf8)
            connection.send(content: head + archive.prefix(half), completion: .contentProcessed { _ in })
            queue.asyncAfter(deadline: .now() + pause) { [archive] in
                self.finish(connection, Data(archive.dropFirst(half)))
            }
            return
        }
        queue.asyncAfter(deadline: .now() + pause) {
            switch self.answer {
            case .nothingNew: self.finish(connection, Data("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".utf8))
            case .failure: self.finish(connection, Data("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".utf8))
            case let .offer(version, signature):
                let port = self.listener?.port?.rawValue ?? 0
                let download = ["url": "http://127.0.0.1:\(port)/update.tar.gz", "signature": signature]
                let release: [String: Any] = [
                    "version": version,
                    "pub_date": "2026-01-01T00:00:00Z",
                    "platforms": ["darwin-aarch64": download, "darwin-aarch64-app": download, "darwin-x86_64": download, "darwin-x86_64-app": download],
                ]
                let json = try! JSONSerialization.data(withJSONObject: release)
                self.finish(connection, Data("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \(json.count)\r\nConnection: close\r\n\r\n".utf8) + json)
            }
        }
    }

    private func finish(_ connection: NWConnection, _ data: Data) {
        connection.send(content: data, contentContext: .finalMessage, isComplete: true, completion: .contentProcessed { _ in connection.cancel() })
    }
}

/// SuperEditr restarts through LaunchServices, which doesn't pass on this test's environment, so the
/// new copy runs with the real HOME and its settings. Only a disposable VM can take that.
func inVirtualMachine() -> Bool {
    var present: Int32 = 0
    var size = MemoryLayout<Int32>.size
    return sysctlbyname("kern.hv_vmm_present", &present, &size, nil, 0) == 0 && present == 1
}

func runTool(_ path: String, _ arguments: [String], environment: [String: String] = [:]) -> Bool {
    let tool = Process()
    tool.executableURL = URL(fileURLWithPath: path)
    tool.arguments = arguments
    tool.environment = ProcessInfo.processInfo.environment.merging(environment) { $1 }
    guard (try? tool.run()) != nil else { return false }
    tool.waitUntilExit()
    return tool.terminationStatus == 0
}

func updateWindow() -> AXUIElement? {
    guard let app = running(superEditr).first,
          let list = attribute(AXUIElementCreateApplication(app.processIdentifier), kAXWindowsAttribute) as? [AXUIElement] else { return nil }
    return list.first { attribute($0, kAXTitleAttribute) as? String == "SuperEditr Updates" }
}
func updateText() -> String { updateWindow().map { allText($0, levels: 40) } ?? "" }
func updateSays(_ words: String, within seconds: Double = 20) -> Bool {
    waitFor(seconds) { updateText().contains(words) ? true : nil } ?? false
}
/// The window's progress bar, when it's there and tall enough to see.
func updateBar() -> AXUIElement? {
    guard let window = updateWindow(),
          let bar = findElement(window, { (attribute($0, kAXRoleAttribute) as? String) == kAXProgressIndicatorRole }) else { return nil }
    return frame(of: bar).height >= 4 ? bar : nil
}
func updateWindowClosed() -> Bool { waitFor(5) { updateWindow() == nil ? true : nil } ?? false }
func screenshotUpdate(_ name: String) {
    if let window = updateWindow() { screenshot(window: Window(frame: frame(of: window)), to: outDir + "/update-\(name).png") }
}
func pressUpdateButton(_ label: String) {
    if let window = updateWindow(), let found = button(in: window, containing: label) {
        AXUIElementPerformAction(found, kAXPressAction as CFString)
    } else {
        log("     no \(label) button in: \(updateText())")
    }
}

// In the home folder: Tauri refuses to update an app whose path has a symlink, and the temporary
// folder is under /var, which is one. Its real path doesn't help, because Process launches
// /private/var/... as /var/...
let updateRoot = NSHomeDirectory() + "/.supereditr-smoke-\(getpid())"
cleanups.append { try? FileManager.default.removeItem(atPath: updateRoot) }
let updateApp = updateRoot + "/SuperEditr.app", updateArchive = updateRoot + "/update.tar.gz"
/// Changes when an update replaces the app's files.
func updateAppFile() -> Int? {
    (try? FileManager.default.attributesOfItem(atPath: updateApp + "/Contents/Info.plist"))?[.systemFileNumber] as? Int
}
try! FileManager.default.createDirectory(atPath: updateRoot, withIntermediateDirectories: true)
let updatePrepared = runTool("/usr/bin/ditto", [appPath, updateApp])
    && runTool("/usr/bin/tar", ["-czf", updateArchive, "-C", updateRoot, "SuperEditr.app"], environment: ["COPYFILE_DISABLE": "1"])
let updateBytes = (try? Data(contentsOf: URL(fileURLWithPath: updateArchive))) ?? Data()
let updateFeed = UpdateFeed(archive: updateBytes)
let updatePort = updateFeed.start()
check(updatePrepared && !updateBytes.isEmpty && updatePort != nil, "the update test has a copy of the app, an archive of it and a feed",
      "prepared \(updatePrepared), \(updateBytes.count) bytes, port \(String(describing: updatePort))")
cleanups.append { updateFeed.stop() }
cleanups.append { quitOpened(superEditr) }
if let updatePort {
    let updateKey = UpdateKey(), newVersion = "99.0.0"
    let installedVersion = Bundle(path: appPath)?.infoDictionary?["CFBundleShortVersionString"] as? String ?? "unknown"
    let updateEnvironment = ["SUPEREDITR_TEST_UPDATE_FEED": "http://127.0.0.1:\(updatePort)/latest.json", "SUPEREDITR_TEST_UPDATE_KEY": updateKey.publicKey]
    let goodOffer = UpdateFeed.Answer.offer(version: newVersion, signature: updateKey.signature(of: updateBytes, version: newVersion))
    /// Waits for a copy just started to have its menu-bar icon, which the checks below click.
    func waitForMenuIcon(_ copy: pid_t) {
        _ = waitFor(15) {
            attribute(AXUIElementCreateApplication(copy), "AXExtrasMenuBar").flatMap { attribute($0 as! AXUIElement, kAXChildrenAttribute) as? [AXUIElement] }?.isEmpty == false ? true : nil
        }
        pump(1)
    }
    /// Waits for the restart: the old copy gone, and a new one running from the same place.
    func restarted(from old: pid_t) -> Bool {
        let wanted = URL(fileURLWithPath: updateApp).resolvingSymlinksInPath().path
        return waitFor(20) {
            running(superEditr).contains { $0.processIdentifier != old && $0.bundleURL?.resolvingSymlinksInPath().path == wanted } && !running(superEditr).contains { $0.processIdentifier == old } ? true : nil
        } ?? false
    }

    app = launch(updateApp, home: home, environment: updateEnvironment)
    let firstCopy = app.processIdentifier
    waitForMenuIcon(firstCopy)

    // Nothing new, answered slowly: the window opens straight away, then reports the result.
    updateFeed.next(.nothingNew, pause: 4)
    let chosen = chooseFromMenu("Check for Updates", screenshotTo: outDir + "/update-menu.png")
    check(chosen != nil, "the menu-bar menu offers Check for Updates when there's an updater", "item: \(chosen ?? "none")")
    check(updateSays("Checking for updates", within: 3), "choosing it opens the update window at once, saying it's checking", "window: \(updateText())")
    check(updateBar() != nil, "and a bar shows it's working", dumpTree(updateWindow() ?? AXUIElementCreateSystemWide()))
    screenshotUpdate("checking")
    check(updateSays("You’re up to date") && updateText().contains(installedVersion), "the window says SuperEditr is up to date, and which version it has", "window: \(updateText())")
    screenshotUpdate("up-to-date")
    press(keyReturn)
    check(updateWindowClosed(), "Return closes it, so it has the keyboard", "window: \(updateText())")

    // The feed fails, then works when asked again.
    updateFeed.next(.failure)
    _ = chooseFromMenu("Check for Updates")
    check(updateSays("Couldn’t check for updates"), "a feed that fails is reported in the window", "window: \(updateText())")
    screenshotUpdate("check-failed")
    updateFeed.next(.nothingNew)
    pressUpdateButton("Try Again")
    check(updateSays("You’re up to date"), "Try Again checks again", "window: \(updateText())")
    press(keyEscape)
    check(updateWindowClosed(), "Esc closes it", "window: \(updateText())")

    // A download signed with another key is refused, and the app stays as it was.
    let fileBefore = updateAppFile()
    updateFeed.next(.offer(version: newVersion, signature: UpdateKey().signature(of: updateBytes, version: newVersion)))
    _ = chooseFromMenu("Check for Updates")
    check(updateSays("Couldn’t install SuperEditr \(newVersion)"), "a download with a bad signature is reported in the window", "window: \(updateText())")
    screenshotUpdate("install-failed")
    check(fileBefore != nil && updateAppFile() == fileBefore, "and the app's files are left alone")
    pressUpdateButton("Close")
    check(updateWindowClosed(), "Close closes it", "window: \(updateText())")

    // A real update, chosen from the menu that now says it failed. It installs, Later keeps working,
    // and the menu offers the restart.
    updateFeed.next(goodOffer, pause: 3)
    let retried = chooseFromMenu("Update Failed")
    check(retried != nil, "the menu offers another try after a failed update", "item: \(retried ?? "none")")
    check(updateSays("Downloading SuperEditr \(newVersion)"), "the window shows the download", "window: \(updateText())")
    let arrived = waitFor(2.5) { updateBar().flatMap { (attribute($0, kAXValueAttribute) as? NSNumber)?.doubleValue }.flatMap { $0 > 0 ? $0 : nil } }
    check(arrived != nil, "a bar shows how much has arrived", dumpTree(updateWindow() ?? AXUIElementCreateSystemWide()))
    screenshotUpdate("downloading")
    check(updateSays("SuperEditr \(newVersion) is ready"), "the window says when the update is ready", "window: \(updateText())")
    screenshotUpdate("ready")
    check(updateAppFile() != fileBefore, "the update replaced the app's files")
    pressUpdateButton("Later")
    check(updateWindowClosed() && app.isRunning, "Later closes the window and SuperEditr keeps running", "window: \(updateText())")
    if inVirtualMachine() {
        let offered = chooseFromMenu("Restart to Update to \(newVersion)", screenshotTo: outDir + "/update-menu-restart.png")
        check(offered != nil, "the menu offers the restart", "item: \(offered ?? "none")")
        check(restarted(from: firstCopy), "choosing it restarts SuperEditr from the same place")
        quitOpened(superEditr)
        stop(app)

        // Closing the window doesn't stop a download, the menu shows it again, and Restart Now works.
        app = launch(updateApp, home: home, environment: updateEnvironment)
        let secondCopy = app.processIdentifier
        waitForMenuIcon(secondCopy)
        updateFeed.next(goodOffer, pause: 6)
        _ = chooseFromMenu("Check for Updates")
        check(updateSays("Downloading SuperEditr \(newVersion)"), "a second update starts downloading", "window: \(updateText())")
        press(keyEscape)
        check(updateWindowClosed(), "the window can close during a download", "window: \(updateText())")
        let reopened = chooseFromMenu("Downloading SuperEditr")
        check(reopened != nil, "the menu shows the download and reopens the window", "item: \(reopened ?? "none")")
        check(updateSays("SuperEditr \(newVersion) is ready"), "the download finished with the window closed", "window: \(updateText())")
        pressUpdateButton("Restart Now")
        check(restarted(from: secondCopy), "Restart Now restarts SuperEditr from the same place")
    } else {
        log("SKIP restarting after an update: only in a virtual machine, because it runs with this Mac's real settings")
    }
    quitOpened(superEditr)
    stop(app)
}

log(failures.isEmpty ? "All UI smoke tests passed." : "\(failures.count) UI smoke test(s) failed: \(failures.joined(separator: "; "))")
finish(failures.isEmpty ? 0 : 1)
