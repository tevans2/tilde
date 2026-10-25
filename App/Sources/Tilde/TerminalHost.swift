import AppKit
import SwiftTerm
import TildeFFI

/// A terminal view running one program: Neovim for EDIT (docs/SPEC.md §6.9). SwiftTerm
/// for now; libghostty can sit behind the same protocol once it has an embedding API.
@MainActor
protocol TerminalHost: AnyObject {
    var view: NSView { get }
    var isRunning: Bool { get }
    /// The program exited (exit code, if it exited normally).
    var onExit: ((Int32?) -> Void)? { get set }
    /// Starts `program`, sized to the view, with `env` added to the app's environment.
    func start(program: String, args: [String], env: [String: String], directory: String?)
    func focus()
    func applyTheme(_ theme: Theme)
    func terminate()
}

@MainActor
final class SwiftTermHost: NSObject, TerminalHost, LocalProcessTerminalViewDelegate {
    private let terminal = LocalProcessTerminalView(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
    private(set) var isRunning = false
    var onExit: ((Int32?) -> Void)?

    var view: NSView { terminal }

    override init() {
        super.init()
        terminal.processDelegate = self
        // Option types characters (é, ü) as on the rest of the Mac.
        terminal.optionAsMetaKey = false
        // nvim has no scrollback to scroll.
        for case let scroller as NSScroller in terminal.subviews { scroller.isHidden = true }
    }

    func start(program: String, args: [String], env: [String: String], directory: String?) {
        var vars = ProcessInfo.processInfo.environment
        vars.merge(env) { _, new in new }
        isRunning = true
        terminal.startProcess(
            executable: program, args: args, environment: vars.map { "\($0.key)=\($0.value)" },
            execName: (program as NSString).lastPathComponent, currentDirectory: directory)
    }

    func focus() {
        terminal.window?.makeFirstResponder(terminal)
    }

    func applyTheme(_ theme: Theme) {
        terminal.font = theme.list
        terminal.nativeBackgroundColor = theme.colors.background
        terminal.nativeForegroundColor = theme.colors.text
        terminal.caretColor = theme.colors.text
        terminal.selectedTextBackgroundColor = theme.colors.selection
    }

    func terminate() {
        guard isRunning else { return }
        terminal.terminate()
    }

    // MARK: LocalProcessTerminalViewDelegate (called on the main queue)

    nonisolated func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) {}
    nonisolated func setTerminalTitle(source: LocalProcessTerminalView, title: String) {}
    nonisolated func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

    nonisolated func processTerminated(source: TerminalView, exitCode: Int32?) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                guard self.isRunning else { return }
                self.isRunning = false
                self.onExit?(exitCode)
            }
        }
    }
}

@MainActor
protocol NvimEventsDelegate: AnyObject {
    func nvimReady()
    func nvimFailed(_ message: String)
    func nvimPreview(_ preview: EditPreview)
    func nvimApply(_ text: String)
    func nvimCancelled()
    func nvimDisconnected()
}

/// Forwards Neovim's callbacks (from Rust threads) to the main thread, in order.
final class NvimEvents: NvimListener, @unchecked Sendable {
    // Only read and written on the main thread.
    nonisolated(unsafe) weak var delegate: NvimEventsDelegate?

    private func main(_ body: @escaping @MainActor (NvimEventsDelegate) -> Void) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                if let d = self.delegate { body(d) }
            }
        }
    }

    func nvimReady() { main { $0.nvimReady() } }
    func nvimFailed(message: String) { main { $0.nvimFailed(message) } }
    func nvimPreview(preview: EditPreview) { main { $0.nvimPreview(preview) } }
    func nvimApply(text: String) { main { $0.nvimApply(text) } }
    func nvimCancelled() { main { $0.nvimCancelled() } }
    func nvimDisconnected() { main { $0.nvimDisconnected() } }
}
