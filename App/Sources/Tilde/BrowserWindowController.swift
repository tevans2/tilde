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
    /// "2/5 · file.dmg" for running operations with several steps.
    private var runningSteps: [UInt64: String] = [:]
    private var overlay: PromptOverlay?
    private let listScroll = NSScrollView()
    /// History (`H`): the undo tree, over the list and preview.
    private let historyView = HistoryView()
    private var inHistory = false
    /// Clean Up (`C`): suggestions for freeing space, over the list (the preview stays).
    private let cleanupView = CleanupView()
    private var inCleanup = false
    /// Started on the first Clean Up, scanning in the background.
    private var cleanup: CleanupService?
    private let cleanupEvents = CleanupEvents()
    /// Every suggestion seen this scan, by path, for checking before acting on the shelf.
    private var cleanupSeen: [String: (CleanupKind, CleanupItem)] = [:]
    private var cleanupReload: DispatchWorkItem?
    // EDIT mode
    private let editor = FolderEditor()
    private let changesView = ChangesView()
    private var editSession: EditSession?
    private var editBaseline = ""
    private var editEscArmed = false
    private var pendingEditOp: UInt64?
    private var editPreviewWork: DispatchWorkItem?
    /// What the changes pane says about applying and discarding.
    private var editFooter = ""
    // EDIT in Neovim (`[edit] editor = "nvim"`): one nvim per window, started on the first
    // edit and kept running between them.
    private let nvimEvents = NvimEvents()
    private lazy var nvim = NvimEditor(listener: nvimEvents)
    private var terminal: TerminalHost?
    /// The current edit is in Neovim.
    private var editInNvim = false
    private var keyMonitor: Any?
    private var focusObservation: NSKeyValueObservation?
    /// PREVIEW: the preview has the keyboard (first `p`), j / k scroll it, and the pane
    /// takes most of the window.
    private var previewFocused = false
    /// `f` in PREVIEW: the preview takes the whole window. Kept for the window's next
    /// PREVIEW too.
    private var previewWhole = false
    private let engine: Engine
    private let results: ResultsCache

    private let pathBar = PathBar()
    private let tableView = ResultsTableView()
    private let preview = PreviewPane()
    private let previewSeparator = NSBox()
    private let topSeparator = NSBox()
    private var previewWidth: NSLayoutConstraint?
    private var pathBarHeight: NSLayoutConstraint!
    private let statusBar = StatusBar()

    private var mode: Mode = .go
    /// The newest generation sent to the engine. Older results are ignored.
    private var latestGeneration: UInt64 = 0
    /// The generation currently shown. The engine re-sends a generation while the
    /// index fills in; those refreshes keep the selection.
    private var shownGeneration: UInt64 = 0
    private var currentFolder = ""
    /// The folder showing (empty in Activity), for the file-dialog helper.
    var folderShowing: String? { inActivity || currentFolder.isEmpty ? nil : currentFolder }
    /// Activity is showing (or has been asked for). `currentFolder` is where it was opened
    /// from, and where `⌫` / `h` / `@` go back to.
    private var inActivity = false
    private var activityFilter = ActivityFilter(origin: .all, inbox: false)
    /// From the latest results: inbox count and date headings.
    private var activity: ActivityInfo?
    /// Row to select when the next results arrive (the folder we came up from, or a
    /// revealed file).
    private var pendingSelection: String?
    /// Like `pendingSelection`, by path, for rows outside the folder (Everywhere).
    private var pendingPath: String?
    /// Where the window is (or is going: set when navigation starts, before results).
    private var place: Place?
    /// `⌃o` / `⌃i`: places left, newest last, and places backed out of.
    private var jumps = JumpList()
    private var showHidden = false
    /// The open folder's order, from the latest results.
    private var sort = FolderSort(by: .name, reverse: false, custom: false)
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
        // No system title bar: the path bar takes its place, with the window buttons in it.
        // An empty compact toolbar sets the bar's height and keeps the buttons centred.
        window.styleMask.insert(.fullSizeContentView)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.titlebarSeparatorStyle = .none
        window.toolbar = NSToolbar(identifier: "TildeBrowser")
        window.toolbarStyle = .unifiedCompact
        super.init(window: window)
        window.delegate = self
        events.delegate = self
        nvimEvents.delegate = self
        buildLayout(in: window)
        applyTheme(Theme.shared)
        window.setContentSize(NSSize(width: 1100, height: 680))
        window.center()
        #if DEBUG
        // Snapshots set their own size and mustn't change the one saved for real use.
        if !DebugSnapshot.isRunning { window.setFrameAutosaveName("TildeBrowser") }
        #else
        window.setFrameAutosaveName("TildeBrowser")
        #endif
        var start = engine.homeDir()
        #if DEBUG
        // Development aid: start somewhere else (used with the snapshot hook).
        if let dir = ProcessInfo.processInfo.environment["TILDE_START"] { start = dir }
        #endif
        open(folder: start)
        setMode(.go)
        restoreShelf()
        startIndexProgress()
        // GO keys are caught before the search field, whose text editing would otherwise
        // take them (⌃h as delete, ⌃k as kill-line, tab to the next field).
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            let handled = MainActor.assumeIsolated { self?.handleWindowKey(event) ?? false }
            return handled ? nil : event
        }
        // Some Quick Look previews (web-based ones, like Markdown extensions) take the
        // keyboard when they finish loading, which stops arrow keys moving through the
        // list until something else is clicked. Focus only stays in the preview if it was
        // clicked into.
        focusObservation = window.observe(\.firstResponder, options: [.new]) { [weak self] _, _ in
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.keepFocusOutOfPreview() } }
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
        // Drag files out (to other apps: always a copy, so nothing moves behind Tilde's
        // back) and in (onto the list or a folder row).
        tableView.setDraggingSourceOperationMask(.copy, forLocal: false)
        tableView.setDraggingSourceOperationMask([.copy, .move], forLocal: true)
        tableView.registerForDraggedTypes([.fileURL])
        tableView.dragGroup = { [weak self] path in
            guard let self, self.shelfSet.contains(path) else { return nil }
            return self.shelf
        }
        let rowMenu = NSMenu()
        rowMenu.delegate = self
        tableView.menu = rowMenu

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

        for v in [pathBar, topSeparator, scroll, previewSeparator, preview, shelfBar, statusBar, editor, changesView, historyView, cleanupView] as [NSView] {
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
            historyView.topAnchor.constraint(equalTo: scroll.topAnchor),
            historyView.bottomAnchor.constraint(equalTo: scroll.bottomAnchor),
            historyView.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            historyView.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            cleanupView.topAnchor.constraint(equalTo: scroll.topAnchor),
            cleanupView.bottomAnchor.constraint(equalTo: scroll.bottomAnchor),
            cleanupView.leadingAnchor.constraint(equalTo: scroll.leadingAnchor),
            cleanupView.trailingAnchor.constraint(equalTo: scroll.trailingAnchor),
        ])
        cleanupView.isHidden = true
        cleanupView.onKey = { [weak self] event in self?.handleCleanupKey(event) ?? false }
        cleanupView.onSelect = { [weak self] in self?.cleanupSelectionChanged() }
        cleanupEvents.onChange = { [weak self] _ in self?.cleanupChanged() }
        historyView.isHidden = true
        historyView.onClose = { [weak self] in self?.closeHistory() }
        historyView.onUndo = { [weak self] e in self?.undoEntry(e) }
        historyView.onRedo = { [weak self] e in self?.redoEntry(e) }
        historyView.onGoto = { [weak self] e in self?.gotoEntry(e) }
        pathBarHeight = pathBar.heightAnchor.constraint(equalToConstant: 46)
        pathBarHeight.isActive = true
        fitTitleBar()
        editor.isHidden = true
        changesView.isHidden = true
        editor.onChange = { [weak self] in self?.editChanged() }
        editor.onApply = { [weak self] in self?.applyEdit() }
        editor.onCancel = { [weak self] in self?.cancelEdit() }
        shelfHeight = shelfBar.heightAnchor.constraint(equalToConstant: 0)
        shelfHeight.isActive = true
        shelfBar.isHidden = true
    }

    /// Makes the path bar exactly the title bar's height, starting after the window
    /// buttons. In full screen the buttons are hidden, so the bar starts at the edge.
    private func fitTitleBar() {
        guard let window else { return }
        let titleBar = window.frame.height - window.contentLayoutRect.height
        pathBarHeight.constant = max(titleBar, 38)
        let fullScreen = window.styleMask.contains(.fullScreen)
        let buttons = window.standardWindowButton(.zoomButton)?.frame.maxX ?? 70
        pathBar.leadingInset = fullScreen ? 0 : buttons + 6
    }

    func windowDidEnterFullScreen(_ notification: Notification) { fitTitleBar() }
    func windowDidExitFullScreen(_ notification: Notification) { fitTitleBar() }

    /// Applies fonts and preview settings from the config. Called at startup and on
    /// Reload Config.
    func applyTheme(_ theme: Theme) {
        window?.backgroundColor = theme.colors.background
        previewSeparator.fillColor = theme.colors.border
        topSeparator.fillColor = theme.colors.border
        updateSidePane(theme)
        pathBar.applyTheme(theme)
        statusBar.applyTheme(theme)
        terminal?.applyTheme(theme)
        historyView.applyTheme(theme)
        cleanupView.applyTheme(theme)
        tableView.rowHeight = theme.rowHeight
        tableView.reloadData()
        preview.invalidate()
        if tableView.selectedRow < 0 { selectFirstResult() } else { schedulePreview() }
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
        let focusedShare = previewWhole ? 1 : CGFloat(theme.previewModeWidth)
        let share = theme.previewVisible
            ? (previewFocused ? focusedShare : CGFloat(theme.previewWidth))
            : (editing ? 0.4 : 0)
        // With the whole window to the preview, the list (and its edge) step aside.
        let whole = previewFocused && previewWhole && !editing
        listScroll.isHidden = whole || inHistory || inCleanup || editing
        cleanupView.isHidden = !inCleanup || whole
        if whole { previewSeparator.isHidden = true }
        previewWidth = preview.widthAnchor.constraint(equalTo: content.widthAnchor, multiplier: share)
        previewWidth?.isActive = true
        preview.isHidden = !theme.previewVisible || editing
        if !whole { previewSeparator.isHidden = !visible }
    }

    // MARK: Modes

    private func setMode(_ newMode: Mode) {
        mode = newMode
        switch newMode {
        case .go:
            window?.makeFirstResponder(pathBar.field)
            pathBar.field.currentEditor()?.selectedRange = NSRange(location: pathBar.field.stringValue.utf16.count, length: 0)
        case .select where inCleanup:
            cleanupView.focus()
        case .select:
            window?.makeFirstResponder(tableView)
        case .edit:
            if editInNvim { terminal?.focus() } else { editor.focus() }
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

    /// Shows `folder`, highlighting `select`. Recorded in the jump list unless it's a jump.
    private func open(folder: String, select: String? = nil, query: String = "", record: Bool = true) {
        leaveHistory()
        leaveCleanup()
        endPreviewFocus(refocus: false)
        if record { recordJump(to: .folder(folder)) }
        place = .folder(folder)
        inActivity = false
        pendingSelection = select
        pathBar.field.stringValue = query
        engine.openFolder(path: folder, generation: nextGeneration())
        engine.setQuery(text: query, generation: nextGeneration())
    }

    /// `@`: show Activity, or go back to the folder it was opened from.
    private func toggleActivity() {
        if inActivity { return open(folder: currentFolder) }
        showActivity(record: true)
    }

    private func showActivity(record: Bool, query: String = "", select: String? = nil) {
        leaveHistory()
        leaveCleanup()
        endPreviewFocus(refocus: false)
        if record { recordJump(to: .activity) }
        place = .activity
        inActivity = true
        pendingSelection = select
        pathBar.field.stringValue = query
        engine.openActivity(filter: activityFilter, generation: nextGeneration())
        engine.setQuery(text: query, generation: nextGeneration())
        updateChrome()
    }

    // MARK: Jump list

    /// Leaving the current place for `target`: remember where we were. A new move drops
    /// the places backed out of, as in a browser.
    private func recordJump(to target: Place) {
        guard let here else { return }
        jumps.leave(here, for: target)
    }

    /// The view now, as a jump list entry.
    private var here: Jump? {
        place.map { Jump(place: $0, select: selectedRow?.path, query: pathBar.field.stringValue) }
    }

    /// `⌃o` (back) and `⌃i` (forward). Folders that no longer exist are skipped.
    private func jump(back: Bool) {
        guard let here else { return }
        let exists = { (place: Place) -> Bool in
            guard case let .folder(path) = place else { return true }
            return FileManager.default.fileExists(atPath: path)
        }
        if let target = jumps.step(back: back, from: here, exists: exists) {
            let name = target.select.map { ($0 as NSString).lastPathComponent }
            switch target.place {
            case let .folder(path):
                // In the folder: by name; elsewhere (an Everywhere result): by path.
                let inFolder = target.select.map { ($0 as NSString).deletingLastPathComponent == path } ?? false
                open(folder: path, select: inFolder ? name : nil, query: target.query, record: false)
                if !inFolder { pendingPath = target.select }
            case .activity:
                showActivity(record: false, query: target.query, select: name)
            }
            // Carry on typing after the restored text.
            let end = (target.query as NSString).length
            pathBar.field.currentEditor()?.selectedRange = NSRange(location: end, length: 0)
            updateChrome()
            return
        }
        statusBar.flash(back ? "Nothing to go back to" : "Nothing to go forward to")
    }

    /// Go › Back (⌘[).
    @objc func goBack(_ sender: Any?) {
        jump(back: true)
    }

    /// Go › Forward (⌘]).
    @objc func goForward(_ sender: Any?) {
        jump(back: false)
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

    /// What the preview shows: the highlighted row, or in Clean Up the highlighted item.
    private var previewRow: RowView? {
        guard inCleanup else { return selectedRow }
        return cleanupView.selectedItem.flatMap { rowForPath(path: $0.path) }
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
        index.recordOpen(path: row.path)
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
        guard let action = Keymap.shared.action(for: event, in: .select) else {
            let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            if flags.isDisjoint(with: [.command, .control, .option]),
               let c = event.charactersIgnoringModifiers, c.count == 1, c.first!.isLetter {
                statusBar.flash("\(c) does nothing in SELECT. Press \(Keymap.shared.key(.search, in: .select)) to search.")
                return true
            }
            return false
        }
        perform(action)
        return true
    }

    /// A SELECT action (from the keymap).
    private func perform(_ action: KeyAction) {
        switch action {
        case .down: moveSelection(by: 1)
        case .up: moveSelection(by: -1)
        case .parent: goUp()
        case .enter: goIntoOrActivate()
        case .open: activateHighlighted()
        case .back: jump(back: true)
        case .forward: jump(back: false)
        case .top: select(row: 0)
        case .bottom: select(row: results.tableRows - 1)
        case .search, .selectMode: setMode(.go)
        case .activity: toggleActivity()
        case .home: open(folder: engine.homeDir())
        case .clear:
            // The filter first, then the shelf.
            if !pathBar.field.stringValue.isEmpty {
                pathBar.field.stringValue = ""
                queryChanged()
            } else if !shelf.isEmpty {
                setShelf([])
            }
        case .shelf: toggleShelf()
        case .shelfAll: toggleShelfAll()
        case .move: pickDestination(copy: false)
        case .copy: pickDestination(copy: true)
        case .trash: trashTargets()
        case .tag: tagTargets()
        case .copyPath: copyPaths()
        case .compress: compressTargets()
        case .hide: hideHighlightedFromSearch()
        case .sort: chooseSort()
        case .history: showHistory()
        case .cleanup: showCleanup()
        case .quickLook: quickLookKey()
        case .quickLookWindow: toggleQuickLook(nil)
        case .previewFull:
            // Straight into PREVIEW with the whole window.
            previewWhole = true
            quickLookKey()
        case .openWith: openWith()
        case .undo: undo()
        case .redo: redo()
        case .rename:
            // Everywhere results live in other folders: rename those with a prompt.
            if selectedRow?.section == .everywhere || inActivity {
                renameHighlighted()
            } else {
                enterEdit(focus: selectedRow?.name, selectName: true, newLine: false, quick: true)
            }
        case .edit where inActivity, .newItem where inActivity, .newFolder where inActivity:
            statusBar.flash("Activity isn't a folder. \(Keymap.shared.key(.enter, in: .select)) reveals the item in its folder.")
        case .edit: enterEdit(focus: selectedRow?.name, selectName: false, newLine: false, quick: false)
        case .newItem: enterEdit(focus: selectedRow?.name, selectName: false, newLine: true, quick: true)
        case .newFolder: newFolder(nil)
        case .filterAll where !inActivity, .filterDownloads where !inActivity, .filterScreenshots where !inActivity,
             .filterEdited where !inActivity, .filterAirdrop where !inActivity, .inbox where !inActivity:
            statusBar.flash("That filter is for Activity (\(Keymap.shared.key(.activity, in: .select))).")
        case .filterAll: setActivityFilter(origin: .all)
        case .filterDownloads: setActivityFilter(origin: .downloads)
        case .filterScreenshots: setActivityFilter(origin: .screenshots)
        case .filterEdited: setActivityFilter(origin: .edited)
        case .filterAirdrop: setActivityFilter(origin: .airDrop)
        case .inbox: setActivityFilter(toggleInbox: true)
        }
    }

    private func goIntoOrActivate() {
        if !goIntoHighlighted() { activateHighlighted() }
    }

    // MARK: History

    /// `H` / Edit › History: the undo tree in place of the list and preview.
    func showHistory() {
        guard mode != .edit, overlay == nil else { return }
        endPreviewFocus(refocus: false)
        leaveCleanup()
        if !inHistory {
            inHistory = true
            listScroll.isHidden = true
            preview.isHidden = true
            previewSeparator.isHidden = true
            historyView.isHidden = false
            historyView.reload(ops, selectCurrent: true)
        }
        mode = .select
        historyView.focus()
        updateChrome()
    }

    private func closeHistory() {
        leaveHistory()
        setMode(.select)
        updateChrome()
    }

    /// Back to the list (when leaving for a folder, Activity or EDIT too).
    private func leaveHistory() {
        guard inHistory else { return }
        inHistory = false
        historyView.isHidden = true
        listScroll.isHidden = false
        updateSidePane(Theme.shared)
        schedulePreview()
    }

    private func historySummary() -> String {
        let all = historyView.rows.compactMap(\.entry)
        let undone = all.filter { $0.state != .applied }.count
        return "\(all.count.formatted()) operation\(all.count == 1 ? "" : "s")" + (undone > 0 ? " · \(undone) undone" : "")
    }

    private func labels(_ ids: [UInt64]) -> [String] {
        ids.map { id in historyView.rows.first { $0.entry?.id == id }?.entry?.label ?? "#\(id)" }
    }

    /// "“A”, “B” and 3 more".
    private func list(_ names: [String]) -> String {
        let shown = names.prefix(3).joined(separator: ", ")
        return names.count > 3 ? "\(shown) and \(names.count - 3) more" : shown
    }

    private func undoEntry(_ e: HistoryEntry) {
        guard e.canUndo else { return statusBar.flash("“\(e.label)” isn't in place; r redoes it") }
        let p = ops.previewUndo(entry: e.id)
        let run = { [weak self] in
            guard let self else { return }
            if let id = self.ops.undoEntry(entry: e.id) { self.myOperations[id] = "" }
        }
        guard p.undo.count > 1 else { return run() }
        let later = labels(Array(p.undo.dropLast()))
        confirm("Undoing \(e.label) also undoes \(later.count) later operation\(later.count == 1 ? "" : "s") on the same files, newest first: \(list(later)).",
                action: "Undo all \(p.undo.count)", run: run)
    }

    private func redoEntry(_ e: HistoryEntry) {
        guard e.canRedo else { return statusBar.flash(e.state == .applied ? "“\(e.label)” is already in place" : "Undo the rest of “\(e.label)” first") }
        let p = ops.previewRedo(entry: e.id)
        let run = { [weak self] in
            guard let self else { return }
            if let id = self.ops.redoEntry(entry: e.id) { self.myOperations[id] = "" }
        }
        guard p.redo.count > 1 else { return run() }
        let earlier = labels(Array(p.redo.dropLast()))
        confirm("Redoing \(e.label) first redoes \(earlier.count) earlier operation\(earlier.count == 1 ? "" : "s") it builds on: \(list(earlier)).",
                action: "Redo all \(p.redo.count)", run: run)
    }

    /// `⏎`: files back the way they were right after `e` (`nil`: before everything).
    private func gotoEntry(_ e: HistoryEntry?) {
        let p = ops.previewGoto(entry: e?.id)
        if let problem = p.problem { return statusBar.flash(problem) }
        guard !p.undo.isEmpty || !p.redo.isEmpty else { return statusBar.flash("Already at that state") }
        let run = { [weak self] in
            guard let self else { return }
            do {
                if let id = try self.ops.gotoEntry(entry: e?.id) { self.myOperations[id] = "" }
            } catch {
                self.statusBar.flash(error.message)
            }
        }
        guard p.undo.count + p.redo.count > 1 else { return run() }
        var parts: [String] = []
        if !p.undo.isEmpty { parts.append("undo \(list(labels(p.undo)))") }
        if !p.redo.isEmpty { parts.append("redo \(list(labels(p.redo)))") }
        let target = e.map { "right after \($0.label)" } ?? "before all of these operations"
        confirm("Go back to how files were \(target): \(parts.joined(separator: ", then "))?",
                action: "Go (\(p.undo.count + p.redo.count) steps)", run: run)
    }

    private func confirm(_ message: String, action: String, run: @escaping () -> Void) {
        showOverlay(PromptOverlay(title: message, kind: .choice(options: [("y", action)], onChoose: { _ in run() }),
                                  hint: "⏎ or y to go ahead   esc cancel"))
    }

    // MARK: Sorting

    private static let sortKeys: [(SortBy, key: String, label: String)] = [
        (.name, "n", "Name"), (.modified, "m", "Modified"), (.added, "a", "Added"), (.size, "s", "Size"), (.kind, "k", "Kind"),
    ]

    /// "date modified, newest first".
    static func describe(_ sort: FolderSort) -> String {
        switch (sort.by, sort.reverse) {
        case (.name, false): "name, A–Z"
        case (.name, true): "name, Z–A"
        case (.modified, false): "date modified, newest first"
        case (.modified, true): "date modified, oldest first"
        case (.added, false): "date added, newest first"
        case (.added, true): "date added, oldest first"
        case (.size, false): "size, largest first"
        case (.size, true): "size, smallest first"
        case (.kind, false): "kind, A–Z"
        case (.kind, true): "kind, Z–A"
        }
    }

    /// `s`: pick the folder's order by key; the current one again reverses it.
    private func chooseSort() {
        guard !inActivity else { return statusBar.flash("Activity is in time order, newest first.") }
        let options = Self.sortKeys.map { by, key, label in
            (key: key, label: by == sort.by ? "\(label) (reverse)" : label)
        }
        showOverlay(PromptOverlay(
            title: "Sort “\(FileManager.default.displayName(atPath: currentFolder))” by… now \(Self.describe(sort))",
            kind: .choice(options: options, onChoose: { [weak self] i in self?.setSort(Self.sortKeys[i].0) }),
            hint: "press a key   the current one reverses   esc cancel"))
    }

    /// Sorts by `by`, reversing if it's already the order.
    private func setSort(_ by: SortBy) {
        let reverse = by == sort.by ? !sort.reverse : false
        engine.setSort(by: by, reverse: reverse, generation: nextGeneration())
        statusBar.flash("Sorted by \(Self.describe(FolderSort(by: by, reverse: reverse, custom: true)))")
    }

    /// View › Sort By.
    @objc func sortByMenu(_ sender: NSMenuItem) {
        guard !inActivity else { return chooseSort() }
        if sender.tag < 0 {
            engine.setSort(by: sort.by, reverse: !sort.reverse, generation: nextGeneration())
        } else {
            setSort(Self.sortKeys[sender.tag].0)
        }
    }

    // MARK: Quick Look and Open With

    /// What Quick Look shows: the shelf if it has anything, otherwise the highlighted row.
    private var quickLookItems: [URL] = []

    /// `p` / File › Quick Look (⌘Y): the full Quick Look panel, which follows the
    /// highlighted row while it's open.
    /// `p` in SELECT: PREVIEW, with the full preview (loaded now if it wasn't). Straight
    /// to the Quick Look window when the pane can't show it: preview hidden, or the shelf
    /// has items (the window shows the shelf).
    private func quickLookKey() {
        if let panel = QLPreviewPanel.shared(), panel.isVisible { return panel.orderOut(nil) }
        let row = previewRow
        guard shelf.isEmpty, Theme.shared.previewVisible, row != nil else { return toggleQuickLook(nil) }
        // The pane follows the selection after a moment: bring it up to date first.
        if preview.shownPath != row?.path {
            previewWork?.cancel()
            preview.show(row)
        }
        _ = preview.loadFull(for: row?.path)
        startPreviewFocus()
    }

    /// File › Quick Look (⌘Y): the Quick Look window, as in the Finder.
    @objc func toggleQuickLook(_ sender: Any?) {
        guard let panel = QLPreviewPanel.shared() else { return }
        if panel.isVisible {
            panel.orderOut(nil)
        } else {
            guard !targets.isEmpty else { return statusBar.flash("Nothing selected") }
            panel.makeKeyAndOrderFront(nil)
        }
    }

    private func refreshQuickLook() {
        guard QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(),
              panel.isVisible, panel.dataSource === self
        else { return }
        quickLookItems = targets.map { URL(fileURLWithPath: $0) }
        panel.reloadData()
    }

    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool { true }

    // Quick Look calls these on the main thread.
    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated {
            quickLookItems = targets.map { URL(fileURLWithPath: $0) }
            panel.dataSource = self
            panel.delegate = self
            panel.reloadData()
        }
    }

    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated {
            panel.dataSource = nil
            panel.delegate = nil
        }
    }

    /// `w`: pick an app (fuzzy, the default first) to open the targets with.
    private func openWith() {
        let paths = targets
        guard let first = paths.first else { return statusBar.flash("Nothing selected") }
        let urls = paths.map { URL(fileURLWithPath: $0) }
        let apps = Self.apps(toOpen: URL(fileURLWithPath: first))
        guard !apps.isEmpty else { return statusBar.flash("No app opens \(describe(paths))") }
        showOverlay(PromptOverlay(
            title: "Open \(describe(paths)) with…",
            kind: .destination(
                search: { query in
                    apps.filter { Self.fuzzy(query, $0.name) }.map {
                        Destination(path: $0.url.path, display: $0.isDefault ? "\($0.name)  (default)" : $0.name, highlights: [])
                    }
                },
                onPick: { [weak self] app in self?.open(urls, with: URL(fileURLWithPath: app)) }),
            hint: "↑↓ choose   ⏎ open   esc cancel",
            placeholder: "Type an app's name"))
    }

    private func open(_ urls: [URL], with app: URL) {
        NSWorkspace.shared.open(urls, withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
        for url in urls { index.recordOpen(path: url.path) }
        statusBar.flash("Opened \(describe(urls.map(\.path))) with \(FileManager.default.displayName(atPath: app.path))")
    }

    /// Apps that can open `url`: the default first, then by name.
    private static func apps(toOpen url: URL) -> [(name: String, url: URL, isDefault: Bool)] {
        let defaultApp = NSWorkspace.shared.urlForApplication(toOpen: url)
        var seen = Set<String>()
        let all = NSWorkspace.shared.urlsForApplications(toOpen: url).compactMap { app -> (String, URL, Bool)? in
            guard seen.insert(app.path).inserted else { return nil }
            let name = FileManager.default.displayName(atPath: app.path).replacingOccurrences(of: ".app", with: "")
            return (name, app, app == defaultApp)
        }
        return all.sorted { a, b in a.2 != b.2 ? a.2 : a.0.localizedCaseInsensitiveCompare(b.0) == .orderedAscending }
    }

    /// The query's letters appear in `name` in order ("vsc" matches "Visual Studio Code").
    static func fuzzy(_ query: String, _ name: String) -> Bool {
        var rest = Substring(name.lowercased())
        for c in query.lowercased() where c != " " {
            guard let i = rest.firstIndex(of: c) else { return false }
            rest = rest[rest.index(after: i)...]
        }
        return true
    }

    // MARK: Chrome

    private func updateChrome() {
        if inCleanup {
            pathBar.setTitle(cleanupTitle())
            pathBar.setMode(.select)
            pathBar.field.placeholderString = ""
            statusBar.update(mode: .select, summary: cleanupSummary(), inPreview: previewFocused,
                             inCleanup: cleanupView.kind.map { $0 == .trash ? .trash : .items } ?? .categories)
            window?.title = "Clean Up"
            window?.representedURL = nil
            return
        }
        if inHistory {
            pathBar.setTitle("History")
            pathBar.setMode(.select)
            pathBar.field.placeholderString = ""
            statusBar.update(mode: .select, summary: historySummary(), inHistory: true)
            window?.title = "History"
            window?.representedURL = nil
            return
        }
        if inActivity {
            pathBar.setActivity(activityFilter)
        } else {
            pathBar.setFolder(currentFolder, home: engine.homeDir())
        }
        pathBar.setMode(mode)
        statusBar.update(mode: mode, summary: summary(), inActivity: inActivity, inPreview: previewFocused)
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
            if !inActivity, sort.custom || sort.by != .name || sort.reverse {
                parts.append("by \(Self.describe(sort))")
            }
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
            statusBar.flash("Couldn't update the config: \(error.message)")
        }
    }

    private func schedulePreview() {
        previewWork?.cancel()
        let row = previewRow
        let work = DispatchWorkItem { [weak self] in
            guard let self, Theme.shared.previewVisible else { return }
            self.preview.show(row)
        }
        previewWork = work
        // Short delay so holding j doesn't render every file on the way.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: work)
    }

    /// GO keys from the keymap while typing in the bar, and up / down in a folder picker.
    private func handleWindowKey(_ event: NSEvent) -> Bool {
        guard event.window === window, mode != .edit else { return false }
        if previewFocused, overlay == nil { return handlePreviewKey(event) }
        if let overlay {
            guard overlay.hasList else { return false }
            switch Keymap.shared.action(for: event, in: .go) {
            case .down: overlay.moveSelection(by: 1)
            case .up: overlay.moveSelection(by: -1)
            default: return false
            }
            return true
        }
        guard mode == .go, let editor = pathBar.field.currentEditor(), window?.firstResponder === editor,
              (editor as? NSTextView)?.hasMarkedText() != true
        else { return false }
        // ⌘⌫ and ⌃⌫ clear what's typed. They must never reach Move to Trash (⌘⌫ in the
        // menu) while typing: the highlighted result isn't what's being deleted.
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if event.keyCode == 51, !flags.isDisjoint(with: [.command, .control]) {
            if !pathBar.field.stringValue.isEmpty {
                pathBar.field.stringValue = ""
                queryChanged()
                updateChrome()
            }
            return true
        }
        guard let action = Keymap.shared.action(for: event, in: .go) else { return false }
        switch action {
        case .down: moveSelection(by: 1)
        case .up: moveSelection(by: -1)
        case .parent: goUp()
        // Acting on the highlighted row waits for results to catch up with fast typing.
        case .enter: whenResultsCurrent { [weak self] in self?.goIntoHighlighted() }
        case .open: whenResultsCurrent { [weak self] in self?.activateHighlighted() }
        case .back: jump(back: true)
        case .forward: jump(back: false)
        case .selectMode: setMode(.select)
        default: return false
        }
        return true
    }

    /// Puts the keyboard back on the bar or the list if the preview took it on its own.
    private func keepFocusOutOfPreview() {
        guard let window, mode != .edit, overlay == nil else { return }
        let inPreview = (window.firstResponder as? NSView)?.isDescendant(of: preview) == true
        // Focus left the preview (a click on the list, a prompt): PREVIEW ends.
        if previewFocused {
            if !inPreview, window.firstResponder !== window { endPreviewFocus(refocus: false) }
            return
        }
        guard inPreview else { return }
        if let event = NSApp.currentEvent, event.window === window,
           [.leftMouseDown, .rightMouseDown, .otherMouseDown].contains(event.type),
           preview.frame.contains(preview.superview?.convert(event.locationInWindow, from: nil) ?? .zero) {
            return
        }
        setMode(mode)
    }

    // MARK: PREVIEW

    /// After the first `p`: the preview gets the keyboard and most of the window. The full
    /// preview may still be loading; it takes focus when it appears.
    private func startPreviewFocus() {
        guard !previewFocused, Theme.shared.previewVisible else { return }
        previewFocused = true
        preview.holdPosition()
        updateSidePane(Theme.shared)
        focusPreviewContent()
        preview.onFullShown = { [weak self] in self?.focusPreviewContent() }
        updateChrome()
    }

    private func focusPreviewContent() {
        guard previewFocused, let target = preview.focusTarget else { return }
        window?.makeFirstResponder(target)
    }

    /// Back to the list (the full preview stays). The keyboard never stays in the preview
    /// once PREVIEW is over, or keys would go nowhere.
    private func endPreviewFocus(refocus: Bool = true) {
        guard previewFocused else { return }
        previewFocused = false
        preview.onFullShown = nil
        preview.holdPosition()
        updateSidePane(Theme.shared)
        let stuck = (window?.firstResponder as? NSView)?.isDescendant(of: preview) == true
        if refocus || stuck { setMode(.select) }
        updateChrome()
    }

    /// Keys while the preview is focused, from the SELECT keymap: move keys scroll, top /
    /// bottom jump, space pages, Quick Look opens the window, esc / h / back return.
    private func handlePreviewKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if flags.contains(.command) { return false }
        if event.keyCode == 49, flags.isDisjoint(with: [.control, .option]) {
            preview.scroll(flags.contains(.shift) ? .pageUp : .pageDown)
            return true
        }
        guard let action = Keymap.shared.action(for: event, in: .select) else { return false }
        switch action {
        case .down: preview.scroll(.down)
        case .up: preview.scroll(.up)
        case .top: preview.scroll(.top)
        case .bottom: preview.scroll(.bottom)
        case .quickLook, .clear, .parent, .back, .search: endPreviewFocus()
        case .quickLookWindow: toggleQuickLook(nil)
        case .previewFull:
            previewWhole.toggle()
            preview.holdPosition()
            updateSidePane(Theme.shared)
            updateChrome()
        default: return false
        }
        return true
    }

    func windowWillClose(_ notification: Notification) {
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        cleanup?.cancel()
        indexTimer?.invalidate()
        preview.close()
        terminal?.terminate()
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
        sort = info.sort
        results.reset(
            generation: generation, hereCount: Int(info.hereCount), everywhereCount: Int(info.everywhereCount),
            groups: info.activity?.groups ?? [])
        tableView.reloadData()

        if let name = pendingSelection, let i = engine.indexOf(generation: generation, name: name) {
            select(row: results.tableRow(forHere: Int(i)))
            pendingSelection = nil
            pendingPath = nil
        } else if let path = pendingPath, let row = results.tableRow(forPath: path) {
            // A jump back to an Everywhere result.
            select(row: row)
            pendingPath = nil
        } else if let keepPath {
            // Refreshes while the index fills in shouldn't move the selection.
            select(row: results.tableRow(forPath: keepPath) ?? min(max(keepRow, 0), results.tableRows - 1))
        } else {
            selectFirstResult()
        }
        if !isRefresh { pendingSelection = nil }
        if !isRefresh, !(pendingPath.map { _ in index.status().state == .scanning } ?? false) { pendingPath = nil }
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
        // Still in the folder we were in: undo the jump list entry for the failed move.
        if place == .folder(folder) {
            place = .folder(currentFolder)
            jumps.moveFailed(stayingAt: .folder(currentFolder))
        }
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

    /// Keys bound in `[keys.go]` never get here (see `handleWindowKey`).
    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.deleteBackward(_:)) where pathBar.field.stringValue.isEmpty:
            goUp()
        // Unbound, these would end editing or move focus out of the bar.
        case #selector(NSResponder.insertNewline(_:)), #selector(NSResponder.insertTab(_:)),
             #selector(NSResponder.insertBacktab(_:)), #selector(NSResponder.cancelOperation(_:)):
            break
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
        // The list is out of sight in Clean Up (a folder refresh may still re-select).
        if inCleanup { return }
        // A folder refresh re-selects the same row; only another item ends PREVIEW.
        if selectedRow?.path != preview.shownPath { endPreviewFocus(refocus: false) }
        schedulePreview()
        refreshQuickLook()
    }

    // MARK: Drag and drop

    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        guard mode != .edit, let r = results.row(row) else { return nil }
        return URL(fileURLWithPath: r.path) as NSURL
    }

    func tableView(
        _ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int,
        proposedDropOperation dropOperation: NSTableView.DropOperation
    ) -> NSDragOperation {
        guard let target = dropTarget(row: row, on: dropOperation) else { return [] }
        if target.row == nil { tableView.setDropRow(-1, dropOperation: .on) }
        return dragOperation(info, into: target.folder)
    }

    func tableView(
        _ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int,
        dropOperation: NSTableView.DropOperation
    ) -> Bool {
        guard let target = dropTarget(row: row, on: dropOperation) else { return false }
        let operation = dragOperation(info, into: target.folder)
        let sources = droppedPaths(info)
        guard !operation.isEmpty, !sources.isEmpty else { return false }
        let copy = operation == .copy
        let folder = target.folder == currentFolder ? "this folder" : "“\((target.folder as NSString).lastPathComponent)”"
        let request = OpRequest(kind: copy ? .copy : .move, sources: sources, destination: target.folder, clash: .keepBoth)
        // After the drag ends, so a clash question can take the keyboard.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.resolvingClashes(request, in: folder) { request in
                self.submit(request, describing: "\(copy ? "Copied" : "Moved") \(self.describe(sources)) to \(folder)")
            }
        }
        return true
    }

    /// A folder row under the pointer, or else the open folder (not Activity).
    private func dropTarget(row: Int, on dropOperation: NSTableView.DropOperation) -> (folder: String, row: Int?)? {
        guard mode != .edit, overlay == nil else { return nil }
        if dropOperation == .on, let r = results.row(row), let folder = folderPath(of: r) {
            return (folder, row)
        }
        guard !inActivity, !currentFolder.isEmpty else { return nil }
        return (currentFolder, nil)
    }

    private func droppedPaths(_ info: NSDraggingInfo) -> [String] {
        let urls = info.draggingPasteboard.readObjects(
            forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        return urls.map(\.path)
    }

    /// Like the Finder: a move on the same disk, a copy to another (or with ⌥ held, which
    /// leaves only copy in the source's mask). Nothing when everything is there already.
    private func dragOperation(_ info: NSDraggingInfo, into folder: String) -> NSDragOperation {
        let paths = droppedPaths(info)
        guard !paths.isEmpty else { return [] }
        if paths.allSatisfy({ ($0 as NSString).deletingLastPathComponent == folder }) { return [] }
        let volume = { (path: String) in
            (try? URL(fileURLWithPath: path).resourceValues(forKeys: [.volumeIdentifierKey]))?.volumeIdentifier as? NSObject
        }
        let target = volume(folder)
        let sameVolume = paths.allSatisfy { volume($0)?.isEqual(target) == true }
        let mask = info.draggingSourceOperationMask
        if sameVolume, mask.contains(.move) || mask.contains(.generic) { return .move }
        if mask.contains(.copy) || mask.contains(.generic) { return .copy }
        return []
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
        if inCleanup { return cleanupView.selectedItem.map { [$0.path] } ?? [] }
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

    private static let shelfKey = "TildeShelf"
    /// The first window gets the shelf from the last session.
    private static var shelfRestored = false

    private func restoreShelf() {
        guard !Self.shelfRestored else { return }
        Self.shelfRestored = true
        var saved = UserDefaults.standard.stringArray(forKey: Self.shelfKey) ?? []
        #if DEBUG
        if DebugSnapshot.isRunning { saved = [] }
        #endif
        let existing = saved.filter { FileManager.default.fileExists(atPath: $0) }
        if !existing.isEmpty { setShelf(existing) }
    }

    private func setShelf(_ paths: [String]) {
        #if DEBUG
        if !DebugSnapshot.isRunning { UserDefaults.standard.set(paths, forKey: Self.shelfKey) }
        #else
        UserDefaults.standard.set(paths, forKey: Self.shelfKey)
        #endif
        shelf = paths
        shelfSet = Set(paths)
        shelfBar.update(paths: paths)
        shelfBar.isHidden = paths.isEmpty
        shelfHeight.constant = paths.isEmpty ? 0 : 26
        let selected = tableView.selectedRowIndexes
        tableView.reloadData()
        tableView.selectRowIndexes(selected, byExtendingSelection: false)
        cleanupView.shelved = shelfSet
        if inCleanup {
            cleanupView.refreshShelf()
            updateChrome()
        }
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
            statusBar.flash(error.message)
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
                self.index.recordDestination(path: dest)
                let what = copy ? "Copied" : "Moved"
                let folder = dest == here ? "this folder" : "“\((dest as NSString).lastPathComponent)”"
                let request = OpRequest(kind: copy ? .copy : .move, sources: sources, destination: dest, clash: .keepBoth)
                self.setShelf([])
                self.resolvingClashes(request, in: folder) { request in
                    self.submit(request, describing: "\(what) \(self.describe(sources)) to \(folder)")
                }
            })))
    }

    /// Asks what to do when items being moved or copied have the same name as something
    /// in the destination, then calls `go` with the answer. No clashes, no question.
    private func resolvingClashes(_ request: OpRequest, in folder: String, go: @escaping (OpRequest) -> Void) {
        let names = (try? ops.clashes(request: request)) ?? []
        guard !names.isEmpty else { return go(request) }
        let message: String
        if names.count == 1 {
            message = "“\(names[0])” already exists in \(folder)."
        } else {
            let listed = names.prefix(3).map { "“\($0)”" }.joined(separator: ", ")
            message = "\(names.count) items already exist in \(folder): \(listed)\(names.count > 3 ? "…" : "")"
        }
        let replace = names.count == 1 ? "Replace it" : "Replace them"
        let skip = names.count == 1 ? "Skip it" : "Skip those"
        showOverlay(PromptOverlay(title: message, kind: .choice(
            options: [("k", "Keep both"), ("r", replace + " (old to Trash)"), ("s", skip)],
            onChoose: { choice in
                var request = request
                request.clash = [ClashPolicy.keepBoth, .replace, .skip][choice]
                go(request)
            })))
    }

    private func trashTargets() {
        let sources = targets
        guard !sources.isEmpty else { return statusBar.flash("Nothing selected") }
        submit(OpRequest(kind: .trash, sources: sources, clash: .keepBoth),
               describing: "Moved \(describe(sources)) to the Trash")
        setShelf([])
    }

    private func compressTargets() {
        let sources = targets
        guard !sources.isEmpty else { return statusBar.flash("Nothing selected") }
        submit(OpRequest(kind: .compress, sources: sources, clash: .keepBoth),
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

    /// `t`: edit tags as a comma-separated list. With several items it shows the tags they
    /// share; removing one removes it from all of them, and other tags are kept.
    private func tagTargets() {
        let paths = targets
        guard !paths.isEmpty else { return statusBar.flash("Nothing selected") }
        let current = commonTags(paths: paths)
        let initial = current.joined(separator: ", ")
        let length = (initial as NSString).length
        let title = paths.count == 1 ? "Tags for \(describe(paths))" : "Tags for \(describe(paths)) (tags they all have)"
        showOverlay(PromptOverlay(
            title: title,
            kind: .text(initial: initial, selection: NSRange(location: length, length: 0), onSubmit: { [weak self] text in
                guard let self else { return }
                let typed = text.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                let has = { (list: [String], tag: String) in list.contains { $0.caseInsensitiveCompare(tag) == .orderedSame } }
                let add = typed.filter { !has(current, $0) }
                let remove = current.filter { !has(typed, $0) }
                guard !add.isEmpty || !remove.isEmpty else { return self.statusBar.flash("Tags unchanged") }
                self.submit(OpRequest(kind: .tag, sources: paths, clash: .keepBoth, tagsAdd: add, tagsRemove: remove),
                            describing: "Tagged \(self.describe(paths))")
            }),
            hint: "comma-separated, like  Work, Red   ⏎ save   esc cancel"))
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
                self.submit(OpRequest(kind: .rename, sources: [row.path], name: newName, clash: .keepBoth),
                            describing: "Renamed to “\(newName)”")
            })))
    }

    @objc func newFolder(_ sender: Any?) {
        guard !inActivity else { return statusBar.flash("Activity isn't a folder. Go to a folder to create one.") }
        let parent = currentFolder
        showOverlay(PromptOverlay(
            title: "New folder in \(display(parent))",
            kind: .text(initial: "untitled folder", selection: NSRange(location: 0, length: 15), onSubmit: { [weak self] name in
                self?.submit(OpRequest(kind: .newFolder, sources: [], destination: parent, name: name, clash: .keepBoth),
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

    func showOverlay(_ prompt: PromptOverlay) {
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
            guard let self else { return }
            self.overlay = nil
            if self.inHistory { self.historyView.focus() } else if self.inCleanup { self.setMode(.select) } else { self.setMode(restore) }
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
        runningSteps[id] = "\(min(done + 1, total))/\(total) · \(current)"
        statusBar.setActivity("\(label)… \(runningSteps[id]!)")
    }

    /// Copies to another volume report bytes: "1.2 GB of 4 GB".
    func operationBytes(id: UInt64, copied: UInt64, total: UInt64) {
        guard reports(id), let label = runningLabels[id], total > 0 else { return }
        let f = ByteCountFormatter()
        f.countStyle = .file
        let amount = "\(f.string(fromByteCount: Int64(copied))) of \(f.string(fromByteCount: Int64(total)))"
        let step = runningSteps[id].map { " · \($0)" } ?? ""
        let stop = AppDelegate.stopKey
        statusBar.setActivity("\(label)… \(amount)\(step)   \(stop) stop")
    }

    func operationFinished(id: UInt64, outcome: OpOutcome) {
        // An applied edit closes the editor; a failed one leaves it open to fix.
        if id == pendingEditOp {
            pendingEditOp = nil
            if outcome.error == nil {
                finishEdit()
            } else if editInNvim, let error = outcome.error {
                nvim.error(message: error)
            }
        }
        let mine = myOperations.removeValue(forKey: id)
        runningSteps[id] = nil
        if runningLabels.removeValue(forKey: id) != nil { statusBar.setActivity(nil) }
        guard mine != nil || window?.isKeyWindow == true else { return }

        if inHistory { historyView.reload(ops) }
        // Trashed suggestions leave the page (and their totals).
        if inCleanup { cleanup?.forgetMissing() }
        if let error = outcome.error {
            if error == "Cancelled" {
                let done = outcome.completed > 0 ? " after \(outcome.completed) of \(outcome.total)" : ""
                statusBar.flash("Stopped \(outcome.label)\(done)\(outcome.completed > 0 ? " · u undo" : "")")
            } else if outcome.completed > 0 {
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
        case .restored: message = "Back to \(outcome.label)"
        }
        var notes: [String] = []
        if outcome.renamed > 0 { notes.append("\(outcome.renamed) renamed to keep both") }
        if outcome.replaced > 0 { notes.append("\(outcome.replaced) replaced, old in the Trash") }
        if outcome.skipped > 0, outcome.completed > 0 { notes.append("\(outcome.skipped) skipped") }
        if !notes.isEmpty { message += " (\(notes.joined(separator: ", ")))" }
        if outcome.phase == .done, outcome.completed == 0, outcome.skipped > 0 {
            message = "Nothing to do"
        } else if outcome.phase != .undone, outcome.phase != .restored {
            message += " · u undo"
        }
        statusBar.flash(message)
        // Tags show under the preview.
        if outcome.label.hasPrefix("Tag ") {
            preview.invalidate()
            schedulePreview()
        }

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
        guard !currentFolder.isEmpty, overlay == nil, !inActivity, !inHistory else { return }
        let session: EditSession
        do {
            session = try EditSession(dir: currentFolder, showHidden: showHidden)
        } catch {
            return statusBar.flash("Can't edit this folder: \(error.message)")
        }
        editSession = session
        editBaseline = session.text()
        editEscArmed = false
        editor.quick = quick
        mode = .edit
        listScroll.isHidden = true
        changesView.isHidden = false
        updateSidePane(Theme.shared)
        let line = focus.flatMap { session.lineOf(name: $0) }
        if Theme.shared.editorNvim, let host = nvimTerminal() {
            editInNvim = true
            host.view.isHidden = false
            editFooter = quick ? "⏎ or :w apply   :q discard   ⌘. cancel" : ":w apply   :q discard   ⌘. cancel"
            nvim.edit(session: session, line: line, mode: selectName ? .rename : newLine ? .new : .edit)
            changesView.show(session.preview(text: editBaseline), footer: editFooter)
        } else {
            editFooter = quick ? "⏎ apply   esc cancel" : "⌘S apply   esc discard"
            editor.isHidden = false
            editor.load(editBaseline, line: line.map(Int.init), selectName: selectName, newLineBelow: newLine)
        }
        setMode(.edit)
    }

    /// The window's Neovim, started if it isn't running. `nil`, after saying why, when it
    /// can't be (EDIT then uses the built-in editor).
    private func nvimTerminal() -> TerminalHost? {
        let host: TerminalHost
        if let terminal {
            host = terminal
        } else {
            host = SwiftTermHost()
            let view = host.view
            view.translatesAutoresizingMaskIntoConstraints = false
            view.isHidden = true
            window?.contentView?.addSubview(view)
            NSLayoutConstraint.activate([
                view.topAnchor.constraint(equalTo: listScroll.topAnchor),
                view.bottomAnchor.constraint(equalTo: listScroll.bottomAnchor),
                view.leadingAnchor.constraint(equalTo: listScroll.leadingAnchor),
                view.trailingAnchor.constraint(equalTo: listScroll.trailingAnchor),
            ])
            host.applyTheme(Theme.shared)
            host.onExit = { [weak self] _ in self?.nvimExited() }
            terminal = host
        }
        if host.isRunning { return host }
        do {
            let spec = try nvim.launchSpec()
            // Sized before starting, so nvim draws for the right size from the start.
            host.view.isHidden = false
            window?.contentView?.layoutSubtreeIfNeeded()
            host.start(program: spec.program, args: spec.args, env: spec.env, directory: currentFolder)
            nvim.processStarted()
            return host
        } catch {
            statusBar.flash("Neovim: \(error.message). Using the built-in editor.")
            return nil
        }
    }

    /// Debounced live preview of what applying would do.
    private func editChanged() {
        editEscArmed = false
        editPreviewWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, let session = self.editSession else { return }
            let preview = session.preview(text: self.editor.text)
            self.changesView.show(preview, footer: self.editFooter)
            self.editor.markErrors(Set(preview.errors.map { Int($0.line) }))
        }
        editPreviewWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1, execute: work)
    }

    /// File › Apply Edits (⌘S).
    @objc func applyEdits(_ sender: Any?) {
        if editInNvim { nvim.write() } else { applyEdit() }
    }

    /// File › Cancel Edit / Stop Operation (⌘.): discards the edit, also from the middle of
    /// typing in Neovim; otherwise stops the operation this window started (or the oldest
    /// running one), partway through a large copy if need be.
    @objc func cancelEdits(_ sender: Any?) {
        guard mode == .edit else {
            let running = ops.running()
            guard let id = running.first(where: { runningLabels[$0] != nil }) ?? running.first else {
                return statusBar.flash("Nothing to stop")
            }
            ops.cancel(id: id)
            return
        }
        if editInNvim {
            nvim.close()
            exitEdit()
        } else {
            cancelEdit()
        }
    }

    private func applyEdit() {
        applyEdit(text: editor.text)
    }

    private func applyEdit(text: String) {
        guard let session = editSession, pendingEditOp == nil else { return }
        let preview = session.preview(text: text)
        if let e = preview.errors.first {
            return failEdit("Fix line \(e.line + 1) first: \(e.message)")
        }
        if preview.changes.isEmpty { return finishEdit() }
        do {
            let id = try ops.submitEdit(session: session, text: text)
            let n = preview.changes.count
            myOperations[id] = "Applied \(n) change\(n == 1 ? "" : "s")"
            pendingEditOp = id
        } catch {
            failEdit(error.message)
        }
    }

    /// The edit can't be applied: say why and keep editing.
    private func failEdit(_ message: String) {
        statusBar.flash(message)
        if editInNvim { nvim.error(message: message) }
    }

    /// Applied, or nothing to apply: close the editor.
    private func finishEdit() {
        if editInNvim { nvim.close() }
        exitEdit()
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
        editInNvim = false
        editor.isHidden = true
        terminal?.view.isHidden = true
        changesView.isHidden = true
        listScroll.isHidden = false
        mode = .select
        updateSidePane(Theme.shared)
        setMode(.select)
        schedulePreview()
    }

    /// nvim exited or dropped the connection. The next edit starts a new one.
    private func nvimExited() {
        nvim.processExited()
        guard editInNvim else { return }
        exitEdit()
        statusBar.flash("Neovim quit; the edit was discarded")
    }
}

extension BrowserWindowController: NvimEventsDelegate {
    func nvimReady() {
        if editInNvim { terminal?.focus() }
    }

    func nvimFailed(_ message: String) {
        guard editInNvim else { return }
        exitEdit()
        statusBar.flash("Neovim: \(message)")
        // Start clean next time.
        terminal?.terminate()
    }

    func nvimPreview(_ preview: EditPreview) {
        guard editInNvim else { return }
        changesView.show(preview, footer: editFooter)
    }

    func nvimApply(_ text: String) {
        guard editInNvim else { return }
        applyEdit(text: text)
    }

    func nvimCancelled() {
        guard editInNvim else { return }
        exitEdit()
    }

    func nvimDisconnected() {
        nvimExited()
    }
}

extension BrowserWindowController: NSMenuItemValidation {
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(applyEdits(_:)):
            return mode == .edit
        case #selector(toggleQuickLook(_:)):
            return mode != .edit
        // Only while acting on the list: in the search bar, a prompt or the editor, ⌘⌫
        // edits text instead.
        case #selector(moveToTrash(_:)):
            return mode == .select && overlay == nil && window?.firstResponder === tableView
        case #selector(goBack(_:)):
            return mode != .edit && jumps.canGoBack
        case #selector(goForward(_:)):
            return mode != .edit && jumps.canGoForward
        case #selector(sortByMenu(_:)):
            if item.tag >= 0 {
                item.state = Self.sortKeys[item.tag].0 == sort.by ? .on : .off
            } else {
                item.state = sort.reverse ? .on : .off
            }
            return !inActivity && mode != .edit
        case #selector(cancelEdits(_:)):
            item.title = mode == .edit ? "Cancel Edit" : "Stop Operation"
            return mode == .edit || !ops.running().isEmpty
        default:
            return true
        }
    }
}

extension BrowserWindowController: QLPreviewPanelDataSource, QLPreviewPanelDelegate {
    nonisolated func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int {
        MainActor.assumeIsolated { quickLookItems.count }
    }

    nonisolated func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! {
        MainActor.assumeIsolated { quickLookItems[index] as NSURL }
    }

    /// The list's up and down keys keep working while the panel has the keyboard; the
    /// Quick Look key closes it.
    nonisolated func previewPanel(_ panel: QLPreviewPanel!, handle event: NSEvent!) -> Bool {
        guard let event, event.type == .keyDown else { return false }
        guard let id = Keymap.id(of: event) else { return false }
        return MainActor.assumeIsolated {
            guard let action = Keymap.shared.action(id: id, in: .select) else { return false }
            switch action {
            case .down: moveSelection(by: 1)
            case .up: moveSelection(by: -1)
            case .quickLook, .quickLookWindow: panel.orderOut(nil)
            default: return false
            }
            return true
        }
    }
}

extension BrowserWindowController: NSMenuDelegate {
    /// The right-click menu, for the clicked row (which becomes the highlighted one).
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let row = tableView.clickedRow
        guard row >= 0, let r = results.row(row) else { return }
        if tableView.selectedRow != row { select(row: row) }
        let url = URL(fileURLWithPath: r.path)
        func add(_ title: String, _ action: @escaping () -> Void) {
            let item = menu.addItem(withTitle: title, action: #selector(MenuAction.run), keyEquivalent: "")
            let target = MenuAction(action)
            item.target = target
            item.representedObject = target
        }
        add("Open") { [weak self] in self?.activateHighlighted() }
        let withItem = menu.addItem(withTitle: "Open With", action: nil, keyEquivalent: "")
        let withMenu = NSMenu()
        for app in Self.apps(toOpen: url) {
            let item = withMenu.addItem(withTitle: app.isDefault ? "\(app.name) (default)" : app.name,
                                        action: #selector(MenuAction.run), keyEquivalent: "")
            let target = MenuAction { [weak self] in self?.open([url], with: app.url) }
            item.target = target
            item.representedObject = target
            item.image = NSWorkspace.shared.icon(forFile: app.url.path)
            item.image?.size = NSSize(width: 16, height: 16)
        }
        withItem.submenu = withMenu
        add("Quick Look") { [weak self] in self?.toggleQuickLook(nil) }
        add("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
        menu.addItem(.separator())
        add("Copy Path") { [weak self] in self?.copyPaths() }
        add("Tags…") { [weak self] in self?.tagTargets() }
        add("Rename…") { [weak self] in self?.renameHighlighted() }
        menu.addItem(.separator())
        add("Move to Trash") { [weak self] in self?.trashTargets() }
    }
}

/// A menu item's action as a closure (kept alive by the item's `representedObject`).
@MainActor
final class MenuAction: NSObject {
    private let action: () -> Void
    init(_ action: @escaping () -> Void) { self.action = action }
    @objc func run() { action() }
}

// MARK: Clean Up

extension BrowserWindowController {
    /// `C` / View › Clean Up: suggestions for freeing space, in place of the list. Looks
    /// the first time it opens, in the background; `r` looks again.
    func showCleanup() {
        guard mode != .edit, overlay == nil else { return }
        endPreviewFocus(refocus: false)
        leaveHistory()
        if cleanup == nil {
            cleanup = CleanupService(index: index, listener: cleanupEvents)
            // The content check reads a lot: not on battery saving.
            rescanCleanup(duplicates: !ProcessInfo.processInfo.isLowPowerModeEnabled)
        }
        if !inCleanup {
            inCleanup = true
            cleanupView.open(nil)
            updateSidePane(Theme.shared)
            reloadCleanup()
        }
        mode = .select
        cleanupView.focus()
        updateChrome()
        schedulePreview()
    }

    private func closeCleanup() {
        leaveCleanup()
        setMode(.select)
    }

    /// Back to the list (when leaving for a folder, Activity or History too).
    private func leaveCleanup() {
        guard inCleanup else { return }
        endPreviewFocus(refocus: false)
        inCleanup = false
        updateSidePane(Theme.shared)
        schedulePreview()
        updateChrome()
    }

    private func rescanCleanup(duplicates: Bool) {
        cleanupSeen = [:]
        cleanup?.scan(duplicates: duplicates)
        reloadCleanup()
    }

    /// Progress comes often while measuring: redraw at most every 150 ms.
    private func cleanupChanged() {
        guard cleanupReload == nil else { return }
        let work = DispatchWorkItem { [weak self] in
            self?.cleanupReload = nil
            self?.reloadCleanup()
        }
        cleanupReload = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
    }

    private func reloadCleanup() {
        guard inCleanup, let cleanup else { return }
        let items = cleanupView.kind.map { cleanup.items(kind: $0) }
        if let kind = cleanupView.kind {
            for item in items ?? [] { cleanupSeen[item.path] = (kind, item) }
        }
        cleanupView.shelved = shelfSet
        cleanupView.update(categories: cleanup.categories(), items: items)
        updateChrome()
    }

    private func cleanupSelectionChanged() {
        if previewFocused, cleanupView.selectedItem?.path != preview.shownPath { endPreviewFocus(refocus: false) }
        schedulePreview()
        refreshQuickLook()
    }

    /// "Clean Up › Duplicates   212 GB free".
    private func cleanupTitle() -> String {
        var title = "Clean Up"
        if let kind = cleanupView.kind { title += " › \(CleanupView.title(kind))" }
        let home = URL(fileURLWithPath: engine.homeDir())
        if let free = try? home.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
            .volumeAvailableCapacityForImportantUsage {
            title += "   \(CleanupView.bytes(UInt64(max(free, 0)))) free"
        }
        return title
    }

    private func cleanupSummary() -> String {
        guard let cleanup else { return "" }
        let categories = cleanup.categories()
        let looking = categories.contains { $0.state == .waiting || $0.state == .working }
        var parts: [String] = []
        if let category = cleanupView.category {
            parts.append("\(category.count.formatted()) item\(category.count == 1 ? "" : "s")")
            parts.append(CleanupView.bytes(category.bytes))
            if category.state == .working { parts.append("still looking…") }
        } else {
            // Categories can overlap (a big file that's also a duplicate): "up to".
            let total = categories.filter { $0.kind != .trash }.reduce(UInt64(0)) { $0 + $1.bytes }
            parts.append("up to \(CleanupView.bytes(total)) to free")
            if looking { parts.append("looking…") }
        }
        let collected = shelf.compactMap { cleanupSeen[$0]?.1 }
        if !collected.isEmpty {
            parts.append("\(collected.count) collected (\(CleanupView.bytes(collected.reduce(UInt64(0)) { $0 + $1.bytes })))")
        }
        return parts.joined(separator: " · ")
    }

    private func handleCleanupKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if flags.contains(.command) { return false }
        let plain = flags.isDisjoint(with: [.control, .option])
        if plain, event.charactersIgnoringModifiers == "r" {
            // Looking again always includes the content check (it was skipped on battery
            // saving).
            rescanCleanup(duplicates: true)
            statusBar.flash("Looking again…")
            return true
        }
        if plain, event.keyCode == 51 {
            cleanupBack()
            return true
        }
        guard let action = Keymap.shared.action(for: event, in: .select) else {
            if plain, let c = event.charactersIgnoringModifiers, c.count == 1, c.first!.isLetter {
                statusBar.flash("\(c) does nothing in Clean Up.")
                return true
            }
            return false
        }
        switch action {
        case .down: cleanupView.move(1)
        case .up: cleanupView.move(-1)
        case .top: cleanupView.select(0)
        case .bottom: cleanupView.select(cleanupView.rows.count - 1, step: -1)
        case .enter, .open: cleanupEnter()
        case .parent: cleanupBack()
        case .clear:
            if shelf.isEmpty { closeCleanup() } else { setShelf([]) }
        case .cleanup: closeCleanup()
        case .shelf: cleanupShelf()
        case .shelfAll: cleanupShelfAll()
        case .trash: cleanupTrash()
        case .move:
            guard cleanupView.kind != nil else { return true }
            pickDestination(copy: false)
        case .copyPath: copyPaths()
        case .hide: cleanupIgnore()
        case .quickLook: quickLookKey()
        case .quickLookWindow: toggleQuickLook(nil)
        case .previewFull:
            previewWhole = true
            quickLookKey()
        case .undo: undo()
        case .redo: redo()
        case .history: showHistory()
        case .activity: showActivity(record: true)
        case .home: open(folder: engine.homeDir())
        case .search: closeCleanup(); setMode(.go)
        default: statusBar.flash("That does nothing in Clean Up.")
        }
        updateChrome()
        return true
    }

    /// `l` / `⏎`: open a category; on an item, show it in its folder.
    private func cleanupEnter() {
        switch cleanupView.selected {
        case let .category(category)?:
            guard category.state != .waiting else { return statusBar.flash("Still looking…") }
            cleanupView.open(category.kind)
            reloadCleanup()
            cleanupView.select(0)
        case let .item(item)?:
            let url = URL(fileURLWithPath: item.path)
            open(folder: url.deletingLastPathComponent().path, select: url.lastPathComponent)
        default: break
        }
    }

    /// `h` / `⌫`: back to the categories, or out of Clean Up.
    private func cleanupBack() {
        guard let kind = cleanupView.kind else { return closeCleanup() }
        cleanupView.open(nil)
        reloadCleanup()
        let row = cleanupView.rows.firstIndex {
            if case let .category(c) = $0 { return c.kind == kind }
            return false
        }
        cleanupView.select(row ?? 0)
    }

    /// Why an item can't go: the copy to keep, or the cache of an app that's running.
    private func cleanupProtection(_ item: CleanupItem, _ kind: CleanupKind) -> String? {
        if item.keep { return "That's the copy to keep" }
        if cleanupView.isProtected(item, in: kind) { return "That app is running: quit it first" }
        return nil
    }

    private func cleanupShelf() {
        guard let item = cleanupView.selectedItem, let kind = cleanupView.kind else { return }
        if shelfSet.contains(item.path) {
            setShelf(shelf.filter { $0 != item.path })
        } else if let why = cleanupProtection(item, kind) {
            statusBar.flash(why)
        } else {
            setShelf(shelf + [item.path])
        }
        cleanupView.move(1)
    }

    /// `a`: every removable item in the category, or none if they're all collected.
    private func cleanupShelfAll() {
        guard let kind = cleanupView.kind else { return }
        let removable = cleanupView.items.filter { cleanupProtection($0, kind) == nil }.map(\.path)
        if removable.allSatisfy(shelfSet.contains) {
            let remove = Set(removable)
            setShelf(shelf.filter { !remove.contains($0) })
        } else {
            setShelf(shelf + removable.filter { !shelfSet.contains($0) })
        }
    }

    /// `d`: to the Trash, after checking each item is still what was suggested (same size
    /// and date; for a duplicate, still the same as the copy kept). In the Trash itself,
    /// empties it.
    private func cleanupTrash() {
        guard let cleanup, let kind = cleanupView.kind else {
            return statusBar.flash("Open a category first (\(Keymap.shared.key(.enter, in: .select)))")
        }
        if kind == .trash, shelf.isEmpty { return confirmEmptyTrash() }
        let paths = targets
        guard !paths.isEmpty else { return statusBar.flash("Nothing selected") }
        if paths.count == 1, let (k, item) = cleanupSeen[paths[0]], let why = cleanupProtection(item, k) {
            return statusBar.flash(why)
        }
        // The copy each duplicate set keeps.
        var keepers: [String: String] = [:]
        for (k, item) in cleanupSeen.values where k == .duplicates && item.keep {
            if let group = item.group { keepers[group] = item.path }
        }
        struct Check: Sendable {
            let path: String
            let size: UInt64
            let modified: Int64
            let keep: String?
            let bytes: UInt64
        }
        var pending: [Check] = []
        var others: [String] = []
        var refused = 0
        for path in paths {
            guard let (k, item) = cleanupSeen[path] else {
                others.append(path)
                continue
            }
            if cleanupProtection(item, k) != nil {
                refused += 1
                continue
            }
            let keep = k == .duplicates ? item.group.flatMap { keepers[$0] } : nil
            if k == .duplicates, keep == nil {
                refused += 1
                continue
            }
            pending.append(Check(path: path, size: item.size, modified: item.modified, keep: keep, bytes: item.bytes))
        }
        let (checks, plain) = (pending, others)
        statusBar.setActivity("Checking \(checks.count == 1 ? "it" : "\(checks.count) items") before trashing…")
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            var ok: [String] = []
            var changed = 0
            var freed: UInt64 = 0
            for c in checks {
                guard cleanup.unchanged(path: c.path, size: c.size, modified: c.modified),
                      c.keep.map({ cleanup.sameContents(a: $0, b: c.path) }) ?? true
                else {
                    changed += 1
                    continue
                }
                ok.append(c.path)
                freed += c.bytes
            }
            let go = ok + plain
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.statusBar.setActivity(nil)
                    var notes: [String] = []
                    if changed > 0 { notes.append("\(changed) changed since Clean Up looked, left alone (r looks again)") }
                    if refused > 0 { notes.append("\(refused) kept (the copy to keep, or a running app's cache)") }
                    guard !go.isEmpty else {
                        return self.statusBar.flash(notes.isEmpty ? "Nothing to trash" : notes.joined(separator: " · "))
                    }
                    let note = notes.isEmpty ? "" : " · " + notes.joined(separator: " · ")
                    self.submit(OpRequest(kind: .trash, sources: go, clash: .keepBoth),
                                describing: "Moved \(self.describe(go)) to the Trash (\(CleanupView.bytes(freed)))\(note)")
                    let gone = Set(go)
                    self.setShelf(self.shelf.filter { !gone.contains($0) })
                }
            }
        }
    }

    private func confirmEmptyTrash() {
        guard let cleanup, let trash = cleanupView.category(.trash), trash.count > 0 else {
            return statusBar.flash("The Trash is empty")
        }
        let items = "\(trash.count.formatted()) item\(trash.count == 1 ? "" : "s")"
        confirm("Empty the Trash? \(items), \(CleanupView.bytes(trash.bytes)). This can't be undone.",
                action: "Empty the Trash") { [weak self] in
            self?.statusBar.setActivity("Emptying the Trash…")
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                var freed: UInt64?
                var problem: String?
                do { freed = try cleanup.emptyTrash() } catch { problem = error.message }
                let (bytes, message) = (freed, problem)
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        guard let self else { return }
                        self.statusBar.setActivity(nil)
                        if let bytes {
                            self.statusBar.flash("Emptied the Trash · freed \(CleanupView.bytes(bytes))")
                        } else {
                            self.statusBar.flash("Couldn't empty the Trash: \(message ?? "")")
                        }
                    }
                }
            }
        }
    }

    /// `X`: never suggest it again (`[cleanup] ignore` in the config).
    private func cleanupIgnore() {
        guard let cleanup, let item = cleanupView.selectedItem else { return }
        do {
            try cleanup.ignore(path: item.path)
            reloadCleanup()
            statusBar.flash("Won't suggest \(describe([item.path])) again ([cleanup] ignore in the config)")
        } catch {
            statusBar.flash("Couldn't update the config: \(error.message)")
        }
    }
}
