import AppKit
import TildeFFI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// One index for every window: loaded from disk or scanned at launch.
    private let index = IndexService()
    /// One operations queue and undo history for every window, like the Finder.
    private let opsEvents = OpsEvents()
    private lazy var ops = FileOps(listener: opsEvents)
    private var windows: [BrowserWindowController] = []
    private lazy var indexWindow = IndexWindowController(index: index)
    private lazy var settingsWindow = SettingsWindowController(
        onChange: { [weak self] in self?.reloadConfig(nil) },
        showIndex: { [weak self] in self?.showIndex(nil) })
    private lazy var welcomeWindow = WelcomeWindowController(onChange: { [weak self] in self?.reloadConfig(nil) })

    private var appearanceObservation: NSKeyValueObservation?

    /// The key that stops a running operation (File › Stop Operation).
    static let stopKey = "⌘."

    func applicationDidFinishLaunching(_ notification: Notification) {
        opsEvents.delegate = self
        NSApp.appearance = Theme.shared.appearance
        NSApp.mainMenu = MainMenu.build()
        // The Dock icon follows light and dark mode (the icon file can't on macOS 15).
        appearanceObservation = NSApp.observe(\.effectiveAppearance, options: [.initial]) { _, _ in
            DispatchQueue.main.async { MainActor.assumeIsolated { Self.updateDockIcon() } }
        }
        // Launched to open a folder (`tilde` in a terminal, or a folder dropped on the Dock
        // icon): that makes the window. Otherwise, one at home.
        let isDefault = notification.userInfo?[NSApplication.launchIsDefaultUserInfoKey] as? Bool ?? true
        if isDefault {
            newWindow(nil)
        } else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
                guard let self, !self.windows.contains(where: { $0.window?.isVisible == true }) else { return }
                self.newWindow(nil)
            }
        }
        NSApp.activate()
        DialogHelper.shared.index = index
        DialogHelper.shared.currentFolder = { [weak self] in
            self?.windows.first { $0.window?.isVisible == true }?.folderShowing
        }
        DialogHelper.shared.configure(hotkey: Theme.shared.dialogHotkey, confirm: Theme.shared.dialogConfirm)
        #if DEBUG
        if PreviewBench.runIfRequested() || DebugSnapshot.runIfRequested() { return }
        #endif
        if !WelcomeWindowController.seen { showWelcome(nil) }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationWillTerminate(_ notification: Notification) {
        // Quitting with windows opened by `tilde --cd` sends their terminals back too.
        windows.forEach { $0.returnToTerminal() }
        // So the next launch loads instead of scanning.
        index.save()
    }

    /// Uses the PNGs that scripts/install.sh puts in the app bundle; development builds
    /// keep the default icon.
    private static func updateDockIcon() {
        let dark = NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        guard let url = Bundle.main.url(forResource: dark ? "Tilde-dark" : "Tilde-light", withExtension: "png"),
              let image = NSImage(contentsOf: url)
        else { return }
        NSApp.applicationIconImage = image
    }

    @objc func newWindow(_ sender: Any?) {
        newWindow(start: nil)
    }

    @discardableResult
    private func newWindow(start: String?, select: String? = nil, terminal: TerminalReturn? = nil) -> BrowserWindowController {
        windows.removeAll { $0.window?.isVisible != true }
        let controller = BrowserWindowController(index: index, ops: ops, start: start, select: select, terminal: terminal)
        windows.append(controller)
        controller.showWindow(nil)
        return controller
    }

    /// Folders (and files, shown in their folder) dropped on the Dock icon or opened with
    /// `open -a Tilde`, and `tilde://open` links from the `tilde` command.
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            if url.isFileURL {
                var isDir: ObjCBool = false
                guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir) else { continue }
                if isDir.boolValue {
                    newWindow(start: url.path)
                } else {
                    newWindow(start: url.deletingLastPathComponent().path, select: url.lastPathComponent)
                }
            } else if url.scheme == "tilde", url.host == "open" {
                openFromTerminal(url)
            }
        }
        NSApp.activate()
    }

    /// `tilde://open?path=…&select=…&reply=…&return=…` (see App/bin/tilde).
    private func openFromTerminal(_ url: URL) {
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        let value = { (name: String) in items.first { $0.name == name }?.value.flatMap { $0.isEmpty ? nil : $0 } }
        guard let path = value("path"), path.hasPrefix("/") else { return }
        let reply = value("reply").flatMap { TerminalReturn.isReplyPipe($0) ? $0 : nil }
        let terminal = reply.map { TerminalReturn(reply: $0, app: value("return")) }
        newWindow(start: path, select: value("select"), terminal: terminal)
    }

    /// Creates the config file from a commented template if needed and opens it.
    @objc func openConfig(_ sender: Any?) {
        let url: URL
        do {
            url = URL(fileURLWithPath: try ensureConfigFile())
        } catch {
            NSSound.beep()
            return
        }
        if NSWorkspace.shared.urlForApplication(toOpen: url) != nil {
            NSWorkspace.shared.open(url)
        } else {
            // No app claims .toml files; fall back to TextEdit.
            let textEdit = URL(fileURLWithPath: "/System/Applications/TextEdit.app")
            NSWorkspace.shared.open([url], withApplicationAt: textEdit, configuration: NSWorkspace.OpenConfiguration())
        }
    }

    @objc func undoFileOperation(_ sender: Any?) {
        if ops.undo() == nil { NSSound.beep() }
    }

    @objc func redoFileOperation(_ sender: Any?) {
        if ops.redo() == nil { NSSound.beep() }
    }

    @objc func showIndex(_ sender: Any?) {
        indexWindow.showWindow(nil)
    }

    /// History opens in the front browser window.
    @objc func showHistory(_ sender: Any?) {
        let front = NSApp.keyWindow?.windowController as? BrowserWindowController
        (front ?? windows.first { $0.window?.isVisible == true })?.showHistory()
    }

    /// Clean Up opens in the front browser window.
    @objc func showCleanup(_ sender: Any?) {
        let front = NSApp.keyWindow?.windowController as? BrowserWindowController
        (front ?? windows.first { $0.window?.isVisible == true })?.showCleanup()
    }

    @objc func showSettings(_ sender: Any?) {
        settingsWindow.show()
    }

    @objc func showKeys(_ sender: Any?) {
        settingsWindow.show(tab: .keys)
    }

    @objc func showCleanupSettings(_ sender: Any?) {
        settingsWindow.show(tab: .cleanup)
    }

    @objc func showWelcome(_ sender: Any?) {
        welcomeWindow.show()
    }

    @objc func reloadConfig(_ sender: Any?) {
        Keymap.reload()
        let theme = Theme.reload()
        DialogHelper.shared.configure(hotkey: theme.dialogHotkey, confirm: theme.dialogConfirm)
        NSApp.appearance = theme.appearance
        windows.removeAll { $0.window?.isVisible != true }
        for w in windows { w.applyTheme(theme) }
    }
}

extension AppDelegate: NSMenuItemValidation {
    /// Undo and Redo name the operation they'd reverse ("Undo Move 3 items").
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(undoFileOperation(_:)):
            let label = ops.undoLabel()
            item.title = label.map { "Undo \($0)" } ?? "Undo"
            return label != nil
        case #selector(redoFileOperation(_:)):
            let label = ops.redoLabel()
            item.title = label.map { "Redo \($0)" } ?? "Redo"
            return label != nil
        default:
            return true
        }
    }
}

extension AppDelegate: OpsEventsDelegate {
    func operationStarted(id: UInt64, label: String, total: Int) {
        windows.forEach { $0.operationStarted(id: id, label: label, total: total) }
    }

    func operationProgress(id: UInt64, done: Int, total: Int, current: String) {
        windows.forEach { $0.operationProgress(id: id, done: done, total: total, current: current) }
    }

    func operationBytes(id: UInt64, copied: UInt64, total: UInt64) {
        windows.forEach { $0.operationBytes(id: id, copied: copied, total: total) }
    }

    func operationFinished(id: UInt64, outcome: OpOutcome) {
        windows.removeAll { $0.window?.isVisible != true }
        windows.forEach { $0.operationFinished(id: id, outcome: outcome) }
    }
}

enum MainMenu {
    @MainActor
    static func build() -> NSMenu {
        let main = NSMenu()

        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "About Tilde", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: "")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Settings…", action: #selector(AppDelegate.showSettings(_:)), keyEquivalent: ",")
        let configFile = appMenu.addItem(withTitle: "Open Config File…", action: #selector(AppDelegate.openConfig(_:)), keyEquivalent: ",")
        configFile.keyEquivalentModifierMask = [.command, .option]
        appMenu.addItem(withTitle: "Reload Config", action: #selector(AppDelegate.reloadConfig(_:)), keyEquivalent: "r")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Hide Tilde", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        appMenu.addItem(withTitle: "Quit Tilde", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        main.addItem(appItem)

        let fileItem = NSMenuItem()
        let fileMenu = NSMenu(title: "File")
        fileMenu.addItem(withTitle: "New Window", action: #selector(AppDelegate.newWindow(_:)), keyEquivalent: "n")
        let newFolder = fileMenu.addItem(withTitle: "New Folder…", action: #selector(BrowserWindowController.newFolder(_:)), keyEquivalent: "n")
        fileMenu.addItem(withTitle: "Open", action: #selector(BrowserWindowController.openSelected(_:)), keyEquivalent: "o")
        // ⌘↓ as in the Finder, without a second line in the menu.
        let openDown = fileMenu.addItem(withTitle: "Open", action: #selector(BrowserWindowController.openSelected(_:)),
                                        keyEquivalent: String(UnicodeScalar(NSDownArrowFunctionKey)!))
        openDown.isHidden = true
        openDown.allowsKeyEquivalentWhenHidden = true
        fileMenu.addItem(withTitle: "Apply Edits", action: #selector(BrowserWindowController.applyEdits(_:)), keyEquivalent: "s")
        // "Stop Operation" when not editing.
        fileMenu.addItem(withTitle: "Cancel Edit", action: #selector(BrowserWindowController.cancelEdits(_:)), keyEquivalent: ".")
        newFolder.keyEquivalentModifierMask = [.command, .shift]
        fileMenu.addItem(.separator())
        fileMenu.addItem(withTitle: "Quick Look", action: #selector(BrowserWindowController.toggleQuickLook(_:)), keyEquivalent: "y")
        fileMenu.addItem(.separator())
        let trash = fileMenu.addItem(withTitle: "Move to Trash", action: #selector(BrowserWindowController.moveToTrash(_:)),
                                     keyEquivalent: String(Character(UnicodeScalar(NSBackspaceCharacter)!)))
        trash.keyEquivalentModifierMask = [.command]
        fileMenu.addItem(.separator())
        fileMenu.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        fileItem.submenu = fileMenu
        main.addItem(fileItem)

        // Standard edit actions so ⌘C/⌘V/⌘A work in the path bar.
        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")
        editMenu.addItem(withTitle: "Undo", action: #selector(AppDelegate.undoFileOperation(_:)), keyEquivalent: "z")
        let redo = editMenu.addItem(withTitle: "Redo", action: #selector(AppDelegate.redoFileOperation(_:)), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]
        let history = editMenu.addItem(withTitle: "History…", action: #selector(AppDelegate.showHistory(_:)), keyEquivalent: "z")
        history.keyEquivalentModifierMask = [.command, .option]
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        // Held ⌥, as in the Finder: the copied files move here.
        let moveHere = editMenu.addItem(withTitle: "Move Items Here", action: #selector(BrowserWindowController.pasteMoving(_:)),
                                        keyEquivalent: "v")
        moveHere.keyEquivalentModifierMask = [.command, .option]
        moveHere.isAlternate = true
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu
        main.addItem(editItem)

        let viewItem = NSMenuItem()
        let viewMenu = NSMenu(title: "View")
        let hidden = viewMenu.addItem(withTitle: "Show Hidden Files", action: #selector(BrowserWindowController.toggleHidden(_:)), keyEquivalent: ".")
        hidden.keyEquivalentModifierMask = [.command, .shift]
        let sortItem = viewMenu.addItem(withTitle: "Sort By", action: nil, keyEquivalent: "")
        let sortMenu = NSMenu(title: "Sort By")
        for (i, title) in ["Name", "Date Modified", "Date Added", "Size", "Kind"].enumerated() {
            let item = sortMenu.addItem(withTitle: title, action: #selector(BrowserWindowController.sortByMenu(_:)),
                                        keyEquivalent: "\(i + 1)")
            item.keyEquivalentModifierMask = [.command, .control]
            item.tag = i
        }
        sortMenu.addItem(.separator())
        sortMenu.addItem(withTitle: "Reverse Order", action: #selector(BrowserWindowController.sortByMenu(_:)), keyEquivalent: "")
            .tag = -1
        sortItem.submenu = sortMenu
        viewMenu.addItem(.separator())
        viewMenu.addItem(withTitle: "Clean Up…", action: #selector(AppDelegate.showCleanup(_:)), keyEquivalent: "")
        let indexItem = viewMenu.addItem(withTitle: "Search Index…", action: #selector(AppDelegate.showIndex(_:)), keyEquivalent: "i")
        indexItem.keyEquivalentModifierMask = [.command, .shift]
        viewItem.submenu = viewMenu
        main.addItem(viewItem)

        let goItem = NSMenuItem()
        let goMenu = NSMenu(title: "Go")
        goMenu.addItem(withTitle: "Back", action: #selector(BrowserWindowController.goBack(_:)), keyEquivalent: "[")
        goMenu.addItem(withTitle: "Forward", action: #selector(BrowserWindowController.goForward(_:)), keyEquivalent: "]")
        goMenu.addItem(withTitle: "Enclosing Folder", action: #selector(BrowserWindowController.enclosingFolder(_:)),
                       keyEquivalent: String(UnicodeScalar(NSUpArrowFunctionKey)!))
        goMenu.addItem(.separator())
        // The Finder's places and shortcuts; g and a key does the same.
        for place in BrowserWindowController.places {
            let item = goMenu.addItem(withTitle: place.title, action: #selector(BrowserWindowController.goToPlace(_:)),
                                      keyEquivalent: place.menuKey ?? "")
            item.keyEquivalentModifierMask = place.menuModifiers
            item.representedObject = place.path
        }
        goMenu.addItem(.separator())
        let activity = goMenu.addItem(withTitle: "Activity", action: #selector(BrowserWindowController.showActivityMenu(_:)),
                                      keyEquivalent: "f")
        activity.keyEquivalentModifierMask = [.command, .shift]
        let goTo = goMenu.addItem(withTitle: "Go to Folder…", action: #selector(BrowserWindowController.goToFolder(_:)),
                                  keyEquivalent: "g")
        goTo.keyEquivalentModifierMask = [.command, .shift]
        goItem.submenu = goMenu
        main.addItem(goItem)

        let helpItem = NSMenuItem()
        let helpMenu = NSMenu(title: "Help")
        helpMenu.addItem(withTitle: "Welcome to Tilde", action: #selector(AppDelegate.showWelcome(_:)), keyEquivalent: "")
        helpMenu.addItem(withTitle: "Keys…", action: #selector(AppDelegate.showKeys(_:)), keyEquivalent: "")
        helpItem.submenu = helpMenu
        main.addItem(helpItem)
        NSApp.helpMenu = helpMenu

        return main
    }
}
