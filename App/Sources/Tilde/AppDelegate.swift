import AppKit
import TildeFFI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// One index for every window: loaded from disk or scanned at launch.
    private let index = IndexService()
    private var windows: [BrowserWindowController] = []
    private lazy var indexWindow = IndexWindowController(index: index)

    func applicationDidFinishLaunching(_ notification: Notification) {
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
        let controller = BrowserWindowController(index: index)
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

    @objc func showIndex(_ sender: Any?) {
        indexWindow.showWindow(nil)
    }

    @objc func reloadConfig(_ sender: Any?) {
        let theme = Theme.reload()
        windows.removeAll { $0.window?.isVisible != true }
        for w in windows { w.applyTheme(theme) }
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
        fileMenu.addItem(withTitle: "Close Window", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        fileItem.submenu = fileMenu
        main.addItem(fileItem)

        // Standard edit actions so ⌘C/⌘V/⌘A work in the path bar.
        let editItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")
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
