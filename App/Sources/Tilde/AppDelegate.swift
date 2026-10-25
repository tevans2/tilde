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

    func applicationDidFinishLaunching(_ notification: Notification) {
        opsEvents.delegate = self
        NSApp.appearance = Theme.shared.appearance
        NSApp.mainMenu = MainMenu.build()
        newWindow(nil)
        NSApp.activate()
        #if DEBUG
        DebugSnapshot.runIfRequested()
        #endif
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    func applicationWillTerminate(_ notification: Notification) {
        // So the next launch loads instead of scanning.
        index.save()
    }

    @objc func newWindow(_ sender: Any?) {
        let controller = BrowserWindowController(index: index, ops: ops)
        windows.append(controller)
        controller.showWindow(nil)
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

    @objc func reloadConfig(_ sender: Any?) {
        let theme = Theme.reload()
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
        appMenu.addItem(withTitle: "Open Config File…", action: #selector(AppDelegate.openConfig(_:)), keyEquivalent: ",")
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
        fileMenu.addItem(withTitle: "Apply Edits", action: #selector(BrowserWindowController.applyEdits(_:)), keyEquivalent: "s")
        fileMenu.addItem(withTitle: "Cancel Edit", action: #selector(BrowserWindowController.cancelEdits(_:)), keyEquivalent: ".")
        newFolder.keyEquivalentModifierMask = [.command, .shift]
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
        editMenu.addItem(.separator())
        editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = editMenu
        main.addItem(editItem)

        let viewItem = NSMenuItem()
        let viewMenu = NSMenu(title: "View")
        let hidden = viewMenu.addItem(withTitle: "Show Hidden Files", action: #selector(BrowserWindowController.toggleHidden(_:)), keyEquivalent: ".")
        hidden.keyEquivalentModifierMask = [.command, .shift]
        viewMenu.addItem(.separator())
        let indexItem = viewMenu.addItem(withTitle: "Search Index…", action: #selector(AppDelegate.showIndex(_:)), keyEquivalent: "i")
        indexItem.keyEquivalentModifierMask = [.command, .shift]
        viewItem.submenu = viewMenu
        main.addItem(viewItem)

        return main
    }
}
