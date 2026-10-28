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
import Foundation

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
func skipEnvironment(_ reason: String) -> Never {
    log("SKIP \(reason)")
    finish(78)
}

// MARK: Events

let replayMark: Int64 = 0x5345_4454  // SuperEditr's marker on keys it replays ("SEDT")
let keyE: CGKeyCode = 14, keyA: CGKeyCode = 0, keyEscape: CGKeyCode = 53, keyTab: CGKeyCode = 48, keySpace: CGKeyCode = 49
let keyShift: CGKeyCode = 56, keyRight: CGKeyCode = 124, keyUp: CGKeyCode = 126, keyReturn: CGKeyCode = 36, keyV: CGKeyCode = 9

/// Holds Shift down (true) or lets it go (false), as a separate key, the way a person does.
func shift(_ down: Bool) {
    guard let event = CGEvent(keyboardEventSource: nil, virtualKey: keyShift, keyDown: down) else { return }
    event.flags = down ? .maskShift : []
    event.post(tap: .cghidEventTap)
    usleep(25_000)
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

let superEditr = "com.sghael.supereditr"
/// SuperEditr’s suggestion panel: about 548 points wide (Settings is 900, the guide card 380).
func overlay() -> Window? { windows(bundleID: superEditr).first { (400...700).contains($0.frame.width) } }
/// The permission guide card, about 380 points wide.
func guideCard() -> Window? { windows(bundleID: superEditr).first { $0.frame.width < 400 } }

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

/// Apps this test started, the only ones it ever quits.
var opened: [NSRunningApplication] = []

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
    let started = waitFor(5) { () -> [NSRunningApplication]? in
        let new = running(bundleID).filter { !before.contains($0.processIdentifier) }
        return new.isEmpty ? nil : new
    } ?? []
    opened += started.filter { new in !opened.contains { $0.processIdentifier == new.processIdentifier } }
    // Success means the test's own process is in front, never a copy that was already open.
    let ours = Set(started.map(\.processIdentifier))
    guard !ours.isEmpty else { return false }
    return waitFor(8) { ours.contains(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1) ? true : nil } ?? false
}

/// Quits the instances of an app that this test started; anything else is left alone.
func quitOpened(_ bundleID: String) {
    for app in opened where app.bundleIdentifier == bundleID { app.forceTerminate() }
    opened.removeAll { $0.bundleIdentifier == bundleID }
    pump(0.5)
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

func button(in root: AXUIElement, containing label: String) -> AXUIElement? {
    findElement(root) { (attribute($0, kAXRoleAttribute) as? String) == kAXButtonRole && allText($0).contains(label) }
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
if !running("com.sghael.supereditr").isEmpty { skipEnvironment("another SuperEditr is running") }
// The test opens and force-quits these; never touch someone's open documents.
for (name, id) in [("TextEdit", "com.apple.TextEdit"), ("Terminal", "com.apple.Terminal")] where !running(id).isEmpty {
    skipEnvironment("\(name) is already open on this Mac")
}
guard KeyLog.start() else { skipEnvironment("couldn't watch key events") }

let work = NSTemporaryDirectory() + "supereditr-smoke-\(getpid())"
let home = work + "/home"
try? FileManager.default.createDirectory(atPath: home, withIntermediateDirectories: true)
cleanups.append { try? FileManager.default.removeItem(atPath: work) }
let support = home + "/Library/Application Support/com.sghael.supereditr"
let status = support + "/status.json"
let draft = work + "/draft.txt"
try! "Their going to love this, it work good and its realy fast.".write(toFile: draft, atomically: true, encoding: .utf8)
// Onboarded, so Settings doesn't open over the test, and Terminal allowed, so check 4 exercises
// the read-only text rule rather than the excluded-apps list.
try! FileManager.default.createDirectory(atPath: support, withIntermediateDirectories: true)
try! #"{"onboarded": true, "excluded_apps": ["com.1password.1password"]}"#.write(toFile: support + "/settings.json", atomically: true, encoding: .utf8)

FileManager.default.createFile(atPath: outDir + "/supereditr.log", contents: nil)
appLog = FileHandle(forWritingAtPath: outDir + "/supereditr.log") ?? FileHandle.nullDevice
var app = launch(appPath, home: home)
cleanups.append { stop(app) }
cleanups.append { for app in opened { app.forceTerminate() } }
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
    // The panel opens at the selection: inside the TextEdit window, not elsewhere on screen.
    if let editor = windows(bundleID: "com.apple.TextEdit").first?.frame {
        let near = automatic.frame.minX >= editor.minX - 1 && automatic.frame.minX < editor.maxX
            && automatic.frame.minY >= editor.minY && automatic.frame.minY < editor.maxY
        check(near, "the panel opens at the selection", "panel \(automatic.frame), TextEdit window \(editor)")
    }
}
press(keyEscape)  // any key in the app dismisses an automatic suggestion
check(waitFor(3) { overlay() == nil ? true : nil } != nil, "typing elsewhere dismisses it", "overlay still showing")

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
    let general = waitFor(5) { button(in: settings, containing: "Behavior") }
    check(general != nil, "Settings shows its sidebar")
    if let general { AXUIElementPerformAction(general, kAXPressAction as CFString) }
    let recorder = waitFor(5) { button(in: settings, containing: "⌘E E") }
    check(recorder != nil, "Settings → Behavior shows the review shortcut")
    if let recorder {
        AXUIElementPerformAction(recorder, kAXPressAction as CFString)
        pump(0.5)
        press(keyE, [.maskControl, .maskAlternate, .maskShift])
        let explained = waitFor(4) { allText(settings, levels: 40).contains("already uses this for Quick fix") ? true : nil }
        check(explained != nil, "recording Quick fix's shortcut explains the conflict", "no explanation shown")
    }
    screenshot(window: nil, rect: frame(of: settings), to: outDir + "/shortcut-recorder.png")

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
        screenshot(window: nil, rect: frame(of: settings), to: outDir + "/find-accounts.png")
    }
    if general == nil || recorder == nil {
        try? dumpTree(settings).write(toFile: outDir + "/settings-accessibility.txt", atomically: true, encoding: .utf8)
        log("     Settings' Accessibility tree: \(outDir)/settings-accessibility.txt")
    }
}
stop(app)

// 6. Always keep: rejecting the rule-based checker's guess at a name ("Rhyan" → "Ryan") offers to add it
// to the word list, and A adds it. No Apple Intelligence (its helper path doesn't exist), so the
// rule-based checker answers on every Mac. Automatic suggestions are off: one would open without focus
// and race ⌘E E, and the keys below must reach the panel.
let named = work + "/named.txt"
try! "Rhyan has tennis lessons today.".write(toFile: named, atomically: true, encoding: .utf8)
try! #"{"onboarded": true, "excluded_apps": ["com.1password.1password"], "auto_popup": false}"#
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
    press(keyEscape)
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
// the paste path rather than an Accessibility write. Checked in a plain text box and in an editable
// div, the kind of field chat apps use. Chrome runs as its own copy with a throwaway profile.
let chromeApp = "/Applications/Google Chrome.app", chrome = "com.google.Chrome"
if !FileManager.default.fileExists(atPath: chromeApp) {
    log("SKIP applying in a web page: Chrome isn't installed")
} else {
    let typo = "Their going to love this."
    let page = work + "/fields.html"
    try! """
    <!doctype html><meta charset="utf-8"><title>Fields</title>
    <textarea id="control" autofocus rows="2" cols="60">Control field.</textarea>
    <textarea id="plain" rows="3" cols="60">\(typo)</textarea>
    <div id="rich" contenteditable="true" style="border:1px solid; width:30em; min-height:3em">\(typo)</div>
    """.write(toFile: page, atomically: true, encoding: .utf8)
    let chromeOpened = openFront(chromeApp, bundleID: chrome, file: page, newInstance: true,
                                 launchArguments: ["--user-data-dir=\(work)/chrome", "--no-first-run", "--no-default-browser-check"])
    check(chromeOpened, "Chrome opens the page with two fields")
    // Keys go only to the test's own Chrome; stop here rather than type into another one.
    if chromeOpened {
        // Chrome builds its Accessibility tree only once asked, as SuperEditr does when it reads. Asking
        // first, and waiting for the tree, makes SuperEditr read the field through Accessibility, as it
        // does in an app whose tree is already on, instead of falling back to a copy-only ⌘C read.
        if let ours = opened.last(where: { $0.bundleIdentifier == chrome }) {
            let element = AXUIElementCreateApplication(ours.processIdentifier)
            for flag in ["AXManualAccessibility", "AXEnhancedUserInterface"] {
                AXUIElementSetAttributeValue(element, flag as CFString, kCFBooleanTrue)
            }
        }
        pump(2)
        let captureLog = home + "/Library/Logs/SuperEditr/capture.log"
        func lastChromeRead() -> String {
            let lines = (try? String(contentsOfFile: captureLog, encoding: .utf8))?.split(separator: "\n") ?? []
            return lines.last { $0.contains("com.google.Chrome: read") }.map(String.init) ?? "none"
        }
        // SuperEditr's first read in a new Chrome process can come before the tree is ready and fall back
        // to a copy-only ⌘C read. One review closed straight away gets past it.
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
            guard let ours = opened.last(where: { $0.bundleIdentifier == chrome }),
                  let window = attribute(AXUIElementCreateApplication(ours.processIdentifier), kAXFocusedWindowAttribute) else { return "no window" }
            return "main \(attribute(window as! AXUIElement, kAXMainAttribute) as? Bool ?? false), focused \(attribute(window as! AXUIElement, kAXFocusedAttribute) as? Bool ?? false)"
        }
        log("NOTE control: Chrome's window before any paste: \(chromeWindowFocused())")
        for (how, toProcess) in [("a ⌘V keystroke", false), ("⌘V posted to Chrome's process", true)] {
            let marker = "Pasted by the test \(toProcess ? 2 : 1)."
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(marker, forType: .string)
            _ = selectAll()
            if toProcess, let ours = opened.last(where: { $0.bundleIdentifier == chrome }) {
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
        for (index, field) in ["text box", "editable div"].enumerated() {
            press(keyTab)  // from the control field to the text box, then to the div
            pump(0.5)
            check(selectAll() || selectAll(), "the \(field)'s text is selected")
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
}
    quitOpened(chrome)
}
stop(app)
// What SuperEditr logged about each read and write (app IDs, timings and lengths, no text).
try? FileManager.default.copyItem(atPath: home + "/Library/Logs/SuperEditr/capture.log", toPath: outDir + "/capture.log")

// 7. The Accessibility guide card, when this build has it, sits at System Settings' bottom-right.
let binary = (try? Data(contentsOf: URL(fileURLWithPath: "\(appPath)/Contents/MacOS/supereditr"))) ?? Data()
if binary.range(of: Data("SUPEREDITR_PREVIEW_GUIDE".utf8)) == nil {
    log("SKIP guide card: not in this build")
} else if !running("com.apple.systempreferences").isEmpty {
    log("SKIP guide card: System Settings is already open on this Mac")
} else {
    app = launch(appPath, home: home, environment: ["SUPEREDITR_PREVIEW_GUIDE": "1"])
    let settingsApp = waitFor(10) { running("com.apple.systempreferences").first }
    if let settingsApp { opened.append(settingsApp) }
    let settingsWindow = settingsApp == nil ? nil : waitFor(10) { windows(bundleID: "com.apple.systempreferences").first { $0.frame.height > 200 } }
    let card = waitFor(10) { guideCard() }
    check(settingsWindow != nil && card != nil, "the guide card appears with System Settings",
          "System Settings window: \(String(describing: settingsWindow?.frame)), card: \(String(describing: card?.frame))")
    if let s = settingsWindow?.frame, let c = card?.frame {
        let inside = c.maxX <= s.maxX + 1 && c.maxY <= s.maxY + 1 && c.minX >= s.minX && c.minY >= s.minY
        let corner = s.maxX - c.maxX < 60 && s.maxY - c.maxY < 60
        check(inside && corner, "the guide card sits at System Settings' bottom-right", "card \(c) in window \(s)")
        screenshot(window: nil, rect: s.union(c), to: outDir + "/guide.png")
    }
    stop(app)
    quitOpened("com.apple.systempreferences")
}

log(failures.isEmpty ? "All UI smoke tests passed." : "\(failures.count) UI smoke test(s) failed: \(failures.joined(separator: "; "))")
finish(failures.isEmpty ? 0 : 1)
