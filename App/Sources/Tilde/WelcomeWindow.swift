import AppKit
import SwiftUI
import TildeFFI

/// First launch (and Help › Welcome to Tilde): permissions, the editor, and the three
/// modes. Everything here can be changed later in Settings.
@MainActor
final class WelcomeWindowController: NSWindowController {
    private static let seenKey = "TildeWelcomeSeen"

    /// Whether the welcome has been shown before.
    static var seen: Bool {
        get { UserDefaults.standard.bool(forKey: seenKey) }
        set { UserDefaults.standard.set(newValue, forKey: seenKey) }
    }

    init(onChange: @escaping @MainActor () -> Void) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 620, height: 640),
            styleMask: [.titled, .closable, .fullSizeContentView], backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isMovableByWindowBackground = true
        super.init(window: window)
        let model = WelcomeModel(onChange: onChange)
        window.contentViewController = NSHostingController(rootView: WelcomeView(model: model) { [weak self] in
            Self.seen = true
            self?.close()
        })
        window.setContentSize(NSSize(width: 620, height: 640))
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }
}

@MainActor
@Observable
final class WelcomeModel {
    var fullDiskAccess = Permissions.hasFullDiskAccess
    /// After "Ask now": which protected folders can be read.
    var folders: [(String, Bool)]?
    var asking = false
    var editorNvim = loadSettings().editorNvim
    var nvim: (found: Bool, message: String)?
    private let onChange: @MainActor () -> Void
    @ObservationIgnored private var timer: Timer?

    init(onChange: @escaping @MainActor () -> Void) {
        self.onChange = onChange
        // Notices Full Disk Access being granted in System Settings.
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.fullDiskAccess = Permissions.hasFullDiskAccess }
        }
        Task {
            nvim = await Task.detached {
                let status = findNvim(configured: loadSettings().nvimPath)
                return (status.found, status.message)
            }.value
        }
    }

    func askForFolders() {
        asking = true
        Task {
            folders = await Task.detached { Permissions.askForFolders() }.value
            asking = false
        }
    }

    func setEditor(nvim: Bool) {
        editorNvim = nvim
        try? setSetting(key: "edit.editor", value: .text(value: nvim ? "nvim" : "builtin"))
        onChange()
    }

    func stop() {
        timer?.invalidate()
    }
}

struct WelcomeView: View {
    @Bindable var model: WelcomeModel
    let done: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Welcome to Tilde").font(.largeTitle.bold())
                Text("A keyboard-first way through your files. A few things first; all of them can be changed later in Settings (⌘,).")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            step(1, "Full Disk Access") {
                Text("Lets Tilde search and open every folder, including ones macOS protects. Without it Tilde still works, and says which folders it can't read.")
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    if model.fullDiskAccess {
                        Label("Granted", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                    } else {
                        Button("Open Privacy & Security") { Permissions.openFullDiskAccessSettings() }
                        Text("then turn on \(Permissions.grantee)").foregroundStyle(.secondary)
                    }
                }
                if Permissions.grantee != "Tilde" {
                    Text("Tilde is running from \(Permissions.grantee), so macOS asks about \(Permissions.grantee) (restart it after granting).")
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            step(2, "Desktop, Documents and Downloads") {
                Text(model.fullDiskAccess
                     ? "Full Disk Access covers these too."
                     : "macOS asks once before an app reads these. Ask now, rather than in the middle of a search.")
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 14) {
                    if let folders = model.folders {
                        ForEach(folders, id: \.0) { name, ok in
                            Label(name, systemImage: ok ? "checkmark.circle.fill" : "xmark.circle.fill")
                                .foregroundStyle(ok ? .green : .orange)
                        }
                        if folders.contains(where: { !$0.1 }) {
                            Button("Files & Folders settings") { Permissions.openFilesAndFoldersSettings() }
                        }
                    } else if !model.fullDiskAccess {
                        Button(model.asking ? "Asking…" : "Ask now") { model.askForFolders() }.disabled(model.asking)
                    }
                }
            }

            step(3, "Editing folders as text") {
                Text("In EDIT mode a folder becomes text: change a name to rename, delete a line to trash, type a path to move. Use the built-in editor or your Neovim.")
                    .fixedSize(horizontal: false, vertical: true)
                Picker("", selection: Binding(get: { model.editorNvim }, set: { model.setEditor(nvim: $0) })) {
                    Text("Built-in").tag(false)
                    Text("Neovim").tag(true)
                }
                .pickerStyle(.radioGroup)
                .labelsHidden()
                if let nvim = model.nvim {
                    Text(nvim.message).font(.caption).foregroundStyle(nvim.found ? Color.secondary : Color.orange)
                }
            }

            step(4, "Three modes") {
                Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                    modeRow("GO", "Type to search here and everywhere. \(go(.enter)) into a folder, \(go(.open)) opens, \(go(.selectMode)) to SELECT.")
                    modeRow("SELECT", "\(sel(.down)) \(sel(.up)) move, \(sel(.shelf)) collect, \(sel(.move)) move, \(sel(.copy)) copy, \(sel(.trash)) trash, \(sel(.tag)) tag, \(sel(.undo)) undo, \(sel(.search)) search.")
                    modeRow("EDIT", "\(sel(.edit)) edits the folder as text, \(sel(.rename)) renames. ⌘S applies, ⌘. cancels.")
                }
                Text("\(sel(.activity)) shows recent downloads and screenshots. Keys can be changed in Settings › Keys.")
                    .font(.caption).foregroundStyle(.secondary)
                Text("\(go(.focusBar)) focuses the search bar; \(go(.focusPreview)) focuses the preview.")
                    .font(.caption).foregroundStyle(.secondary)
            }

            Spacer(minLength: 0)
            HStack {
                Spacer()
                Button("Start Using Tilde") {
                    model.stop()
                    done()
                }
                .keyboardShortcut(.defaultAction)
                .controlSize(.large)
            }
        }
        .padding(28)
        .frame(width: 620)
    }

    private func go(_ a: KeyAction) -> String { Keymap.shared.key(a, in: .go) }
    private func sel(_ a: KeyAction) -> String { Keymap.shared.key(a, in: .select) }

    private func step(_ n: Int, _ title: String, @ViewBuilder content: () -> some View) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Text("\(n)")
                .font(.headline)
                .frame(width: 24, height: 24)
                .background(Circle().fill(.quaternary))
            VStack(alignment: .leading, spacing: 6) {
                Text(title).font(.headline)
                content()
            }
        }
    }

    private func modeRow(_ mode: String, _ text: String) -> some View {
        GridRow {
            Text(mode).font(.system(.caption, design: .monospaced).bold())
            Text(text).fixedSize(horizontal: false, vertical: true).frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
