import AppKit
import ApplicationServices
import Carbon.HIToolbox
import TildeFFI

/// The file-dialog helper (`[dialog]` in the config): a global hotkey that, while another
/// app shows an Open or Save dialog, opens Tilde's search in a small panel over it, then
/// takes the dialog to what's picked. The dialog is driven the way a person would: Go to
/// Folder (⌘⇧G), the path, Return. Needs the Accessibility permission.
@MainActor
final class DialogHelper {
    static let shared = DialogHelper()

    /// Set by the app: for searching, and the folder open in the front browser window.
    var index: IndexService?
    var currentFolder: () -> String? = { nil }

    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private var confirm = false
    private var panel: NSPanel?
    private var keyMonitor: Any?

    /// Registers `hotkey` (or nothing, when it's off).
    func configure(hotkey: HotkeyInfo?, confirm: Bool) {
        self.confirm = confirm
        if let hotKey { UnregisterEventHotKey(hotKey) }
        hotKey = nil
        guard let hotkey, let code = Self.keyCode(for: hotkey.key) else { return }
        installHandler()
        var modifiers: UInt32 = 0
        if hotkey.cmd { modifiers |= UInt32(cmdKey) }
        if hotkey.alt { modifiers |= UInt32(optionKey) }
        if hotkey.ctrl { modifiers |= UInt32(controlKey) }
        if hotkey.shift { modifiers |= UInt32(shiftKey) }
        let id = EventHotKeyID(signature: OSType(0x544C_4445), id: 1) // "TLDE"
        let status = RegisterEventHotKey(UInt32(code), modifiers, id, GetApplicationEventTarget(), 0, &hotKey)
        if status != noErr {
            hotKey = nil
            Toast.show("The file dialog hotkey \(hotkey.display) is taken by another app. Pick another in Settings.")
        }
    }

    private func installHandler() {
        guard handler == nil else { return }
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, _, _ in
            DispatchQueue.main.async { MainActor.assumeIsolated { DialogHelper.shared.hotkeyPressed() } }
            return noErr
        }, 1, &spec, nil, &handler)
    }

    // MARK: The hotkey

    private func hotkeyPressed() {
        if let panel, panel.isVisible { return close() }
        guard AXIsProcessTrusted() else {
            Permissions.askForAccessibility()
            Toast.show("Tilde needs Accessibility access to work with file dialogs. Turn it on in Privacy & Security, then press the hotkey again.")
            return
        }
        guard let app = NSWorkspace.shared.frontmostApplication, app.processIdentifier != getpid() else {
            return Toast.show("Open or Save dialog first, then press the hotkey")
        }
        let pid = app.processIdentifier
        guard let dialog = DialogDriver.findDialog(pid: pid) else {
            NSSound.beep()
            return Toast.show("No Open or Save dialog in front in \(app.localizedName ?? "this app")")
        }
        show(for: dialog, app: app)
    }

    /// The search, in a panel over the dialog that takes the keyboard without bringing
    /// Tilde to the front.
    private func show(for dialog: DialogDriver.Dialog, app: NSRunningApplication) {
        close()
        let size = NSSize(width: 640, height: 420)
        let panel = HelperPanel(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.nonactivatingPanel, .borderless], backing: .buffered, defer: false)
        panel.level = .modalPanel
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.becomesKeyOnlyIfNeeded = false
        panel.isFloatingPanel = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        // Over the dialog, near its top; else the middle of the screen.
        if let frame = dialog.frame, let screen = NSScreen.screens.first {
            // AX frames are top-left based on the primary screen.
            let y = screen.frame.maxY - frame.minY - size.height - 40
            panel.setFrameOrigin(NSPoint(x: frame.midX - size.width / 2, y: y))
        } else {
            panel.center()
        }
        let index = self.index
        let here = currentFolder()
        let kind = dialog.isSave ? "Save" : "Open"
        let overlay = PromptOverlay(
            title: "\(kind) in \(app.localizedName ?? "app"): go to…",
            kind: .destination(
                search: { query in
                    var found = index?.find(query: query, limit: 12) ?? []
                    // With nothing typed, the folder open in Tilde comes first.
                    if query.isEmpty, let here, !found.contains(where: { $0.path == here }) {
                        found.insert(Destination(path: here, display: Self.display(here) + "  (open in Tilde)", highlights: []), at: 0)
                    }
                    return found
                },
                onPick: { [weak self] path in self?.go(to: path, dialog: dialog) }),
            hint: dialog.isSave
                ? "↑↓ choose   ⏎ go there (a file: its folder)   esc cancel"
                : "↑↓ choose   ⏎ go there   esc cancel",
            placeholder: "Search files and folders, or type a path",
            floating: true,
            onClose: { [weak self] in self?.close() })
        panel.contentView = overlay
        self.panel = panel
        panel.makeKeyAndOrderFront(nil)
        overlay.focus()
        // ⌃j / ⌃k (the GO keys) move through results, as in the folder picker.
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            let id = Keymap.id(of: event)
            let handled = MainActor.assumeIsolated { () -> Bool in
                guard event.window === self.panel, let id, let action = Keymap.shared.action(id: id, in: .go) else { return false }
                switch action {
                case .down: overlay.moveSelection(by: 1)
                case .up: overlay.moveSelection(by: -1)
                default: return false
                }
                return true
            }
            return handled ? nil : event
        }
    }

    private func close() {
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        keyMonitor = nil
        panel?.orderOut(nil)
        panel = nil
    }

    private func go(to path: String, dialog: DialogDriver.Dialog) {
        close()
        let isFolder = (try? URL(fileURLWithPath: path).resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true
        // A Save dialog keeps its typed name: a file picks its folder.
        let target = dialog.isSave && !isFolder ? (path as NSString).deletingLastPathComponent : path
        let press = confirm && !dialog.isSave && !isFolder
        index?.recordOpen(path: path)
        let gKey = Self.keyCode(for: "g") ?? 5
        Task.detached {
            let error = DialogDriver.go(pid: dialog.pid, to: target, confirm: press, gKey: gKey)
            if let error { await MainActor.run { Toast.show(error) } }
        }
    }

    static func display(_ path: String) -> String {
        let home = NSHomeDirectory()
        return path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
    }

    // MARK: Keys

    nonisolated private static let namedCodes: [String: UInt16] = [
        "space": 49, "return": 36, "tab": 48, "esc": 53, "backspace": 51, "delete": 117, "down": 125, "up": 126,
        "left": 123, "right": 124, "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
    ]

    /// The key code that types `key` on the current keyboard layout (or a named key's).
    nonisolated static func keyCode(for key: String) -> UInt16? {
        if let code = namedCodes[key] { return code }
        guard let source = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
              let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData)
        else { return nil }
        let layout = unsafeBitCast(data, to: CFData.self)
        let bytes = CFDataGetBytePtr(layout)!
        return bytes.withMemoryRebound(to: UCKeyboardLayout.self, capacity: 1) { keyboard in
            for code in UInt16(0)..<128 {
                var dead: UInt32 = 0
                var chars = [UniChar](repeating: 0, count: 4)
                var length = 0
                let status = UCKeyTranslate(
                    keyboard, code, UInt16(kUCKeyActionDown), 0, UInt32(LMGetKbdType()),
                    OptionBits(kUCKeyTranslateNoDeadKeysBit), &dead, chars.count, &length, &chars)
                if status == noErr, length > 0, String(utf16CodeUnits: chars, count: length).lowercased() == key.lowercased() {
                    return code
                }
            }
            return nil
        }
    }

    /// The config spelling of a key press with modifiers (`ctrl+alt+space`, `cmd+shift+o`),
    /// for recording the hotkey.
    static func spec(of event: NSEvent) -> String? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        var parts: [String] = []
        if flags.contains(.control) { parts.append("ctrl") }
        if flags.contains(.option) { parts.append("alt") }
        if flags.contains(.shift) { parts.append("shift") }
        if flags.contains(.command) { parts.append("cmd") }
        if let name = namedCodes.first(where: { $0.value == event.keyCode })?.key {
            return (parts + [name]).joined(separator: "+")
        }
        guard let chars = event.charactersIgnoringModifiers?.lowercased(), chars.count == 1,
              let scalar = chars.unicodeScalars.first, !CharacterSet.controlCharacters.contains(scalar)
        else { return nil }
        return (parts + [chars]).joined(separator: "+")
    }
}

/// A panel that can take the keyboard without being titled.
final class HelperPanel: NSPanel {
    override var canBecomeKey: Bool { true }
}

/// Finding and driving another app's Open or Save dialog through the Accessibility API.
/// Runs off the main thread: it waits on the other app.
enum DialogDriver {
    struct Dialog: Sendable {
        let pid: pid_t
        let isSave: Bool
        /// Top-left based screen frame, if known.
        let frame: CGRect?
    }

    private static func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
        var value: AnyObject?
        return AXUIElementCopyAttributeValue(e, name as CFString, &value) == .success ? value : nil
    }

    private static func id(_ e: AXUIElement?) -> String {
        e.flatMap { attr($0, "AXIdentifier") as? String } ?? ""
    }

    private static func element(_ value: AnyObject?) -> AXUIElement? {
        guard let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return (value as! AXUIElement)
    }

    /// The app's Open or Save dialog: its focused window, or a sheet on it.
    nonisolated static func findDialog(pid: pid_t) -> Dialog? {
        let app = AXUIElementCreateApplication(pid)
        guard let window = element(attr(app, kAXFocusedWindowAttribute)) else { return nil }
        let sheets = ((attr(window, kAXChildrenAttribute) as? [AXUIElement]) ?? [])
            .filter { (attr($0, kAXRoleAttribute) as? String) == kAXSheetRole }
        for candidate in [window] + sheets {
            let identifier = id(candidate)
            guard identifier == "open-panel" || identifier == "save-panel" else { continue }
            return Dialog(pid: pid, isSave: identifier == "save-panel", frame: frame(of: candidate))
        }
        return nil
    }

    private static func frame(of e: AXUIElement) -> CGRect? {
        var origin = CGPoint.zero, size = CGSize.zero
        guard let p = attr(e, kAXPositionAttribute), let s = attr(e, kAXSizeAttribute),
              AXValueGetValue(p as! AXValue, .cgPoint, &origin), AXValueGetValue(s as! AXValue, .cgSize, &size)
        else { return nil }
        return CGRect(origin: origin, size: size)
    }

    /// Takes the dialog in app `pid` to `path` (Go to Folder), then presses its Open or
    /// Save button if `confirm`. Returns why it couldn't, for the user.
    nonisolated static func go(pid: pid_t, to path: String, confirm: Bool, gKey: UInt16) -> String? {
        let app = AXUIElementCreateApplication(pid)
        func focused() -> AXUIElement? { element(attr(app, kAXFocusedUIElementAttribute)) }
        func window() -> AXUIElement? { element(attr(app, kAXFocusedWindowAttribute)) }
        func wait(_ ok: () -> Bool) -> Bool {
            for _ in 0..<40 {
                if ok() { return true }
                usleep(50_000)
            }
            return false
        }
        func key(_ code: UInt16, _ flags: CGEventFlags = []) {
            for down in [true, false] {
                let e = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)
                e?.flags = flags
                e?.post(tap: .cghidEventTap)
            }
        }

        NSRunningApplication(processIdentifier: pid)?.activate()
        guard wait({ NSWorkspace.shared.frontmostApplication?.processIdentifier == pid }) else {
            return "Couldn't bring the dialog back to the front"
        }
        key(gKey, [.maskCommand, .maskShift])
        guard wait({ id(window()) == "GoToWindow" && id(focused()) == "PathTextField" }), let field = focused() else {
            return "The dialog didn't open Go to Folder (⌘⇧G)"
        }
        // Set rather than typed: typing races the field's completion.
        guard AXUIElementSetAttributeValue(field, kAXValueAttribute as CFString, path as CFString) == .success else {
            return "Couldn't fill in Go to Folder"
        }
        usleep(250_000)
        key(36) // Return
        guard wait({ id(window()) != "GoToWindow" }) else { return "Go to Folder didn't close" }
        if confirm, let dialog = window() {
            usleep(200_000)
            if let ok = find("OKButton", in: dialog) {
                AXUIElementPerformAction(ok, kAXPressAction as CFString)
            }
        }
        return nil
    }

    private static func find(_ identifier: String, in e: AXUIElement, depth: Int = 0) -> AXUIElement? {
        if id(e) == identifier { return e }
        guard depth < 8 else { return nil }
        for child in (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] {
            if let found = find(identifier, in: child, depth: depth + 1) { return found }
        }
        return nil
    }
}

/// A short message in a small floating panel at the top of the screen (the helper works
/// while other apps are in front, so the status bar isn't visible).
@MainActor
enum Toast {
    private static var panel: NSPanel?
    private static var hide: DispatchWorkItem?

    static func show(_ message: String) {
        hide?.cancel()
        let label = NSTextField(wrappingLabelWithString: message)
        label.font = .systemFont(ofSize: 13)
        label.textColor = .labelColor
        label.preferredMaxLayoutWidth = 420
        let size = label.fittingSize
        let box = NSVisualEffectView(frame: NSRect(x: 0, y: 0, width: size.width + 32, height: size.height + 20))
        box.material = .hudWindow
        box.state = .active
        box.wantsLayer = true
        box.layer?.cornerRadius = 10
        label.frame = NSRect(x: 16, y: 10, width: size.width, height: size.height)
        box.addSubview(label)
        let p = panel ?? NSPanel(contentRect: box.frame, styleMask: [.nonactivatingPanel, .borderless], backing: .buffered, defer: false)
        p.level = .statusBar
        p.isOpaque = false
        p.backgroundColor = .clear
        p.ignoresMouseEvents = true
        p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        p.contentView = box
        p.setContentSize(box.frame.size)
        if let screen = NSScreen.main {
            p.setFrameOrigin(NSPoint(x: screen.frame.midX - box.frame.width / 2, y: screen.visibleFrame.maxY - box.frame.height - 24))
        }
        p.orderFrontRegardless()
        panel = p
        let work = DispatchWorkItem { MainActor.assumeIsolated { panel?.orderOut(nil) } }
        hide = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 3.5, execute: work)
    }
}
