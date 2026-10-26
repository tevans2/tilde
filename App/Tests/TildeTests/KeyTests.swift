import AppKit
import Testing
import TildeFFI
@testable import Tilde

@MainActor
struct KeyTests {
    private func event(_ chars: String, code: UInt16 = 0, _ flags: NSEvent.ModifierFlags = []) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0, windowNumber: 0, context: nil,
            characters: chars, charactersIgnoringModifiers: chars, isARepeat: false, keyCode: code)!
    }

    private func chord(_ key: String, ctrl: Bool = false, shift: Bool = false) -> KeyChord {
        KeyChord(key: key, ctrl: ctrl, alt: false, shift: shift, display: key)
    }

    private var keymap: Keymap {
        Keymap(KeymapInfo(
            go: [ActionKeys(action: "down", description: "", keys: [chord("down"), chord("j", ctrl: true)], defaults: [], customized: false)],
            select: [
                ActionKeys(action: "move", description: "", keys: [chord("M")], defaults: [], customized: true),
                ActionKeys(action: "back", description: "", keys: [chord("tab", shift: true)], defaults: [], customized: true),
            ]))
    }

    @Test func matchesCharactersChordsAndNamedKeys() {
        let map = keymap
        #expect(map.action(for: event("j", code: 38, .control), in: .go) == .down)
        #expect(map.action(for: event("J", code: 38, [.control, .shift]), in: .go) == .down, "⌃J is ⌃j")
        #expect(map.action(for: event("\u{F701}", code: 125, [.numericPad, .function]), in: .go) == .down)
        #expect(map.action(for: event("j", code: 38), in: .go) == nil, "plain j types in GO")
        #expect(map.action(for: event("M", code: 46, .shift), in: .select) == .move, "case matters")
        #expect(map.action(for: event("m", code: 46), in: .select) == nil)
        #expect(map.action(for: event("\u{19}", code: 48, .shift), in: .select) == .back)
        #expect(map.action(for: event("m", code: 46, .command), in: .select) == nil, "⌘ belongs to the menus")
        #expect(map.action(for: event("j", code: 38), in: .edit) == nil)
    }

    @Test func recordsKeysInTheConfigsSpelling() {
        #expect(Keymap.spec(of: event("j", code: 38, .control)) == "ctrl+j")
        #expect(Keymap.spec(of: event("G", code: 5, .shift)) == "G")
        #expect(Keymap.spec(of: event("\u{F701}", code: 125, [.function])) == "down")
        #expect(Keymap.spec(of: event("\u{19}", code: 48, .shift)) == "shift+tab")
        #expect(Keymap.spec(of: event("k", code: 40, .command)) == nil)
    }

    @Test func recordsGlobalHotkeys() throws {
        let spec = try #require(DialogHelper.spec(of: event(" ", code: 49, [.control, .option])))
        #expect(spec == "ctrl+alt+space")
        #expect(try parseHotkey(spec: spec).display == "⌃⌥Space")
        #expect(DialogHelper.spec(of: event("o", code: 31, [.command, .shift])) == "shift+cmd+o")
        #expect(DialogHelper.keyCode(for: "space") == 49)
        #expect(DialogHelper.keyCode(for: "return") == 36)
        #expect(DialogHelper.keyCode(for: "g") != nil, "found on the current layout")
    }

    @Test func openWithMatchesLettersInOrder() {
        #expect(BrowserWindowController.fuzzy("vsc", "Visual Studio Code"))
        #expect(BrowserWindowController.fuzzy("", "Preview"))
        #expect(!BrowserWindowController.fuzzy("csv", "Visual Studio Code"))
    }
}
