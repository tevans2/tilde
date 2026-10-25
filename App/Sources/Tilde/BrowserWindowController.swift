import AppKit
import Quartz
import TildeFFI

/// GO: typing searches. SELECT: letters are actions. EDIT: the folder is text.
enum Mode {
    case go
    case select
    case edit
}

/// One browser window: path bar, results, preview, status bar.
@MainActor
final class BrowserWindowController: NSWindowController, NSWindowDelegate {
    private let events = EngineEvents()
    private let index: IndexService
    private let ops: FileOps
    /// Collected with `x`. Actions apply to these when there are any, otherwise to the
    /// highlighted row.
    private var shelf: [String] = []
    private var shelfSet: Set<String> = []
    private let shelfBar = ShelfBar()
    private var shelfHeight: NSLayoutConstraint!
    /// Operations started from this window: what to say when each finishes.
    private var myOperations: [UInt64: String] = [:]
    private var runningLabels: [UInt64: String] = [:]
    private var overlay: PromptOverlay?
    private let listScroll = NSScrollView()
    // EDIT mode
    private let editor = FolderEditor()
    private let changesView = ChangesView()
    private var editSession: EditSession?
    private var editBaseline = ""
    private var editEscArmed = false
    private var pendingEditOp: UInt64?
    private var editPreviewWork: DispatchWorkItem?
    private var keyMonitor: Any?
    private let engine: Engine
    private let results: ResultsCache

    private let pathBar = PathBar()
    private let tableView = ResultsTableView()
    private let preview = PreviewPane()
    private let previewSeparator = NSBox()
    private let topSeparator = NSBox()
    private var previewWidth: NSLayoutConstraint?
    private let statusBar = StatusBar()

    private var mode: Mode = .go
    /// The newest generation sent to the engine. Older results are ignored.
    private var latestGeneration: UInt64 = 0
    /// The generation currently shown. The engine re-sends a generation while the
    /// index fills in; those refreshes keep the selection.
    private var shownGeneration: UInt64 = 0
    private var currentFolder = ""
    /// Activity is showing (or has been asked for). `currentFolder` is where it was opened
    /// from, and where `⌫` / `h` / `@` go back to.
    private var inActivity = false
    private var activityFilter = ActivityFilter(origin: .all, inbox: false)
    /// From the latest results: inbox count and date headings.
    private var activity: ActivityInfo?
    /// Row to select when the next results arrive (the folder we came up from, or a
    /// revealed file).
    private var pendingSelection: String?
    private var showHidden = false
    private var previewWork: DispatchWorkItem?
    /// A `tab`/`⏎`/`/` pressed before results caught up with the typed text. Runs when
    /// they arrive, so fast typing acts on the right row.
    private var pendingAction: (() -> Void)?
    private var indexTimer: Timer?
    private var indexStatus = IndexStatus(state: .scanning, items: 0, capped: false)

    init(index: IndexService, ops: FileOps) {
        self.index = index
        self.ops = ops
        engine = Engine(listener: events, index: index)
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
        var start = engine.homeDir()
        #if DEBUG
        // Development aid: start somewhere else (used with the snapshot hook).
        if let dir = ProcessInfo.processInfo.environment["TILDE_START"] { start = dir }
        #endif
        open(folder: start)
        setMode(.go)
        startIndexProgress()
        // ⌃j ⌃k ⌃h ⌃l navigate in every mode. Caught before the search field, whose text
        // editing would otherwise treat ⌃h as delete and ⌃k as kill-line.
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            let handled = MainActor.assumeIsolated { self?.handleControlNavigation(event) ?? false }
            return handled ? nil : event
        }
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

        let scroll = listScroll
        scroll.documentView = tableView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false
        scroll.contentInsets = NSEdgeInsets(top: 4, left: 0, bottom: 4, right: 0)

        // Plain boxes filled with the theme's border colour (system separators can't be
        // recoloured).
        for box in [previewSeparator, topSeparator] {
            box.boxType = .custom
            box.borderWidth = 0
        }

        for v in [pathBar, topSeparator, scroll, previewSeparator, preview, shelfBar, statusBar, editor, changesView] as [NSView] {
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
            topSeparator.heightAnchor.constraint(equalToConstant: 1),

            scroll.topAnchor.constraint(equalTo: topSeparator.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            scroll.bottomAnchor.constraint(equalTo: shelfBar.topAnchor),
            scroll.trailingAnchor.constraint(equalTo: previewSeparator.leadingAnchor),

            previewSeparator.topAnchor.constraint(equalTo: topSeparator.bottomAnchor),
            previewSeparator.bottomAnchor.constraint(equalTo: shelfBar.topAnchor),
            previewSeparator.widthAnchor.constraint(equalToConstant: 1),
            previewSeparator.trailingAnchor.constraint(equalTo: preview.leadingAnchor),

            preview.topAnchor.constraint(equalTo: topSeparator.bottomAnchor),
            preview.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            preview.bottomAnchor.constraint(equalTo: shelfBar.topAnchor),

            shelfBar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            shelfBar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            shelfBar.bottomAnchor.constraint(equalTo: statusBar.topAnchor),

            statusBar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            statusBar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            statusBar.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            statusBar.heightAnchor.constraint(equalToConstant: 28),

            // EDIT mode swaps these in over the list and the preview.
            editor.topAnchor.constraint(equalTo: scroll.topAnchor),
            editor.bottomAnchor.constraint(equalTo: scroll.bottomAnchor),
            editor.leadingAnchor.constraint(equalTo: scroll.leadingAnchor),
            editor.trailingAnchor.constraint(equalTo: scroll.trailingAnchor),
            changesView.topAnchor.constraint(equalTo: preview.topAnchor),
            changesView.bottomAnchor.constraint(equalTo: preview.bottomAnchor),
            changesView.leadingAnchor.constraint(equalTo: preview.leadingAnchor),
            changesView.trailingAnchor.constraint(equalTo: preview.trailingAnchor),
        ])
        editor.isHidden = true
        changesView.isHidden = true
        editor.onChange = { [weak self] in self?.editChanged() }
        editor.onApply = { [weak self] in self?.applyEdit() }
        editor.onCancel = { [weak self] in self?.cancelEdit() }
        shelfHeight = shelfBar.heightAnchor.constraint(equalToConstant: 0)
        shelfHeight.isActive = true
        shelfBar.isHidden = true
    }

    /// Applies fonts and preview settings from the config. Called at startup and on
    /// Reload Config.
    func applyTheme(_ theme: Theme) {
        window?.backgroundColor = theme.colors.background
        previewSeparator.fillColor = theme.colors.border
        topSeparator.fillColor = theme.colors.border
        updateSidePane(theme)
        pathBar.applyTheme(theme)
        statusBar.applyTheme(theme)
        tableView.rowHeight = theme.rowHeight
        tableView.reloadData()
        if tableView.selectedRow < 0 { selectFirstResult() }
        updateChrome()
        if !theme.warnings.isEmpty { statusBar.flash(theme.warnings.joined(separator: " ")) }
    }

    /// Preview width and visibility from the config. While editing, the side pane always
    /// shows (it lists the pending changes).
    private func updateSidePane(_ theme: Theme) {
        guard let content = window?.contentView else { return }
        // Multipliers can't change, so the width constraint is replaced.
        previewWidth?.isActive = false
        let editing = mode == .edit
        let visible = theme.previewVisible || editing
        let share = theme.previewVisible ? CGFloat(theme.previewWidth) : (editing ? 0.4 : 0)
        previewWidth = preview.widthAnchor.constraint(equalTo: content.widthAnchor, multiplier: share)
        previewWidth?.isActive = true
        preview.isHidden = !theme.previewVisible || editing
        previewSeparator.isHidden = !visible
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
        case .edit:
            editor.focus()
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
        inActivity = false
        pendingSelection = select
        pathBar.field.stringValue = ""
        engine.openFolder(path: folder, generation: nextGeneration())
        engine.setQuery(text: "", generation: nextGeneration())
    }

    /// `@`: show Activity, or go back to the folder it was opened from.
    private func toggleActivity() {
        if inActivity { return open(folder: currentFolder) }
        inActivity = true
        pendingSelection = nil
        pathBar.field.stringValue = ""
        engine.openActivity(filter: activityFilter, generation: nextGeneration())
        engine.setQuery(text: "", generation: nextGeneration())
        updateChrome()
    }

    /// `1`–`5` and `i` in Activity.
    private func setActivityFilter(origin: OriginFilter? = nil, toggleInbox: Bool = false) {
        if let origin { activityFilter.origin = origin }
        if toggleInbox { activityFilter.inbox.toggle() }
        engine.openActivity(filter: activityFilter, generation: nextGeneration())
    }

    private func goUp() {
        if inActivity { return open(folder: currentFolder) }
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
        if row.section == .everywhere || row.origin != nil {
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
        if event.modifierFlags.contains(.control) {
            if event.charactersIgnoringModifiers == "r" { redo(); return true }
            return false
        }
        switch event.keyCode {
        case 125: moveSelection(by: 1); return true // down
        case 126: moveSelection(by: -1); return true // up
        case 123: goUp(); return true // left
        case 124: goIntoOrActivate(); return true // right
        case 36, 76: activateHighlighted(); return true // return, enter
        case 53: // escape: clear the filter, then the shelf
            if !pathBar.field.stringValue.isEmpty {
                pathBar.field.stringValue = ""
                queryChanged()
            } else if !shelf.isEmpty {
                setShelf([])
            }
            return true
        case 49: toggleShelf(); return true // space
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
        case "@": toggleActivity()
        case "1" where inActivity: setActivityFilter(origin: .all)
        case "2" where inActivity: setActivityFilter(origin: .downloads)
        case "3" where inActivity: setActivityFilter(origin: .screenshots)
        case "4" where inActivity: setActivityFilter(origin: .edited)
        case "5" where inActivity: setActivityFilter(origin: .airDrop)
        case "i" where inActivity: setActivityFilter(toggleInbox: true)
        case "e" where inActivity, "o" where inActivity, "n" where inActivity:
            statusBar.flash("Activity isn't a folder. tab reveals the item in its folder.")
        case "~": open(folder: engine.homeDir())
        case "X": hideHighlightedFromSearch()
        case "x": toggleShelf()
        case "a": toggleShelfAll()
        case "m": pickDestination(copy: false)
        case "c": pickDestination(copy: true)
        case "d": trashTargets()
        case "y": copyPaths()
        case "z": compressTargets()
        case "r":
            // Everywhere results live in other folders: rename those with a prompt.
            if selectedRow?.section == .everywhere || inActivity {
                renameHighlighted()
            } else {
                enterEdit(focus: selectedRow?.name, selectName: true, newLine: false, quick: true)
            }
        case "e": enterEdit(focus: selectedRow?.name, selectName: false, newLine: false, quick: false)
        case "o": enterEdit(focus: selectedRow?.name, selectName: false, newLine: true, quick: true)
        case "n": newFolder(nil)
        case "u": undo()
        case "U": redo()
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
        if inActivity {
            pathBar.setActivity(activityFilter)
        } else {
            pathBar.setFolder(currentFolder, home: engine.homeDir())
        }
        pathBar.setMode(mode)
        statusBar.update(mode: mode, summary: summary(), inActivity: inActivity)
        if inActivity {
            window?.title = "Activity"
            window?.representedURL = nil
        } else {
            window?.title = currentFolder.isEmpty ? "Tilde" : FileManager.default.displayName(atPath: currentFolder)
            window?.representedURL = currentFolder.isEmpty ? nil : URL(fileURLWithPath: currentFolder)
        }
    }

    private func summary() -> String {
        func n(_ count: Int, _ word: String) -> String { "\(count.formatted()) \(word)\(count == 1 ? "" : "s")" }
        var parts: [String] = []
        if pathBar.field.stringValue.isEmpty {
            parts.append(n(results.hereCount, "item"))
            if inActivity, let activity {
                parts.append(activity.inboxCount == 0 ? "inbox zero" : "\(activity.inboxCount.formatted()) in inbox")
            }
        } else {
            parts.append("\(results.hereCount.formatted()) here")
            parts.append("\(results.everywhereCount.formatted()) everywhere")
        }
        switch indexStatus.state {
        case .scanning: parts.append("indexing \(Int(indexStatus.items).formatted())…")
        case .cleared, .disabled: parts.append("search index off · ⌘⇧I")
        case .ready where indexStatus.capped: parts.append("index full · ⌘⇧I")
        case .ready: break
        }
        return parts.joined(separator: " · ")
    }

    /// Polls the shared index for the status bar (cheap: a lock and two counters).
    private func startIndexProgress() {
        indexStatus = index.status()
        indexTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.pollIndex() }
        }
    }

    private func pollIndex() {
        let previous = indexStatus
        indexStatus = index.status()
        guard indexStatus != previous else { return }
        updateChrome()
        if previous.state == .scanning, indexStatus.state == .ready {
            let note = indexStatus.capped ? " (limit reached; see ⌘⇧I)" : ""
            statusBar.flash("Indexed \(Int(indexStatus.items).formatted()) items\(note)")
        }
    }

    /// `X` in SELECT: leave the highlighted item out of search for good (adds it to
    /// `[index] hide` in the config and rebuilds).
    private func hideHighlightedFromSearch() {
        guard let row = selectedRow else { return }
        do {
            try index.addExclusion(path: row.path, hide: true)
            statusBar.flash("Hid \(row.name) from search. Undo in the config (⌘,), then Rebuild (⌘⇧I).")
        } catch {
            statusBar.flash("Couldn't update the config: \(error.localizedDescription)")
        }
    }

    private func schedulePreview() {
        previewWork?.cancel()
        let row = selectedRow
        let work = DispatchWorkItem { [weak self] in
            guard let self, Theme.shared.previewVisible else { return }
            self.preview.show(row)
        }
        previewWork = work
        // Short delay so holding j doesn't render every file on the way.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: work)
    }

    /// ⌃j / ⌃k move down / up, ⌃h goes up a folder, ⌃l goes in (like `tab` or `l`).
    private func handleControlNavigation(_ event: NSEvent) -> Bool {
        guard event.window === window, mode != .edit,
              event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .control,
              let key = event.charactersIgnoringModifiers?.lowercased()
        else { return false }
        if let overlay {
            switch key {
            case "j": overlay.moveSelection(by: 1)
            case "k": overlay.moveSelection(by: -1)
            default: return false
            }
            return true
        }
        switch key {
        case "j": moveSelection(by: 1)
        case "k": moveSelection(by: -1)
        case "h": goUp()
        case "l":
            if mode == .go {
                whenResultsCurrent { [weak self] in self?.goIntoHighlighted() }
            } else {
                goIntoOrActivate()
            }
        default: return false
        }
        return true
    }

    func windowWillClose(_ notification: Notification) {
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        indexTimer?.invalidate()
        preview.close()
    }
}

// MARK: Engine events

extension BrowserWindowController: EngineEventsDelegate {
    func engineResultsReady(_ info: ResultsInfo) {
        let generation = info.generation
        guard generation == latestGeneration else { return }
        let isRefresh = generation == shownGeneration
        let keepPath = isRefresh ? selectedRow?.path : nil
        let keepRow = tableView.selectedRow
        shownGeneration = generation
        currentFolder = info.folder
        activity = info.activity
        results.reset(
            generation: generation, hereCount: Int(info.hereCount), everywhereCount: Int(info.everywhereCount),
            groups: info.activity?.groups ?? [])
        tableView.reloadData()

        if let name = pendingSelection, let i = engine.indexOf(generation: generation, name: name) {
            select(row: results.tableRow(forHere: Int(i)))
            pendingSelection = nil
        } else if let keepPath {
            // Refreshes while the index fills in shouldn't move the selection.
            select(row: results.tableRow(forPath: keepPath) ?? min(max(keepRow, 0), results.tableRows - 1))
        } else {
            selectFirstResult()
        }
        if !isRefresh { pendingSelection = nil }
        // A refresh can put a different item under the same row (the old one moved away)
        // without a selection change, so the preview is updated here.
        if isRefresh, selectedRow?.path != keepPath { schedulePreview() }
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
        } else if text == "@" {
            field.stringValue = ""
            toggleActivity()
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
            let (title, count) = results.header(row) ?? ("", 0)
            header.configure(title: title, count: count)
            return header
        }
        let cell = tableView.makeView(withIdentifier: RowCellView.identifier, owner: nil) as? RowCellView ?? RowCellView()
        if let r = results.row(row) { cell.configure(with: r, onShelf: shelfSet.contains(r.path)) }
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

/// The current results as table rows: Here rows (with date headings in Activity), then
/// (if any) an Everywhere header followed by Everywhere rows. Rows are fetched from the
/// engine in chunks on demand.
@MainActor
final class ResultsCache {
    private struct Header {
        /// Table row of the header.
        let row: Int
        let title: String
        let count: Int
    }

    private let engine: Engine
    private var generation: UInt64 = 0
    private var rows: [Int: RowView] = [:]
    private(set) var hereCount = 0
    private(set) var everywhereCount = 0
    private var headers: [Header] = []
    private let chunk = 64

    init(engine: Engine) { self.engine = engine }

    var tableRows: Int { hereCount + everywhereCount + headers.count }

    func isHeader(_ tableRow: Int) -> Bool { headers.contains { $0.row == tableRow } }

    /// Title and item count for a header row.
    func header(_ tableRow: Int) -> (String, Int)? {
        headers.first { $0.row == tableRow }.map { ($0.title, $0.count) }
    }

    func reset(generation: UInt64, hereCount: Int, everywhereCount: Int, groups: [RowGroup]) {
        self.generation = generation
        self.hereCount = hereCount
        self.everywhereCount = everywhereCount
        // Headers in result order: each sits before its first result.
        var starts: [(Int, String, Int)] = groups.enumerated().map { i, g in
            let next = i + 1 < groups.count ? Int(groups[i + 1].start) : hereCount
            return (Int(g.start), g.title, next - Int(g.start))
        }
        if everywhereCount > 0 { starts.append((hereCount, "Everywhere", everywhereCount)) }
        headers = starts.enumerated().map { k, h in Header(row: h.0 + k, title: h.1, count: h.2) }
        rows.removeAll(keepingCapacity: true)
    }

    /// Table row for the Here result at `index`.
    func tableRow(forHere index: Int) -> Int {
        var row = index
        for h in headers where h.row <= row { row += 1 }
        return row
    }

    func row(_ tableRow: Int) -> RowView? {
        guard tableRow >= 0, tableRow < tableRows, !isHeader(tableRow) else { return nil }
        let index = tableRow - headers.filter { $0.row < tableRow }.count
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

// MARK: File operations

extension BrowserWindowController: OpsEventsDelegate {
    /// The shelf if it has anything, otherwise the highlighted row.
    private var targets: [String] {
        if !shelf.isEmpty { return shelf }
        return selectedRow.map { [$0.path] } ?? []
    }

    private func describe(_ paths: [String]) -> String {
        paths.count == 1 ? "“\((paths[0] as NSString).lastPathComponent)”" : "\(paths.count) items"
    }

    private func display(_ path: String) -> String {
        let home = engine.homeDir()
        if path == home { return "~" }
        return path.hasPrefix(home + "/") ? "~" + path.dropFirst(home.count) : path
    }

    // MARK: Shelf

    private func setShelf(_ paths: [String]) {
        shelf = paths
        shelfSet = Set(paths)
        shelfBar.update(paths: paths)
        shelfBar.isHidden = paths.isEmpty
        shelfHeight.constant = paths.isEmpty ? 0 : 26
        let selected = tableView.selectedRowIndexes
        tableView.reloadData()
        tableView.selectRowIndexes(selected, byExtendingSelection: false)
    }

    private func toggleShelf() {
        guard let row = selectedRow else { return }
        if shelfSet.contains(row.path) {
            setShelf(shelf.filter { $0 != row.path })
        } else {
            setShelf(shelf + [row.path])
        }
        moveSelection(by: 1)
    }

    /// Adds every shown row, or removes them if they're all on the shelf already.
    private func toggleShelfAll() {
        let shown = (0..<results.tableRows).compactMap { results.row($0)?.path }
        if shown.allSatisfy(shelfSet.contains) {
            let remove = Set(shown)
            setShelf(shelf.filter { !remove.contains($0) })
        } else {
            setShelf(shelf + shown.filter { !shelfSet.contains($0) })
        }
    }

    // MARK: Actions

    private func submit(_ request: OpRequest, describing message: String) {
        do {
            let id = try ops.submit(request: request)
            myOperations[id] = message
        } catch {
            statusBar.flash(error.localizedDescription)
        }
    }

    private func pickDestination(copy: Bool) {
        let sources = targets
        guard !sources.isEmpty else { return statusBar.flash("Nothing selected") }
        let here = currentFolder
        let title = "\(copy ? "Copy" : "Move") \(describe(sources)) to…"
        showOverlay(PromptOverlay(title: title, kind: .destination(
            search: { [index, weak self] query in
                var found = index.destinations(query: query, limit: 12)
                // Copying into the current folder makes a duplicate; offer it first.
                if copy, query.isEmpty, let self {
                    found.insert(Destination(path: here, display: "\(self.display(here))  (this folder)", highlights: []), at: 0)
                }
                return found
            },
            onPick: { [weak self] dest in
                guard let self else { return }
                let what = copy ? "Copied" : "Moved"
                let folder = dest == here ? "this folder" : "“\((dest as NSString).lastPathComponent)”"
                self.submit(
                    OpRequest(kind: copy ? .copy : .move, sources: sources, destination: dest, name: nil),
                    describing: "\(what) \(self.describe(sources)) to \(folder)")
                self.setShelf([])
            })))
    }

    private func trashTargets() {
        let sources = targets
        guard !sources.isEmpty else { return statusBar.flash("Nothing selected") }
        submit(OpRequest(kind: .trash, sources: sources, destination: nil, name: nil),
               describing: "Moved \(describe(sources)) to the Trash")
        setShelf([])
    }

    private func compressTargets() {
        let sources = targets
        guard !sources.isEmpty else { return statusBar.flash("Nothing selected") }
        submit(OpRequest(kind: .compress, sources: sources, destination: nil, name: nil),
               describing: "Compressed \(describe(sources))")
        setShelf([])
    }

    private func copyPaths() {
        let paths = targets
        guard !paths.isEmpty else { return statusBar.flash("Nothing selected") }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(paths.joined(separator: "\n"), forType: .string)
        statusBar.flash("Copied \(paths.count == 1 ? "path" : "\(paths.count) paths")")
    }

    private func renameHighlighted() {
        guard let row = selectedRow else { return }
        let name = row.name
        // Select the part before the extension, like the Finder.
        let stem: Int = {
            guard row.kind != .folder, let dot = name.lastIndex(of: "."), dot != name.startIndex else {
                return (name as NSString).length
            }
            return (String(name[..<dot]) as NSString).length
        }()
        showOverlay(PromptOverlay(
            title: "Rename “\(name)”",
            kind: .text(initial: name, selection: NSRange(location: 0, length: stem), onSubmit: { [weak self] newName in
                guard let self, newName != name else { return }
                self.submit(OpRequest(kind: .rename, sources: [row.path], destination: nil, name: newName),
                            describing: "Renamed to “\(newName)”")
            })))
    }

    @objc func newFolder(_ sender: Any?) {
        guard !inActivity else { return statusBar.flash("Activity isn't a folder. Go to a folder to create one.") }
        let parent = currentFolder
        showOverlay(PromptOverlay(
            title: "New folder in \(display(parent))",
            kind: .text(initial: "untitled folder", selection: NSRange(location: 0, length: 15), onSubmit: { [weak self] name in
                self?.submit(OpRequest(kind: .newFolder, sources: [], destination: parent, name: name),
                             describing: "Created “\(name)”")
            })))
    }

    @objc func moveToTrash(_ sender: Any?) {
        trashTargets()
    }

    // Undo and redo pressed here report here (an empty description means "describe it
    // from the outcome").
    private func undo() {
        if let id = ops.undo() { myOperations[id] = "" } else { statusBar.flash("Nothing to undo") }
    }

    private func redo() {
        if let id = ops.redo() { myOperations[id] = "" } else { statusBar.flash("Nothing to redo") }
    }

    private func showOverlay(_ prompt: PromptOverlay) {
        overlay?.removeFromSuperview()
        guard let content = window?.contentView else { return }
        let restore = mode
        overlay = prompt
        prompt.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(prompt)
        NSLayoutConstraint.activate([
            prompt.topAnchor.constraint(equalTo: content.topAnchor),
            prompt.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            prompt.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            prompt.trailingAnchor.constraint(equalTo: content.trailingAnchor),
        ])
        prompt.focus()
        prompt.onCloseExtra = { [weak self] in
            self?.overlay = nil
            self?.setMode(restore)
        }
    }

    // MARK: Operation events (from AppDelegate)

    /// Progress shows in the window that started the operation, or the front window for
    /// undo and redo.
    private func reports(_ id: UInt64) -> Bool {
        myOperations[id] != nil || window?.isKeyWindow == true
    }

    func operationStarted(id: UInt64, label: String, total: Int) {
        guard reports(id) else { return }
        runningLabels[id] = label
        statusBar.setActivity("\(label)…")
    }

    func operationProgress(id: UInt64, done: Int, total: Int, current: String) {
        guard reports(id), let label = runningLabels[id], total > 1, !current.isEmpty else { return }
        statusBar.setActivity("\(label)… \(min(done + 1, total))/\(total) · \(current)")
    }

    func operationFinished(id: UInt64, outcome: OpOutcome) {
        // An applied edit closes the editor; a failed one leaves it open to fix.
        if id == pendingEditOp {
            pendingEditOp = nil
            if outcome.error == nil { exitEdit() }
        }
        let mine = myOperations.removeValue(forKey: id)
        if runningLabels.removeValue(forKey: id) != nil { statusBar.setActivity(nil) }
        guard mine != nil || window?.isKeyWindow == true else { return }

        if let error = outcome.error {
            if outcome.completed > 0 {
                statusBar.flash("\(outcome.label) stopped after \(outcome.completed) of \(outcome.total): \(error)")
            } else {
                statusBar.flash(error)
            }
            return
        }
        var message: String
        switch outcome.phase {
        case .done: message = mine.flatMap { $0.isEmpty ? nil : $0 } ?? outcome.label
        case .undone: message = "Undid \(outcome.label)"
        case .redone: message = "Redid \(outcome.label)"
        }
        if outcome.renamed > 0 {
            message += " (\(outcome.renamed) renamed to keep both)"
        }
        if outcome.phase == .done, outcome.completed == 0, outcome.skipped > 0 {
            message = "Already there, nothing to do"
        } else if outcome.phase != .undone {
            message += " · u undo"
        }
        statusBar.flash(message)

        // Select what was just renamed or created when it's in this folder; the folder
        // refresh that follows picks it up.
        if outcome.phase != .undone, outcome.results.count == 1,
           let result = outcome.results.first,
           !inActivity,
           (result as NSString).deletingLastPathComponent == currentFolder {
            pendingSelection = (result as NSString).lastPathComponent
            if let i = engine.indexOf(generation: shownGeneration, name: pendingSelection!) {
                select(row: results.tableRow(forHere: Int(i)))
                pendingSelection = nil
            }
        }
    }
}

// MARK: EDIT mode

extension BrowserWindowController {
    /// Opens the current folder as text. `focus` names the line to start on; `selectName`
    /// selects its name (rename); `newLine` opens an empty line below it. In quick mode `⏎`
    /// applies.
    private func enterEdit(focus: String?, selectName: Bool, newLine: Bool, quick: Bool) {
        guard !currentFolder.isEmpty, overlay == nil, !inActivity else { return }
        let session: EditSession
        do {
            session = try EditSession(dir: currentFolder, showHidden: showHidden)
        } catch {
            return statusBar.flash("Can't edit this folder: \(error.localizedDescription)")
        }
        editSession = session
        editBaseline = session.text()
        editEscArmed = false
        editor.quick = quick
        mode = .edit
        listScroll.isHidden = true
        editor.isHidden = false
        changesView.isHidden = false
        updateSidePane(Theme.shared)
        let line = focus.flatMap { session.lineOf(name: $0) }.map(Int.init)
        editor.load(editBaseline, line: line, selectName: selectName, newLineBelow: newLine)
        setMode(.edit)
    }

    /// Debounced live preview of what applying would do.
    private func editChanged() {
        editEscArmed = false
        editPreviewWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, let session = self.editSession else { return }
            let preview = session.preview(text: self.editor.text)
            self.changesView.show(preview, quick: self.editor.quick)
            self.editor.markErrors(Set(preview.errors.map { Int($0.line) }))
        }
        editPreviewWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1, execute: work)
    }

    /// File › Apply Edits (⌘S).
    @objc func applyEdits(_ sender: Any?) {
        applyEdit()
    }

    private func applyEdit() {
        guard let session = editSession, pendingEditOp == nil else { return }
        let text = editor.text
        let preview = session.preview(text: text)
        if let e = preview.errors.first {
            return statusBar.flash("Fix line \(e.line + 1) first: \(e.message)")
        }
        if preview.changes.isEmpty { return exitEdit() }
        do {
            let id = try ops.submitEdit(session: session, text: text)
            let n = preview.changes.count
            myOperations[id] = "Applied \(n) change\(n == 1 ? "" : "s")"
            pendingEditOp = id
        } catch {
            statusBar.flash(error.localizedDescription)
        }
    }

    private func cancelEdit() {
        let changed = editSession.map { $0.preview(text: editor.text).changes.count } ?? 0
        if changed > 0 && !editEscArmed {
            editEscArmed = true
            return statusBar.flash("Press esc again to discard \(changed) change\(changed == 1 ? "" : "s")")
        }
        exitEdit()
    }

    private func exitEdit() {
        guard mode == .edit else { return }
        editPreviewWork?.cancel()
        editSession = nil
        editor.isHidden = true
        changesView.isHidden = true
        listScroll.isHidden = false
        mode = .select
        updateSidePane(Theme.shared)
        setMode(.select)
        schedulePreview()
    }
}

extension BrowserWindowController: NSMenuItemValidation {
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        if item.action == #selector(applyEdits(_:)) { return mode == .edit }
        return true
    }
}
