import AppKit
import TildeFFI

/// History (Edit › History…, `H` in SELECT): every file operation, newest first, including
/// undone ones. Any entry can be undone or redone, not only the latest: undoing something
/// old reverses just that operation, and anything a later operation made impossible is
/// reported instead of forced.
@MainActor
final class HistoryWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate {
    private let ops: FileOps
    private var entries: [HistoryEntry] = []
    private let table = HistoryTable()
    private let details = NSTextView()
    private let status = NSTextField(labelWithString: "")
    private let undoButton = NSButton(title: "Undo", target: nil, action: nil)
    private let redoButton = NSButton(title: "Redo", target: nil, action: nil)
    /// Undo and redo started here, to report how they went.
    private var mine: Set<UInt64> = []

    init(ops: FileOps) {
        self.ops = ops
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 700, height: 560),
            styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "History"
        window.minSize = NSSize(width: 480, height: 380)
        super.init(window: window)
        window.delegate = self
        build(in: window)
        window.setFrameAutosaveName("TildeHistory")
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show() {
        reload()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        window?.makeFirstResponder(table)
        if table.selectedRow < 0, !entries.isEmpty { table.selectRowIndexes([0], byExtendingSelection: false) }
    }

    // MARK: Layout

    private func build(in window: NSWindow) {
        let theme = Theme.shared
        let hint = NSTextField(labelWithString:
            "j k move   u undo this   r redo this   esc close. Undoing an old entry reverses only that one.")
        hint.font = theme.small
        hint.textColor = .secondaryLabelColor
        hint.lineBreakMode = .byTruncatingTail

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("entry"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = theme.rowHeight * 1.7
        table.style = .inset
        table.dataSource = self
        table.delegate = self
        table.onKey = { [weak self] event in self?.handleKey(event) ?? false }
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false

        details.isEditable = false
        details.isSelectable = true
        details.drawsBackground = false
        details.font = theme.small
        details.textContainerInset = NSSize(width: 10, height: 8)
        let detailsScroll = NSScrollView()
        detailsScroll.documentView = details
        detailsScroll.hasVerticalScroller = true
        detailsScroll.drawsBackground = false
        details.autoresizingMask = [.width]
        details.isVerticallyResizable = true

        status.font = theme.small
        status.lineBreakMode = .byTruncatingTail
        undoButton.target = self
        undoButton.action = #selector(undoSelected)
        redoButton.target = self
        redoButton.action = #selector(redoSelected)
        let separator = NSBox()
        separator.boxType = .separator

        let content = NSView()
        window.contentView = content
        for v in [hint, scroll, separator, detailsScroll, status, undoButton, redoButton] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(v)
        }
        NSLayoutConstraint.activate([
            hint.topAnchor.constraint(equalTo: content.topAnchor, constant: 10),
            hint.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 14),
            hint.trailingAnchor.constraint(lessThanOrEqualTo: content.trailingAnchor, constant: -14),
            scroll.topAnchor.constraint(equalTo: hint.bottomAnchor, constant: 6),
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            scroll.heightAnchor.constraint(equalTo: content.heightAnchor, multiplier: 0.55),
            separator.topAnchor.constraint(equalTo: scroll.bottomAnchor),
            separator.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            detailsScroll.topAnchor.constraint(equalTo: separator.bottomAnchor),
            detailsScroll.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            detailsScroll.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            detailsScroll.bottomAnchor.constraint(equalTo: undoButton.topAnchor, constant: -8),
            status.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 14),
            status.centerYAnchor.constraint(equalTo: undoButton.centerYAnchor),
            status.trailingAnchor.constraint(lessThanOrEqualTo: undoButton.leadingAnchor, constant: -10),
            redoButton.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -14),
            redoButton.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -12),
            undoButton.trailingAnchor.constraint(equalTo: redoButton.leadingAnchor, constant: -8),
            undoButton.bottomAnchor.constraint(equalTo: redoButton.bottomAnchor),
        ])
    }

    // MARK: Data

    private func reload() {
        let selected = table.selectedRow >= 0 && table.selectedRow < entries.count ? entries[table.selectedRow].id : nil
        entries = ops.history()
        table.reloadData()
        let row = selected.flatMap { id in entries.firstIndex { $0.id == id } } ?? (entries.isEmpty ? nil : 0)
        if let row { table.selectRowIndexes([row], byExtendingSelection: false) }
        updateDetails()
    }

    private var selected: HistoryEntry? {
        let row = table.selectedRow
        return row >= 0 && row < entries.count ? entries[row] : nil
    }

    private func updateDetails() {
        guard let e = selected else {
            details.string = entries.isEmpty ? "Nothing has been done yet." : ""
            undoButton.isEnabled = false
            redoButton.isEnabled = false
            return
        }
        let state = switch e.state {
        case .applied: "Done"
        case .undone: "Undone"
        case .partlyUndone: "Partly undone (the rest can still be undone)"
        }
        var lines = ["\(e.label)", "\(state) · \(Self.time(e.at))" + (e.changed != e.at ? ", changed \(Self.time(e.changed))" : ""), ""]
        lines += e.changes.isEmpty ? ["(No record of the individual changes.)"] : e.changes
        details.string = lines.joined(separator: "\n")
        details.textColor = Theme.shared.colors.text
        undoButton.isEnabled = e.canUndo
        redoButton.isEnabled = e.canRedo
    }

    /// "11:55", "Yesterday 14:02", "3 Oct 09:12".
    static func time(_ seconds: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(seconds))
        let cal = Calendar.current
        let clock = date.formatted(date: .omitted, time: .shortened)
        if cal.isDateInToday(date) { return clock }
        if cal.isDateInYesterday(date) { return "Yesterday \(clock)" }
        return "\(date.formatted(.dateTime.day().month(.abbreviated))) \(clock)"
    }

    // MARK: Actions

    @objc private func undoSelected() {
        guard let e = selected else { return }
        guard let id = ops.undoEntry(entry: e.id) else { return flash("Nothing to undo there", error: true) }
        mine.insert(id)
        flash("Undoing \(e.label)…")
    }

    @objc private func redoSelected() {
        guard let e = selected else { return }
        guard let id = ops.redoEntry(entry: e.id) else { return flash("Only undone entries can be redone", error: true) }
        mine.insert(id)
        flash("Redoing \(e.label)…")
    }

    private func flash(_ message: String, error: Bool = false) {
        status.stringValue = message
        status.textColor = error ? .systemOrange : .secondaryLabelColor
    }

    /// Any operation finished (from any window): refresh, and say how ours went.
    func operationFinished(id: UInt64, outcome: OpOutcome) {
        guard window?.isVisible == true else { return }
        reload()
        guard mine.remove(id) != nil else { return }
        if let error = outcome.error {
            let partial = outcome.completed > 0 ? " after \(outcome.completed) of \(outcome.total)" : ""
            flash("Stopped\(partial): \(error)", error: true)
        } else {
            flash("\(outcome.phase == .undone ? "Undid" : "Redid") \(outcome.label)")
        }
    }

    private func handleKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        guard flags.isDisjoint(with: [.command, .control, .option]) else { return false }
        switch event.charactersIgnoringModifiers {
        case "j": move(1)
        case "k": move(-1)
        case "u": undoSelected()
        case "r", "U": redoSelected()
        case "\u{1b}": close()
        default: return false
        }
        return true
    }

    private func move(_ delta: Int) {
        guard !entries.isEmpty else { return }
        let row = min(max(table.selectedRow + delta, 0), entries.count - 1)
        table.selectRowIndexes([row], byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { entries.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = tableView.makeView(withIdentifier: HistoryCell.identifier, owner: nil) as? HistoryCell ?? HistoryCell()
        cell.configure(entries[row])
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        updateDetails()
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        AlwaysEmphasizedRowView()
    }
}

/// The table, passing keys to the window first.
final class HistoryTable: NSTableView {
    var onKey: ((NSEvent) -> Bool)?

    override func keyDown(with event: NSEvent) {
        if onKey?(event) != true { super.keyDown(with: event) }
    }

    override func cancelOperation(_ sender: Any?) {
        window?.close()
    }
}

/// Two lines: state, label and time; then the first change.
@MainActor
final class HistoryCell: NSTableCellView {
    static let identifier = NSUserInterfaceItemIdentifier("HistoryCell")
    private let title = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")
    private let when = NSTextField(labelWithString: "")

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
        for f in [title, detail, when] {
            f.translatesAutoresizingMaskIntoConstraints = false
            f.lineBreakMode = .byTruncatingMiddle
            addSubview(f)
        }
        when.setContentCompressionResistancePriority(.required, for: .horizontal)
        title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        detail.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        NSLayoutConstraint.activate([
            title.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            title.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            title.trailingAnchor.constraint(lessThanOrEqualTo: when.leadingAnchor, constant: -10),
            when.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            when.firstBaselineAnchor.constraint(equalTo: title.firstBaselineAnchor),
            detail.leadingAnchor.constraint(equalTo: title.leadingAnchor, constant: 18),
            detail.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 1),
            detail.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -8),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func configure(_ e: HistoryEntry) {
        let theme = Theme.shared
        let (mark, color): (String, NSColor) = switch e.state {
        case .applied: ("●", theme.colors.select)
        case .undone: ("○", theme.colors.textDim)
        case .partlyUndone: ("◐", .systemOrange)
        }
        let s = NSMutableAttributedString(string: "\(mark)  ", attributes: [.font: theme.list, .foregroundColor: color])
        var attrs: [NSAttributedString.Key: Any] = [.font: theme.list, .foregroundColor: theme.colors.text]
        if e.state == .undone {
            attrs[.foregroundColor] = theme.colors.textDim
            attrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
        }
        s.append(NSAttributedString(string: e.label, attributes: attrs))
        title.attributedStringValue = s
        let more = e.changes.count > 1 ? "  (+\(e.changes.count - 1) more)" : ""
        detail.stringValue = (e.changes.first ?? "") + more
        detail.font = theme.small
        detail.textColor = theme.colors.textDim
        when.stringValue = HistoryWindowController.time(e.at)
        when.font = theme.small
        when.textColor = theme.colors.textDim
    }
}
