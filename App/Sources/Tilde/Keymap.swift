import AppKit
import TildeFFI

/// Actions bound in `[keys.go]` and `[keys.select]`. Raw values are the config names.
enum KeyAction: String {
    case down, up, parent, enter, open, back, forward
    case selectMode = "select_mode"
    case top, bottom, search, activity, home, clear
    case shelf
    case shelfAll = "shelf_all"
    case move, copy, trash, tag, rename, edit
    case newItem = "new_item"
    case newFolder = "new_folder"
    case compress
    case copyPath = "copy_path"
    case hide, undo, redo, sort, history, cleanup
    case quickLook = "quick_look"
    case quickLookWindow = "quick_look_window"
    case previewFull = "preview_full"
    case previewSource = "preview_source"
    case help
    case openWith = "open_with"
    case filterAll = "filter_all"
    case filterDownloads = "filter_downloads"
    case filterScreenshots = "filter_screenshots"
    case filterEdited = "filter_edited"
    case filterAirdrop = "filter_airdrop"
    case inbox
    case goto, bookmark, yank, paste
    case pasteMove = "paste_move"
    case hidden, refresh, close
    case reverseSort = "reverse_sort"
}

/// The key bindings from the config, for matching key presses.
@MainActor
final class Keymap {
    private(set) static var shared = Keymap(loadKeymap())

    /// Re-reads the config file.
    static func reload() {
        shared = Keymap(loadKeymap())
    }

    let info: KeymapInfo
    private let go: [String: KeyAction]
    private let select: [String: KeyAction]

    init(_ info: KeymapInfo) {
        self.info = info
        func table(_ bindings: [ActionKeys]) -> [String: KeyAction] {
            var map: [String: KeyAction] = [:]
            for b in bindings {
                guard let action = KeyAction(rawValue: b.action) else {
                    assertionFailure("unknown action \(b.action)")
                    continue
                }
                for k in b.keys { map[Self.id(k.key, ctrl: k.ctrl, alt: k.alt, shift: k.shift)] = action }
            }
            return map
        }
        go = table(info.go)
        select = table(info.select)
    }

    /// The action `event` is bound to in `mode` (GO or SELECT).
    func action(for event: NSEvent, in mode: Mode) -> KeyAction? {
        Self.id(of: event).flatMap { action(id: $0, in: mode) }
    }

    /// The action for a key press already turned into its ID (`id(of:)`).
    func action(id: String, in mode: Mode) -> KeyAction? {
        switch mode {
        case .go: return go[id]
        case .select: return select[id]
        case .edit: return nil
        }
    }

    /// The first key for `action`, for hints: `m`, `⌃R`. Empty when unbound.
    func key(_ action: KeyAction, in mode: Mode) -> String {
        bindings(mode).first { $0.action == action.rawValue }?.keys.first?.display ?? ""
    }

    /// All keys for `action`, joined: `j ↓ ⌃J`.
    func keys(_ action: KeyAction, in mode: Mode) -> String {
        (bindings(mode).first { $0.action == action.rawValue }?.keys ?? []).map(\.display).joined(separator: " ")
    }

    private func bindings(_ mode: Mode) -> [ActionKeys] {
        mode == .go ? info.go : info.select
    }

    /// The actions of `mode` with their descriptions and keys, in the keymap's order.
    func all(in mode: Mode) -> [ActionKeys] {
        bindings(mode)
    }

    /// The first key of `action` as a key press in `window`, to run the action as if it
    /// were typed (the keys panel). `nil` when unbound.
    func event(for action: KeyAction, in mode: Mode, window: NSWindow) -> NSEvent? {
        guard let chord = bindings(mode).first(where: { $0.action == action.rawValue })?.keys.first else { return nil }
        return Self.event(key: chord.key, ctrl: chord.ctrl, alt: chord.alt, shift: chord.shift, window: window)
    }

    /// A key press: `key` is a character or one of the names (`return`, `down`…).
    static func event(key: String, ctrl: Bool = false, alt: Bool = false, shift: Bool = false, window: NSWindow) -> NSEvent? {
        let named: [String: (UInt16, String)] = [
            "down": (125, String(UnicodeScalar(NSDownArrowFunctionKey)!)),
            "up": (126, String(UnicodeScalar(NSUpArrowFunctionKey)!)),
            "left": (123, String(UnicodeScalar(NSLeftArrowFunctionKey)!)),
            "right": (124, String(UnicodeScalar(NSRightArrowFunctionKey)!)),
            "return": (36, "\r"), "tab": (48, "\t"), "esc": (53, "\u{1b}"), "space": (49, " "),
            "backspace": (51, "\u{7f}"), "delete": (117, String(UnicodeScalar(NSDeleteFunctionKey)!)),
            "home": (115, String(UnicodeScalar(NSHomeFunctionKey)!)), "end": (119, String(UnicodeScalar(NSEndFunctionKey)!)),
            "pageup": (116, String(UnicodeScalar(NSPageUpFunctionKey)!)),
            "pagedown": (121, String(UnicodeScalar(NSPageDownFunctionKey)!)),
        ]
        var flags: NSEvent.ModifierFlags = []
        if ctrl { flags.insert(.control) }
        if alt { flags.insert(.option) }
        if shift { flags.insert(.shift) }
        // Characters carry no key code; the keymap matches them by character.
        let (code, chars) = named[key] ?? (0, key)
        if [115, 116, 117, 119, 121, 123, 124, 125, 126].contains(code) { flags.insert(.function) }
        return NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, characters: chars, charactersIgnoringModifiers: chars,
            isARepeat: false, keyCode: code)
    }

    // MARK: Matching

    nonisolated private static let named: [UInt16: String] = [
        125: "down", 126: "up", 123: "left", 124: "right", 36: "return", 76: "return", 48: "tab", 53: "esc",
        49: "space", 51: "backspace", 117: "delete", 115: "home", 119: "end", 116: "pageup", 121: "pagedown",
    ]

    nonisolated private static func id(_ key: String, ctrl: Bool, alt: Bool, shift: Bool) -> String {
        "\(ctrl ? "c" : "")\(alt ? "a" : "")\(shift ? "s" : "")|\(key)"
    }

    /// The same form for a key press. ⌘ combinations belong to the menus.
    nonisolated static func id(of event: NSEvent) -> String? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        guard !flags.contains(.command) else { return nil }
        let ctrl = flags.contains(.control), alt = flags.contains(.option)
        if let name = named[event.keyCode], event.charactersIgnoringModifiers.map(isNamed) ?? true {
            return id(name, ctrl: ctrl, alt: alt, shift: flags.contains(.shift))
        }
        guard var key = event.charactersIgnoringModifiers, key.count == 1 else { return nil }
        // ⌃J and ⌃j are the same chord.
        if ctrl, key.first?.isLetter == true { key = key.lowercased() }
        return id(key, ctrl: ctrl, alt: alt, shift: false)
    }

    /// The config spelling of a key press (`ctrl+j`, `G`, `shift+tab`), for recording a
    /// binding. `nil` for ⌘ combinations and modifier keys alone.
    nonisolated static func spec(of event: NSEvent) -> String? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        guard !flags.contains(.command) else { return nil }
        var parts: [String] = []
        if flags.contains(.control) { parts.append("ctrl") }
        if flags.contains(.option) { parts.append("alt") }
        if let name = named[event.keyCode], event.charactersIgnoringModifiers.map(isNamed) ?? true {
            if flags.contains(.shift) { parts.append("shift") }
            return (parts + [name]).joined(separator: "+")
        }
        guard var key = event.charactersIgnoringModifiers, key.count == 1,
              key.unicodeScalars.first.map({ !CharacterSet.controlCharacters.contains($0) }) == true
        else { return nil }
        if flags.contains(.control), key.first?.isLetter == true { key = key.lowercased() }
        return (parts + [key]).joined(separator: "+")
    }

    /// Key codes are layout-independent; a code that types a printable character on this
    /// layout (keypad, unusual layouts) is matched by its character instead. Named keys
    /// produce control or private-use characters.
    nonisolated private static func isNamed(_ chars: String) -> Bool {
        guard let scalar = chars.unicodeScalars.first else { return true }
        return chars == " " || CharacterSet.controlCharacters.contains(scalar) || (0xF700...0xF8FF).contains(scalar.value)
    }
}
