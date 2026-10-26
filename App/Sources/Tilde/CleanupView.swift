import AppKit
import TildeFFI

/// Clean Up (`C`): suggestions for freeing space, over the list (the preview stays beside
/// it). First the categories with what each would free, then a category's items, biggest
/// first. Acting on them uses the window's shelf and actions, so everything but emptying
/// the Trash goes to the Trash and can be undone.
@MainActor
final class CleanupView: NSView, NSTableViewDataSource, NSTableViewDelegate {
    enum Row {
        case category(CleanupCategory)
        /// A project, an app's cache, a set of duplicates.
        case header(String)
        case item(CleanupItem)
    }

    /// The category open, or `nil` for the list of categories.
    private(set) var kind: CleanupKind?
    private(set) var rows: [Row] = []
    private var categories: [CleanupCategory] = []
    private let table = HistoryTable()
    private let scroll = NSScrollView()
    /// Paths on the window's shelf (drawn with a dot).
    var shelved: Set<String> = []
    /// Bundle IDs of running apps: their caches are left alone.
    private var running: Set<String> = []
    /// The first row stays selected while sizes come in and the order changes, until a
    /// key moves the selection.
    private var pinned = true

    var onKey: ((NSEvent) -> Bool)?
    var onSelect: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("cleanup"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.intercellSpacing = .zero
        table.style = .plain
        table.backgroundColor = .clear
        table.focusRingType = .none
        table.dataSource = self
        table.delegate = self
        table.onKey = { [weak self] event in self?.onKey?(event) ?? false }
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func focus() {
        window?.makeFirstResponder(table)
    }

    func applyTheme(_ theme: Theme) {
        table.rowHeight = Self.rowHeight
        table.reloadData()
    }

    // MARK: Data

    /// Shows `categories`, or the open category's `items`.
    func update(categories: [CleanupCategory], items: [CleanupItem]?) {
        running = Set(NSWorkspace.shared.runningApplications.compactMap(\.bundleIdentifier))
        self.categories = categories
        let selected = selectedKey
        if kind == nil {
            // Biggest first; the Trash always last.
            rows = categories
                .sorted { a, b in
                    if (a.kind == .trash) != (b.kind == .trash) { return b.kind == .trash }
                    return a.bytes > b.bytes
                }
                .map { .category($0) }
        } else if let items {
            var out: [Row] = []
            var lastGroup: String?
            for item in items {
                if let g = item.group, kind == .duplicates || kind == .buildOutput, g != lastGroup {
                    out.append(.header(headerTitle(g, items: items)))
                    lastGroup = g
                }
                out.append(.item(item))
            }
            rows = out
        }
        table.reloadData()
        let restore = pinned ? nil : selected.flatMap { key in rows.firstIndex { Self.key($0) == key } }
        let first = rows.firstIndex { if case .header = $0 { return false }; return true }
        if let row = restore ?? first {
            table.selectRowIndexes([row], byExtendingSelection: false)
            table.scrollRowToVisible(row)
        }
    }

    func open(_ kind: CleanupKind?) {
        self.kind = kind
        pinned = true
    }

    var category: CleanupCategory? { categories.first { $0.kind == kind } }

    func category(_ kind: CleanupKind) -> CleanupCategory? { categories.first { $0.kind == kind } }

    var selected: Row? {
        let i = table.selectedRow
        return i >= 0 && i < rows.count ? rows[i] : nil
    }

    var selectedItem: CleanupItem? {
        if case let .item(item)? = selected { return item }
        return nil
    }

    var items: [CleanupItem] {
        rows.compactMap { if case let .item(i) = $0 { return i }; return nil }
    }

    /// Whether an item is left alone: the copy of a duplicate set to keep, or the cache of
    /// an app that's running.
    func isProtected(_ item: CleanupItem, in kind: CleanupKind? = nil) -> Bool {
        item.keep || ((kind ?? self.kind) == .appCaches && item.group.map { running.contains($0) } == true)
    }

    private var selectedKey: String? { selected.map(Self.key) }

    private static func key(_ row: Row) -> String {
        switch row {
        case let .category(c): "c:\(c.kind)"
        case let .header(h): "h:\(h)"
        case let .item(i): "i:\(i.path)"
        }
    }

    func move(_ delta: Int) {
        pinned = false
        select(table.selectedRow + delta, step: delta < 0 ? -1 : 1)
    }

    /// Selects `row`, or the nearest item or category from there in the direction of `step`
    /// (headers aren't stops).
    func select(_ row: Int, step: Int = 1) {
        guard !rows.isEmpty else { return }
        if row > 0 { pinned = false }
        let isStop = { (i: Int) -> Bool in
            if case .header = self.rows[i] { return false }
            return true
        }
        var i = min(max(row, 0), rows.count - 1)
        while i >= 0, i < rows.count, !isStop(i) { i += step }
        if i < 0 || i >= rows.count {
            i = min(max(row, 0), rows.count - 1)
            while i >= 0, i < rows.count, !isStop(i) { i -= step }
        }
        guard i >= 0, i < rows.count else { return }
        table.selectRowIndexes([i], byExtendingSelection: false)
        table.scrollRowToVisible(i)
    }

    func refreshShelf() {
        table.reloadData()
    }

    private func headerTitle(_ group: String, items: [CleanupItem]) -> String {
        let members = items.filter { $0.group == group }
        let bytes = members.filter { !$0.keep }.reduce(UInt64(0)) { $0 + $1.bytes }
        switch kind {
        case .duplicates:
            let name = (members.first?.path as NSString?)?.lastPathComponent ?? ""
            let checking = members.contains { !$0.confirmed } ? " · checking…" : ""
            return "\(name) · \(members.count) copies · \(Self.bytes(bytes)) to free\(checking)"
        default:
            return "\(group) · \(Self.bytes(bytes))"
        }
    }

    /// For an app's cache: the app's name, and whether it's running.
    private func note(_ item: CleanupItem) -> String? {
        guard kind == .appCaches, let id = item.group else { return nil }
        if running.contains(id) { return "running, close it first" }
        return NSWorkspace.shared.urlForApplication(withBundleIdentifier: id)
            .map { FileManager.default.displayName(atPath: $0.path).replacingOccurrences(of: ".app", with: "") }
    }

    /// Two lines (name, then notes) and the bar under them.
    static var rowHeight: CGFloat {
        let theme = Theme.shared
        return (theme.list.pointSize * 1.45 + theme.small.pointSize * 1.45 + 16).rounded(.up)
    }

    static func bytes(_ n: UInt64) -> String {
        let f = ByteCountFormatter()
        f.countStyle = .file
        f.allowsNonnumericFormatting = false
        return f.string(fromByteCount: Int64(n))
    }

    static func title(_ kind: CleanupKind) -> String {
        switch kind {
        case .buildOutput: "Build output in idle projects"
        case .bigFiles: "Big files you haven't opened"
        case .oldDownloads: "Old downloads"
        case .duplicates: "Duplicates"
        case .appCaches: "App caches"
        case .trash: "Trash"
        }
    }

    static func explanation(_ kind: CleanupKind) -> String {
        switch kind {
        case .buildOutput: "node_modules, target, .venv… in projects you haven't touched: rebuilt on the next build"
        case .bigFiles: "large files not opened or changed in months"
        case .oldDownloads: "still in Downloads or on the Desktop"
        case .duplicates: "same contents; the copy marked keep stays"
        case .appCaches: "apps rebuild their caches (they may be slower once)"
        case .trash: "emptying it frees the space for good"
        }
    }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = tableView.makeView(withIdentifier: CleanupCell.identifier, owner: nil) as? CleanupCell ?? CleanupCell()
        let biggest = rows.reduce(UInt64(1)) { m, r in
            switch r {
            case let .category(c): max(m, c.bytes)
            case let .item(i): max(m, i.bytes)
            case .header: m
            }
        }
        var note: String?
        var protected = false
        if case let .item(item) = rows[row] {
            note = self.note(item)
            protected = isProtected(item)
        }
        cell.configure(rows[row], biggest: biggest, shelved: shelved, protected: protected, note: note)
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        if case .header = rows[row] { return false }
        return true
    }

    func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool {
        if case .header = rows[row] { return true }
        return false
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        if case .header = rows[row] { return Theme.shared.rowHeight }
        return Self.rowHeight
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        if case .header = rows[row] { return NSTableRowView() }
        return AlwaysEmphasizedRowView()
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        onSelect?()
    }
}

/// A category (name, explanation, total, progress) or an item (name, where, why, size),
/// with a bar for its share of the biggest.
@MainActor
final class CleanupCell: NSView {
    static let identifier = NSUserInterfaceItemIdentifier("CleanupCell")
    private var row: CleanupView.Row?
    private var biggest: UInt64 = 1
    private var shelved: Set<String> = []
    private var protected = false
    private var note: String?

    override var isFlipped: Bool { true }

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func configure(_ row: CleanupView.Row, biggest: UInt64, shelved: Set<String>, protected: Bool, note: String?) {
        self.row = row
        self.biggest = max(biggest, 1)
        self.shelved = shelved
        self.protected = protected
        self.note = note
        needsDisplay = true
    }

    private var selected: Bool { (superview as? NSTableRowView)?.isSelected == true }

    override func draw(_ dirtyRect: NSRect) {
        guard let row else { return }
        let theme = Theme.shared
        let c = theme.colors
        let text = selected ? c.selectionText : c.text
        let dim = selected ? c.selectionText.withAlphaComponent(0.7) : c.textDim
        let h = bounds.height
        let left: CGFloat = 26

        func draw(_ s: String, _ font: NSFont, _ color: NSColor, x: CGFloat, y: CGFloat, width: CGFloat,
                  truncate: NSLineBreakMode = .byTruncatingTail, align: NSTextAlignment = .left) {
            let para = NSMutableParagraphStyle()
            para.lineBreakMode = truncate
            para.alignment = align
            NSAttributedString(string: s, attributes: [.font: font, .foregroundColor: color, .paragraphStyle: para])
                .draw(with: NSRect(x: x, y: y, width: max(width, 0), height: font.pointSize * 1.5), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        }
        func bar(_ bytes: UInt64) {
            let share = CGFloat(Double(bytes) / Double(biggest))
            let rect = NSRect(x: left, y: h - 6, width: max((bounds.width - left - 14) * share, 2), height: 2.5)
            (selected ? c.selectionText.withAlphaComponent(0.5) : c.match.withAlphaComponent(0.55)).setFill()
            NSBezierPath(roundedRect: rect, xRadius: 1.25, yRadius: 1.25).fill()
        }

        switch row {
        case let .header(title):
            draw(title, theme.small, c.textDim, x: 14, y: (h - theme.small.pointSize * 1.3) / 2, width: bounds.width - 28)
        case let .category(cat):
            let size = cat.state == .waiting ? "…" : CleanupView.bytes(cat.bytes)
            let sizeWidth: CGFloat = 90
            draw(CleanupView.title(cat.kind), theme.listBold, text, x: left, y: 6, width: bounds.width - left - sizeWidth - 20)
            var note = CleanupView.explanation(cat.kind)
            switch cat.state {
            case .waiting: note = "waiting…"
            case .working where cat.kind == .duplicates && cat.total > 0:
                note = "checking contents · \(Int(Double(cat.done) / Double(max(cat.total, 1)) * 100))% · \(note)"
            case .working: note = "measuring… · \(note)"
            case .skipped: note = "not checked in Low Power Mode · r to check"
            case .ready: note = cat.count == 0 ? "nothing to suggest" : "\(cat.count) item\(cat.count == 1 ? "" : "s") · \(note)"
            }
            draw(note, theme.small, dim, x: left, y: 6 + theme.list.pointSize * 1.45, width: bounds.width - left - 14)
            draw(size, theme.listBold, text, x: bounds.width - sizeWidth - 14, y: 6, width: sizeWidth, align: .right)
            if cat.state != .waiting { bar(cat.bytes) }
        case let .item(item):
            if shelved.contains(item.path) {
                c.shelf.setFill()
                NSBezierPath(ovalIn: NSRect(x: 10, y: 9, width: 7, height: 7)).fill()
            }
            let name = (item.path as NSString).lastPathComponent + (item.isDir ? "/" : "")
            let sizeWidth: CGFloat = 90
            draw(name, theme.list, protected ? dim : text, x: left, y: 6, width: bounds.width - left - sizeWidth - 20,
                 truncate: .byTruncatingMiddle)
            var notes: [String] = []
            if item.keep { notes.append("keep") }
            if let note { notes.append(note) }
            if !item.confirmed { notes.append("checking…") }
            if !item.detail.isEmpty { notes.append(item.detail) }
            // APFS clones share their blocks: trashing one alone frees next to nothing.
            if !item.isDir, item.size > 1_000_000, item.bytes < item.size / 10 { notes.append("a clone, frees little on its own") }
            if let t = item.lastOpened {
                let date = Date(timeIntervalSince1970: TimeInterval(t)).formatted(date: .abbreviated, time: .omitted)
                notes.append(item.group != nil && item.isDir ? "project untouched since \(date)" : "opened \(date)")
            }
            notes.append((item.display as NSString).deletingLastPathComponent)
            draw(notes.joined(separator: " · "), theme.small, dim, x: left, y: 6 + theme.list.pointSize * 1.45, width: bounds.width - left - 14)
            draw(CleanupView.bytes(item.bytes), theme.list, protected ? dim : text, x: bounds.width - sizeWidth - 14, y: 6, width: sizeWidth, align: .right)
            if !item.keep { bar(item.bytes) }
        }
    }
}

/// Forwards Clean Up's progress (from its scan thread) to the main thread.
final class CleanupEvents: CleanupListener, @unchecked Sendable {
    // Only read and written on the main thread.
    nonisolated(unsafe) var onChange: (@MainActor (CleanupCategory) -> Void)?

    func cleanupChanged(category: CleanupCategory) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { self.onChange?(category) }
        }
    }
}
