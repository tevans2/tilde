#if DEBUG
import AppKit
import SwiftTerm

/// Development aid: `TILDE_SNAPSHOT=/path/out.png` renders the key window to a PNG after
/// launch and quits. `TILDE_SNAPSHOT_KEYS` replays keystrokes first (`\e` is escape, `\t` tab,
/// `\r` return, `\b` delete, `^j` control-j, `@s` command-s, `\w` a one-second pause), so
/// flows can be checked without screen-recording access.
/// `TILDE_SNAPSHOT_INDEX=1` opens the Search Index window and snapshots that instead;
/// `TILDE_SNAPSHOT_WINDOW=settings|keys|welcome` does the same for those windows, and
/// `panel` snapshots a panel the keys opened (Quick Look).
@MainActor
enum DebugSnapshot {
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
        case "welcome": #selector(AppDelegate.showWelcome(_:))
        case "history": #selector(AppDelegate.showHistory(_:))
        default: nil
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
            if let other { NSApp.sendAction(other, to: nil, from: nil) }
            type(Array(keys)) {
                if env["TILDE_SNAPSHOT_INDEX"] == "1" { NSApp.sendAction(#selector(AppDelegate.showIndex(_:)), to: nil, from: nil) }
                DispatchQueue.main.asyncAfter(deadline: .now() + (showIndex || other != nil ? 2.0 : 0.6)) {
                    write(to: out, panel: showIndex, other: other != nil)
                    NSApp.terminate(nil)
                }
            }
        }
        return true
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
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
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

    private static func write(to path: String, panel: Bool, other: Bool) {
        let window = panel
            ? NSApp.windows.first { $0 is NSPanel && $0.isVisible }
            : other
            ? NSApp.windows.first { $0.isVisible && $0.windowController is SettingsWindowController || $0.windowController is WelcomeWindowController
                || $0.windowController is HistoryWindowController }
            : (NSApp.keyWindow ?? NSApp.windows.first)
        guard let window, let view = window.contentView?.superview else { return }
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
        // Terminal views draw through layers the snapshot may miss: save their text too.
        if let terminal = find(TerminalView.self, in: view), !terminal.isHidden {
            try? terminal.getTerminal().getBufferAsData().write(to: URL(fileURLWithPath: path + ".txt"))
        }
    }

    private static func find<T: NSView>(_ type: T.Type, in view: NSView) -> T? {
        if let v = view as? T { return v }
        for sub in view.subviews {
            if let found = find(type, in: sub) { return found }
        }
        return nil
    }
}
#endif
