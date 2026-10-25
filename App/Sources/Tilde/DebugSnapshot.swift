#if DEBUG
import AppKit

/// Development aid: `TILDE_SNAPSHOT=/path/out.png` renders the key window to a PNG after
/// launch and quits. `TILDE_SNAPSHOT_KEYS` replays keystrokes first (`\e` is escape, `\t` tab,
/// `\r` return, `\b` delete, `^j` control-j), so flows can be checked without
/// screen-recording access.
/// `TILDE_SNAPSHOT_INDEX=1` opens the Search Index window and snapshots that instead.
@MainActor
enum DebugSnapshot {
    static func runIfRequested() {
        let env = ProcessInfo.processInfo.environment
        guard let out = env["TILDE_SNAPSHOT"] else { return }
        let keys = decode(env["TILDE_SNAPSHOT_KEYS"] ?? "")
        let showIndex = env["TILDE_SNAPSHOT_INDEX"] == "1"
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
            type(Array(keys)) {
                if showIndex { NSApp.sendAction(#selector(AppDelegate.showIndex(_:)), to: nil, from: nil) }
                DispatchQueue.main.asyncAfter(deadline: .now() + (showIndex ? 2.0 : 0.6)) {
                    write(to: out, panel: showIndex)
                    NSApp.terminate(nil)
                }
            }
        }
    }

    /// Control keys are encoded as a private-use character just above U+E000.
    private static let controlBase: UInt32 = 0xE000
    /// Command keys (`@s` is ⌘S), likewise.
    private static let commandBase: UInt32 = 0xE100

    private static func decode(_ s: String) -> String {
        var s = s
        for c in "abcdefghijklmnopqrstuvwxyz" {
            s = s.replacingOccurrences(of: "^\(c)", with: String(UnicodeScalar(controlBase + UInt32(c.asciiValue!))!))
            s = s.replacingOccurrences(of: "@\(c)", with: String(UnicodeScalar(commandBase + UInt32(c.asciiValue!))!))
        }
        return s.replacingOccurrences(of: "\\e", with: "\u{1b}")
            .replacingOccurrences(of: "\\t", with: "\t")
            .replacingOccurrences(of: "\\r", with: "\r")
            .replacingOccurrences(of: "\\b", with: "\u{7f}")
    }

    private static func type(_ chars: [Character], then done: @escaping @MainActor () -> Void) {
        guard let c = chars.first else { return done() }
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

    private static func write(to path: String, panel: Bool) {
        let window = panel
            ? NSApp.windows.first { $0 is NSPanel && $0.isVisible }
            : (NSApp.keyWindow ?? NSApp.windows.first)
        guard let window, let view = window.contentView?.superview else { return }
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
    }
}
#endif
