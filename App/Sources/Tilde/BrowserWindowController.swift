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
    /// The generation currently shown. The engine re-sends a generation while the
    /// index fills in; those refreshes keep the selection.
    private var shownGeneration: UInt64 = 0
    private var currentFolder = ""
    /// Row to select when the next results arrive (the folder we came up from, or a
    /// revealed file).
    private var pendingSelection: String?
    private var showHidden = false
    private var previewWork: DispatchWorkItem?
    /// A `tab`/`⏎`/`/` pressed before results caught up with the typed text. Runs when
    /// they arrive, so fast typing acts on the right row.
    private var pendingAction: (() -> Void)?
    private var indexTimer: Timer?
    private var indexStatus = IndexStatus(items: 0, scanning: true)

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
        applyTheme(Theme.shared)
        window.setContentSize(NSSize(width: 1100, height: 680))
        window.center()
        window.setFrameAutosaveName("TildeBrowser")
        open(folder: engine.homeDir())
        setMode(.go)
        startIndexProgress()
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

    /// Applies fonts from the config. Called at startup and on Reload Config.
    func applyTheme(_ theme: Theme) {
        pathBar.applyTheme(theme)
        statusBar.applyTheme(theme)
        tableView.rowHeight = theme.rowHeight
        tableView.reloadData()
        if tableView.selectedRow < 0 { selectFirstResult() }
        updateChrome()
        if !theme.warnings.isEmpty { statusBar.flash(theme.warnings.joined(separator: " ")) }
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

    private func whenResultsCurrent(_ action: @escaping () -> Void) {
        if shownGeneration == latestGeneration {
            action()
        } else {
            pendingAction = action
        }
    }

    private func queryChanged() {
        engine.setQuery(text: pathBar.field.stringValue, generation: nextGeneration())
    }

    private var selectedRow: RowView? {
        results.row(tableView.selectedRow)
    }

    /// The folder a row leads into: itself for folders and symlinks to folders.
    private func folderPath(of row: RowView) -> String? {
        switch row.kind {
        case .folder:
            return row.path
        case .symlink:
            let resolved = URL(fileURLWithPath: row.path).resolvingSymlinksInPath()
            let isDir = (try? resolved.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true
            return isDir ? resolved.path : nil
        case .file, .package, .other:
            return nil
        }
    }

    /// `tab`, `/`, `l`: enter a folder, or reveal an Everywhere file in its folder.
    /// Returns false for files in the current folder.
    @discardableResult
    private func goIntoHighlighted() -> Bool {
        guard let row = selectedRow else { return false }
        if let folder = folderPath(of: row) {
            open(folder: folder)
            return true
        }
        if row.section == .everywhere {
            let url = URL(fileURLWithPath: row.path)
            open(folder: url.deletingLastPathComponent().path, select: url.lastPathComponent)
            return true
        }
        return false
    }

    /// `⏎`: enter folders, open everything else in its default app.
    private func activateHighlighted() {
        guard let row = selectedRow else { return }
        if let folder = folderPath(of: row) {
            open(folder: folder)
            return
        }
        NSWorkspace.shared.open(URL(fileURLWithPath: row.path))
        statusBar.flash("Opened \(row.name)")
    }

    /// Moves by `delta` rows, stepping over the Everywhere header.
    private func moveSelection(by delta: Int) {
        guard results.tableRows > 0 else { return }
        var row = max(tableView.selectedRow, 0) + delta
        if results.isHeader(row) { row += delta.signum() }
        select(row: min(max(row, 0), results.tableRows - 1))
    }

    private func select(row: Int) {
        var row = row
        if results.isHeader(row) { row += row + 1 < results.tableRows ? 1 : -1 }
        guard row >= 0, row < results.tableRows else { return }
        tableView.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        tableView.scrollRowToVisible(row)
    }

    private func selectFirstResult() {
        if results.tableRows > 0 { select(row: 0) } else { schedulePreview() }
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
        case 124: goIntoOrActivate(); return true // right
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
        case "l": goIntoOrActivate()
        case "g": select(row: 0)
        case "G": select(row: results.tableRows - 1)
        case "/": setMode(.go)
        case "~": open(folder: engine.homeDir())
        case let c where c.count == 1 && c.first!.isLetter:
            statusBar.flash("\(c) does nothing yet in SELECT. Press / to search.")
        default: return false
        }
        return true
    }

    private func goIntoOrActivate() {
        if !goIntoHighlighted() { activateHighlighted() }
    }

    // MARK: Chrome

    private func updateChrome() {
        pathBar.setFolder(currentFolder, home: engine.homeDir())
        pathBar.setMode(mode)
        statusBar.update(mode: mode, summary: summary())
        window?.title = currentFolder.isEmpty ? "Tilde" : FileManager.default.displayName(atPath: currentFolder)
        window?.representedURL = currentFolder.isEmpty ? nil : URL(fileURLWithPath: currentFolder)
    }

    private func summary() -> String {
        func n(_ count: Int, _ word: String) -> String { "\(count.formatted()) \(word)\(count == 1 ? "" : "s")" }
        var parts: [String] = []
        if pathBar.field.stringValue.isEmpty {
            parts.append(n(results.hereCount, "item"))
        } else {
            parts.append("\(results.hereCount.formatted()) here")
            parts.append("\(results.everywhereCount.formatted()) everywhere")
        }
        if indexStatus.scanning {
            parts.append("indexing \(Int(indexStatus.items).formatted())…")
        }
        return parts.joined(separator: " · ")
    }

    /// Polls index progress for the status bar until the first scan finishes.
    private func startIndexProgress() {
        indexTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.pollIndex() }
        }
    }

    private func pollIndex() {
        indexStatus = engine.indexStatus()
        updateChrome()
        if !indexStatus.scanning {
            indexTimer?.invalidate()
            indexTimer = nil
            statusBar.flash("Indexed \(Int(indexStatus.items).formatted()) items")
        }
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
        indexTimer?.invalidate()
        preview.close()
    }
}

// MARK: Engine events

extension BrowserWindowController: EngineEventsDelegate {
    func engineResultsReady(generation: UInt64, folder: String, hereCount: Int, everywhereCount: Int) {
        guard generation == latestGeneration else { return }
        let isRefresh = generation == shownGeneration
        let keepPath = isRefresh ? selectedRow?.path : nil
        let keepRow = tableView.selectedRow
        shownGeneration = generation
        currentFolder = folder
        results.reset(generation: generation, hereCount: hereCount, everywhereCount: everywhereCount)
        tableView.reloadData()

        if let name = pendingSelection, let i = engine.indexOf(generation: generation, name: name) {
            select(row: Int(i))
        } else if let keepPath {
            // Refreshes while the index fills in shouldn't move the selection.
            select(row: results.tableRow(forPath: keepPath) ?? min(max(keepRow, 0), results.tableRows - 1))
        } else {
            selectFirstResult()
        }
        if !isRefresh { pendingSelection = nil }
        updateChrome()
        if let action = pendingAction {
            pendingAction = nil
            action()
        }
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
            // "/" goes into the highlighted folder instead of being typed. The query
            // without it has already been sent.
            field.stringValue = String(text.dropLast())
            whenResultsCurrent { [weak self] in self?.goIntoHighlighted() }
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
            whenResultsCurrent { [weak self] in self?.activateHighlighted() }
        case #selector(NSResponder.insertTab(_:)):
            whenResultsCurrent { [weak self] in self?.goIntoHighlighted() }
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
    func numberOfRows(in tableView: NSTableView) -> Int { results.tableRows }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        if results.isHeader(row) {
            let header = tableView.makeView(withIdentifier: SectionHeaderView.identifier, owner: nil) as? SectionHeaderView
                ?? SectionHeaderView()
            header.configure(title: "Everywhere", count: results.everywhereCount)
            return header
        }
        let cell = tableView.makeView(withIdentifier: RowCellView.identifier, owner: nil) as? RowCellView ?? RowCellView()
        if let r = results.row(row) { cell.configure(with: r) }
        return cell
    }

    func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool { results.isHeader(row) }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { !results.isHeader(row) }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        results.isHeader(row) ? NSTableRowView() : AlwaysEmphasizedRowView()
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        schedulePreview()
    }
}

/// The current results as table rows: Here rows, then (if any) an Everywhere header
/// followed by Everywhere rows. Rows are fetched from the engine in chunks on demand.
@MainActor
final class ResultsCache {
    private let engine: Engine
    private var generation: UInt64 = 0
    private var rows: [Int: RowView] = [:]
    private(set) var hereCount = 0
    private(set) var everywhereCount = 0
    private let chunk = 64

    init(engine: Engine) { self.engine = engine }

    var tableRows: Int { hereCount + (everywhereCount > 0 ? everywhereCount + 1 : 0) }

    func isHeader(_ tableRow: Int) -> Bool { everywhereCount > 0 && tableRow == hereCount }

    func reset(generation: UInt64, hereCount: Int, everywhereCount: Int) {
        self.generation = generation
        self.hereCount = hereCount
        self.everywhereCount = everywhereCount
        rows.removeAll(keepingCapacity: true)
    }

    func row(_ tableRow: Int) -> RowView? {
        guard tableRow >= 0, tableRow < tableRows, !isHeader(tableRow) else { return nil }
        let index = tableRow < hereCount ? tableRow : tableRow - 1
        if let r = rows[index] { return r }
        let start = (index / chunk) * chunk
        for (offset, r) in engine.rows(generation: generation, start: UInt32(start), count: UInt32(chunk)).enumerated() {
            rows[start + offset] = r
        }
        return rows[index]
    }

    /// Table row showing `path`, searching the first few hundred rows.
    func tableRow(forPath path: String) -> Int? {
        (0..<min(tableRows, 400)).first { row($0)?.path == path }
    }
}
