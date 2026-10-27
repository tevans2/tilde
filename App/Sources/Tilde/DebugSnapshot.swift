#if DEBUG
import AppKit
import ImageIO
import Quartz
import SwiftTerm
import UniformTypeIdentifiers

/// Development aid: `TILDE_SNAPSHOT=/path/out.png` renders the key window to a PNG after
/// launch and quits. `TILDE_SNAPSHOT_KEYS` replays keystrokes first (`\e` is escape, `\t` tab,
/// `\r` return, `\b` delete, `^j` control-j, `@s` command-s, `\w` a one-second pause), so
/// flows can be checked without screen-recording access.
/// `TILDE_SNAPSHOT_INDEX=1` opens the Search Index window and snapshots that instead;
/// `TILDE_SNAPSHOT_WINDOW=settings|keys|cleanup-settings|welcome|history|cleanup` does the same for those windows, and
/// `panel` snapshots a panel the keys opened (Quick Look).
///
/// The image is the window exactly as macOS draws it (rounded corners, no shadow, Neovim
/// and Quick Look included), at the screen's scale. For README images and GIFs
/// (scripts/screenshots.py): `TILDE_SNAPSHOT_SIZE=1200x750` sets the window's size in points
/// (that's the default; it's put back if a window manager changes it), `TILDE_SNAPSHOT_KEY_DELAY` the seconds between keys (0.15), and
/// `TILDE_SNAPSHOT_FRAMES=<dir>` also saves a frame every 1/`TILDE_SNAPSHOT_FPS` (10) of a
/// second while the keys play, as `00000.png`…, with the keys and when they were pressed in
/// `keys.tsv`. `TILDE_SNAPSHOT_DELAY` is how long to wait after launch before starting (1 s).
@MainActor
enum DebugSnapshot {
    /// Whether this launch is a snapshot: it then leaves the saved window size and shelf alone.
    nonisolated static let isRunning = ProcessInfo.processInfo.environment["TILDE_SNAPSHOT"] != nil

    private static var keyDelay = 0.15
    private static var recorder: FrameRecorder?
    /// Returns whether a snapshot or benchmark is running (the welcome isn't shown then).
    static func runIfRequested() -> Bool {
        let env = ProcessInfo.processInfo.environment
        if let steps = env["TILDE_SCROLL_BENCH"].flatMap(Int.init) {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { scrollBench(steps: steps) }
            return true
        }
        guard let out = env["TILDE_SNAPSHOT"] else { return false }
        let keys = decode(env["TILDE_SNAPSHOT_KEYS"] ?? "")
        // `panel`: snapshot whatever panel is showing (Quick Look) after the keys.
        let showIndex = env["TILDE_SNAPSHOT_INDEX"] == "1" || env["TILDE_SNAPSHOT_WINDOW"] == "panel"
        let other: Selector? = switch env["TILDE_SNAPSHOT_WINDOW"] {
        case "settings": #selector(AppDelegate.showSettings(_:))
        case "keys": #selector(AppDelegate.showKeys(_:))
        case "cleanup-settings": #selector(AppDelegate.showCleanupSettings(_:))
        case "welcome": #selector(AppDelegate.showWelcome(_:))
        case "history": #selector(AppDelegate.showHistory(_:))
        case "cleanup": #selector(AppDelegate.showCleanup(_:))
        default: nil
        }
        keyDelay = env["TILDE_SNAPSHOT_KEY_DELAY"].flatMap(Double.init) ?? 0.15
        let size = (env["TILDE_SNAPSHOT_SIZE"] ?? "1200x750").split(separator: "x").compactMap { Double($0) }
        if size.count == 2 { snapshotSize = NSSize(width: size[0], height: size[1]) }
        holdSize {}
        let start = env["TILDE_SNAPSHOT_DELAY"].flatMap(Double.init) ?? 1.0
        DispatchQueue.main.asyncAfter(deadline: .now() + start) { holdSize {
            if let other { NSApp.sendAction(other, to: nil, from: nil) }
            if let dir = env["TILDE_SNAPSHOT_FRAMES"] {
                let fps = env["TILDE_SNAPSHOT_FPS"].flatMap(Double.init) ?? 10
                recorder = FrameRecorder(dir: dir, fps: fps) { target(panel: showIndex, other: other != nil) }
            }
            type(Array(keys)) {
                if env["TILDE_SNAPSHOT_INDEX"] == "1" { NSApp.sendAction(#selector(AppDelegate.showIndex(_:)), to: nil, from: nil) }
                DispatchQueue.main.asyncAfter(deadline: .now() + (showIndex || other != nil ? 2.0 : 0.6)) { holdSize {
                    recorder?.stop()
                    write(to: out, panel: showIndex, other: other != nil)
                    // `TILDE_SNAPSHOT_FOCUS=1`: which view has the keyboard at the end.
                    if env["TILDE_SNAPSHOT_FOCUS"] == "1" {
                        let responder = NSApp.keyWindow?.firstResponder ?? NSApp.windows.first?.firstResponder
                        let popup = QLPreviewPanel.sharedPreviewPanelExists() && QLPreviewPanel.shared().isVisible
                        let pane = (NSApp.windows.compactMap { $0.contentView.flatMap { find(PreviewPane.self, in: $0) } }.first)?.debugState ?? "-"
                        FileHandle.standardError.write("PREVIEW \(pane)\n".data(using: .utf8)!)
                        FileHandle.standardError.write("FOCUS \(responder.map { String(describing: Swift.type(of: $0)) } ?? "none") popup=\(popup)\n".data(using: .utf8)!)
                    }
                    NSApp.terminate(nil)
                } }
            }
        } }
        return true
    }

    private static var snapshotSize: NSSize?

    /// Gives the window its snapshot size, then runs `next` once it has kept it for a moment.
    /// A tiling window manager (AeroSpace, yabai) resizes new windows, sometimes a while
    /// after they appear, so the size is checked and put back until it stays.
    private static func holdSize(tries: Int = 12, then next: @escaping @MainActor () -> Void) {
        guard let size = snapshotSize, tries > 0,
              let window = NSApp.windows.first(where: { $0.windowController is BrowserWindowController })
        else { return next() }
        if window.contentRect(forFrameRect: window.frame).size == size {
            return next()
        }
        window.setContentSize(size)
        window.center()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { holdSize(tries: tries - 1, then: next) }
    }

    /// `TILDE_SCROLL_BENCH=<steps>`: scrolls the results list down a screenful-ish at a time
    /// (as a fast trackpad flick does), forcing layout and drawing after each step, and
    /// prints how long each step took. A step over 8.3 ms would drop a frame at 120 Hz.
    private static func scrollBench(steps: Int) {
        guard let window = NSApp.keyWindow ?? NSApp.windows.first,
              let table = window.contentView.flatMap({ find(NSTableView.self, in: $0) }),
              let clip = table.enclosingScrollView?.contentView
        else { return }
        let rows = table.numberOfRows
        let step = ProcessInfo.processInfo.environment["TILDE_SCROLL_STEP"].flatMap { Double($0) }.map { CGFloat($0) }
            ?? clip.bounds.height * 0.75
        var times: [Double] = []
        var layoutTimes: [Double] = []
        var y: CGFloat = 0
        let ms = { (a: UInt64, b: UInt64) in Double(b - a) / 1e6 }
        for _ in 0..<steps {
            y = min(y + step, max(table.bounds.height - clip.bounds.height, 0))
            let start = DispatchTime.now().uptimeNanoseconds
            clip.scroll(to: NSPoint(x: 0, y: y))
            table.enclosingScrollView?.reflectScrolledClipView(clip)
            window.contentView?.layoutSubtreeIfNeeded()
            let laidOut = DispatchTime.now().uptimeNanoseconds
            window.displayIfNeeded()
            let end = DispatchTime.now().uptimeNanoseconds
            times.append(ms(start, end))
            layoutTimes.append(ms(start, laidOut))
        }
        layoutTimes.sort()
        print(String(format: "  layout (row views, data): p50 %.2f ms", layoutTimes[layoutTimes.count / 2]))
        times.sort()
        let pct = { (p: Double) in times[min(times.count - 1, Int(Double(times.count) * p))] }
        print(String(format: "scroll bench: %d rows, %d steps of %.0f pt: p50 %.2f ms  p95 %.2f ms  max %.2f ms  over 8.3 ms: %d",
                     rows, steps, step, pct(0.5), pct(0.95), times.last ?? 0, times.filter { $0 > 8.3 }.count))
        NSApp.terminate(nil)
    }

    /// Control keys are encoded as a private-use character just above U+E000.
    private static let controlBase: UInt32 = 0xE000
    /// Command keys (`@s` is ⌘S), likewise.
    private static let commandBase: UInt32 = 0xE100
    /// `\w`: wait a second (for Neovim to start, say).
    private static let pause = Character(UnicodeScalar(0xE200)!)

    private static func decode(_ s: String) -> String {
        var s = s
        for c in "abcdefghijklmnopqrstuvwxyz" {
            s = s.replacingOccurrences(of: "^\(c)", with: String(UnicodeScalar(controlBase + UInt32(c.asciiValue!))!))
            s = s.replacingOccurrences(of: "@\(c)", with: String(UnicodeScalar(commandBase + UInt32(c.asciiValue!))!))
        }
        s = s.replacingOccurrences(of: "@.", with: String(UnicodeScalar(commandBase + 46)!))
        // ⌘⌫ and ⌃⌫.
        s = s.replacingOccurrences(of: "@\\b", with: String(UnicodeScalar(commandBase + 127)!))
        s = s.replacingOccurrences(of: "^\\b", with: String(UnicodeScalar(controlBase + 127)!))
        return s.replacingOccurrences(of: "\\e", with: "\u{1b}")
            .replacingOccurrences(of: "\\w", with: String(pause))
            .replacingOccurrences(of: "\\t", with: "\t")
            .replacingOccurrences(of: "\\r", with: "\r")
            .replacingOccurrences(of: "\\b", with: "\u{7f}")
    }

    private static func type(_ chars: [Character], then done: @escaping @MainActor () -> Void) {
        guard let c = chars.first else { return done() }
        if c == pause {
            return DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { type(Array(chars.dropFirst()), then: done) }
        }
        if let window = NSApp.keyWindow ?? NSApp.windows.first, let event = keyEvent(for: c, window: window) {
            recorder?.pressed(label(for: c))
            // Through the app (not the window) so local key monitors see it, as they would
            // a real keystroke.
            window.makeKey()
            // A scripted launch may never become the active app, and then menus have no key
            // window to send actions to. Find the menu item for a command key and send its
            // action down this window's responder chain, as a real keystroke would.
            if event.modifierFlags.contains(.command),
               let item = menuItem(for: event, in: NSApp.mainMenu),
               let action = item.action {
                _ = (window.firstResponder ?? window).tryToPerform(action, with: item)
            } else {
                NSApp.sendEvent(event)
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + keyDelay) {
            type(Array(chars.dropFirst()), then: done)
        }
    }

    private static func menuItem(for event: NSEvent, in menu: NSMenu?) -> NSMenuItem? {
        for item in menu?.items ?? [] {
            if let found = menuItem(for: event, in: item.submenu) { return found }
            let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            if item.keyEquivalent == event.charactersIgnoringModifiers, item.keyEquivalentModifierMask == flags {
                return item
            }
        }
        return nil
    }

    private static func keyEvent(for c: Character, window: NSWindow) -> NSEvent? {
        var s = String(c)
        var flags: NSEvent.ModifierFlags = []
        if let v = c.unicodeScalars.first?.value, v > controlBase, v < controlBase + 128 {
            s = String(UnicodeScalar(UInt8(v - controlBase)))
            flags = .control
        } else if let v = c.unicodeScalars.first?.value, v > commandBase, v < commandBase + 128 {
            s = String(UnicodeScalar(UInt8(v - commandBase)))
            flags = .command
        }
        let keyCode: UInt16 = switch s {
        case "\u{1b}": 53
        case "\t": 48
        case "\r": 36
        case "\u{7f}": 51
        default: 0
        }
        return NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: s, charactersIgnoringModifiers: s,
            isARepeat: false, keyCode: keyCode
        )
    }

    /// How a key is shown in a GIF's caption.
    private static func label(for c: Character) -> String {
        guard let v = c.unicodeScalars.first?.value else { return "" }
        if v > controlBase, v < controlBase + 128 { return "⌃" + (v - controlBase == 127 ? "⌫" : String(UnicodeScalar(UInt8(v - controlBase)))) }
        if v > commandBase, v < commandBase + 128 {
            let k = UInt8(v - commandBase)
            return "⌘" + (k == 127 ? "⌫" : String(UnicodeScalar(k)).uppercased())
        }
        return switch c {
        case "\u{1b}": "esc"
        case "\t": "tab"
        case "\r": "⏎"
        case "\u{7f}": "⌫"
        case " ": "space"
        default: String(c)
        }
    }

    private static func target(panel: Bool, other: Bool) -> NSWindow? {
        panel
            ? NSApp.windows.first { $0 is NSPanel && $0.isVisible }
            : other
            ? (NSApp.windows.first { $0.isVisible && $0.windowController is SettingsWindowController || $0.windowController is WelcomeWindowController }
                ?? NSApp.keyWindow ?? NSApp.windows.first)
            : (NSApp.keyWindow ?? NSApp.windows.first)
    }

    private static func write(to path: String, panel: Bool, other: Bool) {
        guard let window = target(panel: panel, other: other), let view = window.contentView?.superview else { return }
        if let image = capture(window) {
            savePNG(image, to: path)
        } else if let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
        }
        // Terminal views draw through layers the snapshot may miss: save their text too.
        if let terminal = find(TerminalView.self, in: view), !terminal.isHidden {
            try? terminal.getTerminal().getBufferAsData().write(to: URL(fileURLWithPath: path + ".txt"))
        }
    }

    /// The window as the window server draws it. An app may capture its own windows without
    /// the Screen Recording permission. `CGWindowListCreateImage` is hidden from Swift since
    /// macOS 15 in favour of ScreenCaptureKit (which asks for the permission), but it still
    /// works, so it's looked up at run time.
    static func capture(_ window: NSWindow) -> CGImage? {
        typealias Create = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else { return nil }
        let create = unsafeBitCast(symbol, to: Create.self)
        let options: CGWindowImageOption = [.boundsIgnoreFraming, .bestResolution]
        return create(.null, CGWindowListOption.optionIncludingWindow.rawValue, UInt32(window.windowNumber), options.rawValue)?
            .takeRetainedValue()
    }

    nonisolated static func savePNG(_ image: CGImage, to path: String) {
        guard let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: path) as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { return }
        CGImageDestinationAddImage(dest, image, nil)
        CGImageDestinationFinalize(dest)
    }

    private static func find<T: NSView>(_ type: T.Type, in view: NSView) -> T? {
        if let v = view as? T { return v }
        for sub in view.subviews {
            if let found = find(type, in: sub) { return found }
        }
        return nil
    }
}
/// Saves the window every 1/fps of a second, for GIFs. PNGs are encoded off the main thread.
@MainActor
private final class FrameRecorder {
    private let dir: String
    private let window: () -> NSWindow?
    private var timer: Timer?
    private var frame = 0
    private let start = Date()
    private var keys = ""
    private let queue = DispatchQueue(label: "tilde.frames", qos: .userInitiated)

    init(dir: String, fps: Double, window: @escaping () -> NSWindow?) {
        self.dir = dir
        self.window = window
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        timer = Timer.scheduledTimer(withTimeInterval: 1 / fps, repeats: true) { _ in
            MainActor.assumeIsolated { self.grab() }
        }
        grab()
    }

    func pressed(_ label: String) {
        keys += String(format: "%.3f\t%@\n", Date().timeIntervalSince(start), label)
    }

    private func grab() {
        guard let window = window(), let image = DebugSnapshot.capture(window) else { return }
        let path = (dir as NSString).appendingPathComponent(String(format: "%05d.png", frame))
        frame += 1
        nonisolated(unsafe) let image2 = image
        queue.async { DebugSnapshot.savePNG(image2, to: path) }
    }

    func stop() {
        timer?.invalidate()
        grab()
        let keys = keys, dir = dir
        queue.sync {
            try? keys.write(toFile: (dir as NSString).appendingPathComponent("keys.tsv"), atomically: true, encoding: .utf8)
        }
    }
}
#endif
