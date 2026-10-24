#if DEBUG
import AppKit

/// Development aid: `TILDE_SNAPSHOT=/path/out.png` renders the key window to a PNG after
/// launch and quits. `TILDE_SNAPSHOT_KEYS` replays keystrokes first (`\e` is escape, `\t` tab,
/// `\r` return, `\b` delete), so flows can be checked without screen-recording access.
@MainActor
enum DebugSnapshot {
    static func runIfRequested() {
        let env = ProcessInfo.processInfo.environment
        guard let out = env["TILDE_SNAPSHOT"] else { return }
        let keys = decode(env["TILDE_SNAPSHOT_KEYS"] ?? "")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
            type(Array(keys)) {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) {
                    write(to: out)
                    NSApp.terminate(nil)
                }
            }
        }
    }

    private static func decode(_ s: String) -> String {
        s.replacingOccurrences(of: "\\e", with: "\u{1b}")
            .replacingOccurrences(of: "\\t", with: "\t")
            .replacingOccurrences(of: "\\r", with: "\r")
            .replacingOccurrences(of: "\\b", with: "\u{7f}")
    }

    private static func type(_ chars: [Character], then done: @escaping @MainActor () -> Void) {
        guard let c = chars.first else { return done() }
        if let window = NSApp.keyWindow ?? NSApp.windows.first, let event = keyEvent(for: c, window: window) {
            window.sendEvent(event)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
            type(Array(chars.dropFirst()), then: done)
        }
    }

    private static func keyEvent(for c: Character, window: NSWindow) -> NSEvent? {
        let s = String(c)
        let keyCode: UInt16 = switch s {
        case "\u{1b}": 53
        case "\t": 48
        case "\r": 36
        case "\u{7f}": 51
        default: 0
        }
        return NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: s, charactersIgnoringModifiers: s,
            isARepeat: false, keyCode: keyCode
        )
    }

    private static func write(to path: String) {
        guard let window = NSApp.keyWindow ?? NSApp.windows.first, let view = window.contentView?.superview else { return }
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: rep)
        try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
    }
}
#endif
