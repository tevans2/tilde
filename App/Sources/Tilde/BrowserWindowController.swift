import AppKit
import Quartz
import TildeFFI

/// GO: typing searches. SELECT: letters are actions. (EDIT arrives in M4.)
enum Mode {
    case go
    case select
}

/// One browser window: path bar, results, preview, status bar.
@MainActor
final class BrowserWindowController: NSWindowController, NSWindowDelegate {
    private let events = EngineEvents()
    private let engine: Engine
    private let results: ResultsCache

    private let pathBar = PathBar()
    private let tableView = ResultsTableView()
    private let preview = QLPreviewView(frame: .zero, style: .normal)!
    private let statusBar = StatusBar()

    private var mode: Mode = .go
    /// The newest generation sent to the engine. Older results are ignored.
    private var latestGeneration: UInt64 = 0
    private var currentFolder = ""
    /// Row to select when the next results arrive (the folder we just came up from).
    private var pendingSelection: String?
    private var showHidden = false
    private var previewWork: DispatchWorkItem?

    init() {
        engine = Engine(listener: events)
        results = ResultsCache(engine: engine)
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1100, height: 680),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.minSize = NSSize(width: 640, height: 360)
        window.tabbingMode = .disallowed
        super.init(window: window)
        window.delegate = self
        events.delegate = self
        buildLayout(in: window)
        window.setContentSize(NSSize(width: 1100, height: 680))
        window.center()
        window.setFrameAutosaveName("TildeBrowser")
        open(folder: engine.homeDir())
        setMode(.go)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    // MARK: Layout

    private func buildLayout(in window: NSWindow) {
        let content = NSView()
        window.contentView = content

        pathBar.field.delegate = self
        pathBar.field.onFocus = { [weak self] in self?.modeDidFocus(.go) }

        tableView.dataSource = self
        tableView.delegate = self
        tableView.target = self
        tableView.doubleAction = #selector(doubleClicked)
        tableView.onFocus = { [weak self] in self?.modeDidFocus(.select) }
        tableView.onKey = { [weak self] event in self?.handleSelectKey(event) ?? false }

        let scroll = NSScrollView()
        scroll.documentView = tableView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false
        scroll.contentInsets = NSEdgeInsets(top: 4, left: 0, bottom: 4, right: 0)

        preview.autostarts = true
        preview.shouldCloseWithWindow = true

        let previewSeparator = NSBox()
        previewSeparator.boxType = .separator
        let topSeparator = NSBox()
        topSeparator.boxType = .separator

        for v in [pathBar, topSeparator, scroll, previewSeparator, preview, statusBar] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(v)
        }

        NSLayoutConstraint.activate([
            // Without these, Auto Layout shrinks the window to the bars' fitting height.
            content.widthAnchor.constraint(greaterThanOrEqualToConstant: 640),
            content.heightAnchor.constraint(greaterThanOrEqualToConstant: 360),

            pathBar.topAnchor.constraint(equalTo: content.topAnchor),
            pathBar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            pathBar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            pathBar.heightAnchor.constraint(equalToConstant: 46),

            topSeparator.topAnchor.constraint(equalTo: pathBar.bottomAnchor),
            topSeparator.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            topSeparator.trailingAnchor.constraint(equalTo: content.trailingAnchor),

            scroll.topAnchor.constraint(equalTo: topSeparator.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            scroll.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            scroll.trailingAnchor.constraint(equalTo: previewSeparator.leadingAnchor),

            previewSeparator.topAnchor.constraint(equalTo: topSeparator.bottomAnchor),
            previewSeparator.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            previewSeparator.widthAnchor.constraint(equalToConstant: 1),
            previewSeparator.trailingAnchor.constraint(equalTo: preview.leadingAnchor),

            preview.topAnchor.constraint(equalTo: topSeparator.bottomAnchor),
            preview.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            preview.bottomAnchor.constraint(equalTo: statusBar.topAnchor),
            preview.widthAnchor.constraint(equalTo: content.widthAnchor, multiplier: 0.3),

            statusBar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            statusBar.heightAnchor.constraint(equalToConstant: 28),
        ])
    }

    // MARK: Modes

    private func setMode(_ newMode: Mode) {
        mode = newMode
        switch newMode {
        case .go:
            window?.makeFirstResponder(pathBar.field)
            pathBar.field.currentEditor()?.selectedRange = NSRange(location: pathBar.field.stringValue.utf16.count, length: 0)
        case .select:
            window?.makeFirstResponder(tableView)
        }
        updateChrome()
    }

    /// Clicking into the bar or the list switches mode too.
    private func modeDidFocus(_ focused: Mode) {
        guard mode != focused else { return }
        mode = focused
        updateChrome()
    }

    // MARK: Navigation

    private func nextGeneration() -> UInt64 {
        latestGeneration += 1
        return latestGeneration
    }

    private func open(folder: String, select: String? = nil) {
        pendingSelection = select
        pathBar.field.stringValue = ""
        engine.openFolder(path: folder, generation: nextGeneration())
        engine.setQuery(text: "", generation: nextGeneration())
    }

    private func goUp() {
        guard currentFolder != "/" else { return }
        let url = URL(fileURLWithPath: currentFolder)
        open(folder: url.deletingLastPathComponent().path, select: url.lastPathComponent)
    }

    private func queryChanged() {
        engine.setQuery(text: pathBar.field.stringValue, generation: nextGeneration())
    }

    private var selectedRow: RowView? {
        let i = tableView.selectedRow
        return i >= 0 ? results.row(i) : nil
    }

    /// Enters the highlighted folder (following symlinks). Returns false for files.
    @discardableResult
    private func enterHighlighted() -> Bool {
        guard let row = selectedRow else { return false }
        switch row.kind {
        case .folder:
            open(folder: row.path)
            return true
        case .symlink:
            let resolved = URL(fileURLWithPath: row.path).resolvingSymlinksInPath()
            if (try? resolved.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true {
                open(folder: resolved.path)
                return true
            }
            return false
        case .file, .package, .other:
            return false
        }
    }

    /// Enters folders, opens everything else in its default app.
    private func activateHighlighted() {
        guard let row = selectedRow else { return }
        if enterHighlighted() { return }
        NSWorkspace.shared.open(URL(fileURLWithPath: row.path))
        statusBar.flash("Opened \(row.name)")
    }

    private func moveSelection(by delta: Int) {
        guard results.count > 0 else { return }
        let current = max(tableView.selectedRow, 0)
        select(row: min(max(current + delta, 0), results.count - 1))
    }

    private func select(row: Int) {
        guard row >= 0, row < results.count else { return }
        tableView.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        tableView.scrollRowToVisible(row)
    }

    @objc private func doubleClicked() {
        if tableView.clickedRow >= 0 { activateHighlighted() }
    }

    @objc func toggleHidden(_ sender: Any?) {
        showHidden.toggle()
        engine.setShowHidden(show: showHidden, generation: nextGeneration())
        statusBar.flash(showHidden ? "Showing hidden files" : "Hiding hidden files")
    }

    // MARK: SELECT keys

    private func handleSelectKey(_ event: NSEvent) -> Bool {
        if event.modifierFlags.contains(.command) { return false }
        switch event.keyCode {
        case 125: moveSelection(by: 1); return true // down
        case 126: moveSelection(by: -1); return true // up
        case 123: goUp(); return true // left
        case 124: activateHighlightedFolderOrFile(); return true // right
        case 36, 76: activateHighlighted(); return true // return, enter
        case 53: // escape: clear the filter
            if !pathBar.field.stringValue.isEmpty {
                pathBar.field.stringValue = ""
                queryChanged()
            }
            return true
        case 51: goUp(); return true // delete
        default: break
        }
        switch event.charactersIgnoringModifiers ?? "" {
        case "j": moveSelection(by: 1)
        case "k": moveSelection(by: -1)
        case "h": goUp()
        case "l": activateHighlightedFolderOrFile()
        case "g": select(row: 0)
        case "G": select(row: results.count - 1)
        case "/": setMode(.go)
        case "~": open(folder: engine.homeDir())
        case let c where c.count == 1 && c.first!.isLetter:
            statusBar.flash("\(c) does nothing yet in SELECT. Press / to search.")
        default: return false
        }
        return true
    }

    private func activateHighlightedFolderOrFile() {
        if !enterHighlighted() { activateHighlighted() }
    }

    // MARK: Chrome

    private func updateChrome() {
        pathBar.setFolder(currentFolder, home: engine.homeDir())
        pathBar.setMode(mode)
        statusBar.update(mode: mode, count: results.count, filter: pathBar.field.stringValue)
        window?.title = currentFolder.isEmpty ? "Tilde" : FileManager.default.displayName(atPath: currentFolder)
        window?.representedURL = currentFolder.isEmpty ? nil : URL(fileURLWithPath: currentFolder)
    }

    private func schedulePreview() {
        previewWork?.cancel()
        let path = selectedRow?.path
        let work = DispatchWorkItem { [weak self] in
            self?.preview.previewItem = path.map { URL(fileURLWithPath: $0) as NSURL }
        }
        previewWork = work
        // Short delay so holding j doesn't render every file on the way.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: work)
    }

    func windowWillClose(_ notification: Notification) {
        preview.close()
    }
}

// MARK: Engine events

extension BrowserWindowController: EngineEventsDelegate {
    func engineResultsReady(generation: UInt64, folder: String, count: Int) {
        guard generation == latestGeneration else { return }
        currentFolder = folder
        results.reset(generation: generation, count: count)
        tableView.reloadData()
        var row = 0
        if let name = pendingSelection, let i = engine.indexOf(generation: generation, name: name) {
            row = Int(i)
        }
        pendingSelection = nil
        if count > 0 { select(row: row) } else { schedulePreview() }
        updateChrome()
    }

    func engineFolderFailed(generation: UInt64, folder: String, message: String) {
        let name = (folder as NSString).lastPathComponent
        let hint = message.contains("ermission") ? " Tilde may need Full Disk Access." : ""
        statusBar.flash("Can't open \(name): \(message).\(hint)")
    }
}

// MARK: GO keys (path bar)

extension BrowserWindowController: NSTextFieldDelegate {
    func controlTextDidChange(_ obj: Notification) {
        let field = pathBar.field
        let text = field.stringValue
        if text == "~" {
            open(folder: engine.homeDir())
        } else if text.hasSuffix("/") {
            // "/" goes into the highlighted folder instead of being typed.
            field.stringValue = String(text.dropLast())
            if !enterHighlighted() { queryChanged() }
        } else {
            queryChanged()
        }
        updateChrome()
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveDown(_:)):
            moveSelection(by: 1)
        case #selector(NSResponder.moveUp(_:)):
            moveSelection(by: -1)
        case #selector(NSResponder.insertNewline(_:)):
            activateHighlighted()
        case #selector(NSResponder.insertTab(_:)):
            enterHighlighted()
        case #selector(NSResponder.cancelOperation(_:)):
            setMode(.select)
        case #selector(NSResponder.deleteBackward(_:)) where pathBar.field.stringValue.isEmpty:
            goUp()
        default:
            return false
        }
        return true
    }
}

// MARK: Table

extension BrowserWindowController: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { results.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = tableView.makeView(withIdentifier: RowCellView.identifier, owner: nil) as? RowCellView ?? RowCellView()
        if let r = results.row(row) { cell.configure(with: r) }
        return cell
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        AlwaysEmphasizedRowView()
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        schedulePreview()
    }
}

/// Rows of the current results, fetched from the engine in chunks as the table asks.
@MainActor
final class ResultsCache {
    private let engine: Engine
    private var generation: UInt64 = 0
    private var rows: [Int: RowView] = [:]
    private(set) var count = 0
    private let chunk = 64

    init(engine: Engine) { self.engine = engine }

    func reset(generation: UInt64, count: Int) {
        self.generation = generation
        self.count = count
        rows.removeAll(keepingCapacity: true)
    }

    func row(_ index: Int) -> RowView? {
        if let r = rows[index] { return r }
        guard index >= 0, index < count else { return nil }
        let start = (index / chunk) * chunk
        for (offset, r) in engine.rows(generation: generation, start: UInt32(start), count: UInt32(chunk)).enumerated() {
            rows[start + offset] = r
        }
        return rows[index]
    }
}
