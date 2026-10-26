import AppKit
import TildeFFI

/// History (`H`, Edit › History): the undo tree, in the window in place of the list and
/// preview. Operations are nodes on branches, newest at the top, with a "start" node at
/// the bottom; the current state is marked. The side shows what the selected operation
/// did and what `u` (undo it), `r` (redo it) and `⏎` (go to the state right after it)
/// would change, since undoing an old operation also undoes later ones on the same files.
@MainActor
final class HistoryView: NSView, NSTableViewDataSource, NSTableViewDelegate {
    /// Undo, redo or go to an entry (`nil`: the start). The window asks first when more
    /// than one operation is involved.
    var onUndo: ((HistoryEntry) -> Void)?
    var onRedo: ((HistoryEntry) -> Void)?
    var onGoto: ((HistoryEntry?) -> Void)?
    var onClose: (() -> Void)?
    var onSelect: (() -> Void)?

    private(set) var rows: [HistoryGraph.Row] = []
    private var ops: FileOps?
    private let table = HistoryTable()
    private let scroll = NSScrollView()
    private let details = NSTextView()
    private let detailsScroll = NSScrollView()
    private let separator = NSBox()

    override init(frame: NSRect) {
        super.init(frame: frame)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("node"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.intercellSpacing = .zero
        table.style = .plain
        table.backgroundColor = .clear
        table.focusRingType = .none
        table.dataSource = self
        table.delegate = self
        table.onKey = { [weak self] event in self?.handleKey(event) ?? false }
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false

        details.isEditable = false
        details.isSelectable = true
        details.drawsBackground = false
        details.textContainerInset = NSSize(width: 14, height: 12)
        details.isVerticallyResizable = true
        details.autoresizingMask = [.width]
        detailsScroll.documentView = details
        detailsScroll.hasVerticalScroller = true
        detailsScroll.drawsBackground = false
        separator.boxType = .custom
        separator.borderWidth = 0

        for v in [scroll, separator, detailsScroll] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.widthAnchor.constraint(equalTo: widthAnchor, multiplier: 0.55),
            separator.topAnchor.constraint(equalTo: topAnchor),
            separator.bottomAnchor.constraint(equalTo: bottomAnchor),
            separator.leadingAnchor.constraint(equalTo: scroll.trailingAnchor),
            separator.widthAnchor.constraint(equalToConstant: 1),
            detailsScroll.topAnchor.constraint(equalTo: topAnchor),
            detailsScroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            detailsScroll.leadingAnchor.constraint(equalTo: separator.trailingAnchor),
            detailsScroll.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func applyTheme(_ theme: Theme) {
        separator.fillColor = theme.colors.border
        table.rowHeight = (theme.rowHeight * 1.15).rounded()
        table.reloadData()
        updateDetails()
    }

    /// Re-reads the history, keeping the selection (or selecting the current state).
    func reload(_ ops: FileOps, selectCurrent: Bool = false) {
        self.ops = ops
        let selected = selectedRow?.entry?.id
        rows = HistoryGraph.layout(ops.history())
        table.reloadData()
        let current = rows.firstIndex { $0.isCurrent }
        let row = selectCurrent ? current : (selected.flatMap { id in rows.firstIndex { $0.entry?.id == id } } ?? current)
        if let row {
            table.selectRowIndexes([row], byExtendingSelection: false)
            table.scrollRowToVisible(row)
        }
        updateDetails()
    }

    func focus() {
        window?.makeFirstResponder(table)
    }

    var selectedRow: HistoryGraph.Row? {
        let i = table.selectedRow
        return i >= 0 && i < rows.count ? rows[i] : nil
    }

    // MARK: Keys

    private func handleKey(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if flags.contains(.command) { return false }
        let select = Keymap.shared.action(for: event, in: .select)
        if event.keyCode == 53 || select == .history || select == .back { onClose?(); return true }
        if event.keyCode == 36 || event.keyCode == 76 {
            if let row = selectedRow { onGoto?(row.entry) }
            return true
        }
        switch (event.charactersIgnoringModifiers, select) {
        case ("u", _), (_, .undo):
            if let e = selectedRow?.entry { onUndo?(e) }
        case ("r", _), (_, .redo):
            if let e = selectedRow?.entry { onRedo?(e) }
        case (_, .down): move(1)
        case (_, .up): move(-1)
        case (_, .top): move(-rows.count)
        case (_, .bottom): move(rows.count)
        default: return false
        }
        return true
    }

    private func move(_ delta: Int) {
        guard !rows.isEmpty else { return }
        let row = min(max(table.selectedRow + delta, 0), rows.count - 1)
        table.selectRowIndexes([row], byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    // MARK: Details

    private func updateDetails() {
        let theme = Theme.shared
        let c = theme.colors
        let s = NSMutableAttributedString()
        func add(_ text: String, _ font: NSFont = theme.small, _ color: NSColor = c.text) {
            s.append(NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color]))
        }
        func names(_ ids: [UInt64]) -> [String] {
            ids.map { id in rows.first { $0.entry?.id == id }?.entry?.label ?? "#\(id)" }
        }
        guard let row = selectedRow, let ops else {
            details.textStorage?.setAttributedString(s)
            return
        }
        guard let e = row.entry else {
            add("Start\n", theme.listBold)
            add("Before any of these operations.\n\n", theme.small, c.textDim)
            describeGoto(ops.previewGoto(entry: nil), add: add, names: names)
            details.textStorage?.setAttributedString(s)
            return
        }
        add("\(e.label)\n", theme.listBold)
        let state = switch e.state {
        case .applied: "Done"
        case .undone: "Undone"
        case .partlyUndone: "Partly undone (u undoes the rest)"
        }
        add("\(state) · \(Self.time(e.at))", theme.small, c.textDim)
        if e.changed != e.at { add(", last changed \(Self.time(e.changed))", theme.small, c.textDim) }
        if row.isCurrent { add("  · now", theme.small, c.select) }
        add("\n")
        if let parent = e.parent, let p = rows.first(where: { $0.entry?.id == parent })?.entry {
            add("Done after: \(p.label)\n", theme.small, c.textDim)
        }
        add("\n")
        if e.changes.isEmpty {
            add("(No record of the individual changes.)\n", theme.small, c.textDim)
        } else {
            for change in e.changes { add("\(change)\n") }
        }

        add("\n")
        if e.canUndo {
            let p = ops.previewUndo(entry: e.id)
            add("u  undo", theme.small, c.go)
            if p.undo.count > 1 {
                add(" this and \(p.undo.count - 1) later operation\(p.undo.count == 2 ? "" : "s") on the same files:\n", theme.small, c.textDim)
                for n in names(Array(p.undo.dropLast())) { add("     \(n)\n", theme.small, c.textDim) }
            } else {
                add(" this\n", theme.small, c.textDim)
            }
        }
        if e.canRedo {
            let p = ops.previewRedo(entry: e.id)
            add("r  redo", theme.small, c.go)
            if p.redo.count > 1 {
                add(" after \(p.redo.count - 1) earlier operation\(p.redo.count == 2 ? "" : "s") it builds on:\n", theme.small, c.textDim)
                for n in names(Array(p.redo.dropLast())) { add("     \(n)\n", theme.small, c.textDim) }
            } else {
                add(" this\n", theme.small, c.textDim)
            }
        }
        describeGoto(ops.previewGoto(entry: e.id), add: add, names: names)
        details.textStorage?.setAttributedString(s)
    }

    private func describeGoto(
        _ p: HistoryPreview, add: (String, NSFont, NSColor) -> Void, names: ([UInt64]) -> [String]
    ) {
        let theme = Theme.shared
        let c = theme.colors
        add("⏎  go to this state", theme.small, c.go)
        if let problem = p.problem {
            add(": \(problem)\n", theme.small, .systemOrange)
            return
        }
        if p.undo.isEmpty && p.redo.isEmpty {
            add(": you're here\n", theme.small, c.textDim)
            return
        }
        add(":\n", theme.small, c.textDim)
        for n in names(p.undo) { add("     undo  \(n)\n", theme.small, c.textDim) }
        for n in names(p.redo) { add("     redo  \(n)\n", theme.small, c.textDim) }
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

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = tableView.makeView(withIdentifier: HistoryNodeView.identifier, owner: nil) as? HistoryNodeView ?? HistoryNodeView()
        cell.row = rows[row]
        return cell
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        AlwaysEmphasizedRowView()
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        updateDetails()
        onSelect?()
    }
}

/// The table, passing keys to the view first.
final class HistoryTable: NSTableView {
    var onKey: ((NSEvent) -> Bool)?

    override func keyDown(with event: NSEvent) {
        if onKey?(event) != true { super.keyDown(with: event) }
    }
}

/// Lays the history out as a graph, newest first: each operation gets a lane (column);
/// branches open lanes beside the one they leave, and join back where they started.
enum HistoryGraph {
    struct Row {
        /// `nil`: the start (before everything).
        let entry: HistoryEntry?
        let isCurrent: Bool
        let lane: Int
        /// Lanes with a line coming down into this row from above, and going on below it.
        let above: Set<Int>
        let below: Set<Int>
        /// Lanes that end here, joining this row's node from above (branches meeting).
        let joins: Set<Int>
        let lanes: Int
    }

    static func layout(_ entries: [HistoryEntry]) -> [Row] {
        // Newest first, by when they were first done.
        let sorted = entries.sorted { ($0.at, $0.id) > ($1.at, $1.id) }
        let ids = Set(sorted.map(\.id))
        let start: UInt64 = 0
        let anyCurrent = sorted.contains { $0.isHead }
        // Each lane waits for the id of the next node down it.
        var lanes: [UInt64?] = []
        var rows: [Row] = []
        var width = 0
        for item in sorted.map({ Optional($0) }) + [nil] {
            let id = item?.id ?? start
            let above = Set(lanes.indices.filter { lanes[$0] != nil })
            let incoming = lanes.indices.filter { lanes[$0] == id }
            let lane = incoming.first ?? lanes.firstIndex(where: { $0 == nil }) ?? lanes.count
            if lane == lanes.count { lanes.append(nil) }
            for i in incoming.dropFirst() { lanes[i] = nil }
            // The parent continues the lane; the start ends it.
            let parent = item?.parent.flatMap { ids.contains($0) ? $0 : nil } ?? start
            lanes[lane] = item == nil ? nil : parent
            while lanes.last == .some(nil) { lanes.removeLast() }
            let below = Set(lanes.indices.filter { lanes[$0] != nil })
            width = max(width, lane + 1, (above.max() ?? -1) + 1)
            rows.append(Row(
                entry: item, isCurrent: item.map(\.isHead) ?? !anyCurrent, lane: lane,
                above: above, below: below, joins: Set(incoming.dropFirst()), lanes: 0))
        }
        return rows.map { Row(entry: $0.entry, isCurrent: $0.isCurrent, lane: $0.lane, above: $0.above,
                              below: $0.below, joins: $0.joins, lanes: width) }
    }
}

/// One node: the graph lines, the node (by state), the label and the time.
@MainActor
final class HistoryNodeView: NSView {
    static let identifier = NSUserInterfaceItemIdentifier("HistoryNode")
    private static let laneWidth: CGFloat = 16

    var row: HistoryGraph.Row? { didSet { needsDisplay = true } }

    override var isFlipped: Bool { true }

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private var selected: Bool {
        (superview as? NSTableRowView)?.isSelected == true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let row else { return }
        let theme = Theme.shared
        let c = theme.colors
        let h = bounds.height
        let mid = h / 2
        let x = { (lane: Int) in 14 + CGFloat(lane) * Self.laneWidth }
        let line = selected ? c.selectionText.withAlphaComponent(0.5) : c.textDim.withAlphaComponent(0.55)

        // Lines: straight through for lanes passing by, into the node from above and out
        // below, and curves for branches joining the node.
        let path = NSBezierPath()
        path.lineWidth = 1.5
        for lane in 0..<row.lanes where lane != row.lane {
            if row.above.contains(lane), row.below.contains(lane) {
                path.move(to: NSPoint(x: x(lane), y: 0))
                path.line(to: NSPoint(x: x(lane), y: h))
            }
        }
        if row.above.contains(row.lane) {
            path.move(to: NSPoint(x: x(row.lane), y: 0))
            path.line(to: NSPoint(x: x(row.lane), y: mid))
        }
        if row.below.contains(row.lane) {
            path.move(to: NSPoint(x: x(row.lane), y: mid))
            path.line(to: NSPoint(x: x(row.lane), y: h))
        }
        for lane in row.joins {
            path.move(to: NSPoint(x: x(lane), y: 0))
            path.curve(to: NSPoint(x: x(row.lane), y: mid),
                       controlPoint1: NSPoint(x: x(lane), y: mid * 0.8),
                       controlPoint2: NSPoint(x: x(row.lane) + (x(lane) - x(row.lane)) * 0.3, y: mid))
        }
        line.setStroke()
        path.stroke()

        // The node: filled when in place, hollow when undone, half when partly undone;
        // ringed for the current state.
        let r: CGFloat = row.isCurrent ? 5 : 4
        let node = NSRect(x: x(row.lane) - r, y: mid - r, width: r * 2, height: r * 2)
        let state = row.entry?.state
        let color: NSColor = switch state {
        case .applied: c.select
        case .partlyUndone: .systemOrange
        case .undone: c.textDim
        case nil: c.go
        }
        (theme.colors.background).setFill()
        NSBezierPath(ovalIn: node).fill()
        color.setStroke()
        color.setFill()
        let oval = NSBezierPath(ovalIn: node)
        oval.lineWidth = 1.5
        switch state {
        case .undone: oval.stroke()
        case .partlyUndone:
            oval.stroke()
            let half = NSBezierPath()
            half.appendArc(withCenter: NSPoint(x: node.midX, y: node.midY), radius: r, startAngle: 90, endAngle: 270)
            half.close()
            half.fill()
        default: oval.fill()
        }
        if row.isCurrent {
            let ring = NSBezierPath(ovalIn: node.insetBy(dx: -3, dy: -3))
            ring.lineWidth = 1.5
            c.go.setStroke()
            ring.stroke()
        }

        // Text: label, then (dim) the first change; the time on the right.
        let textX = x(max(row.lanes - 1, 0)) + 18
        let textColor = selected ? c.selectionText : c.text
        let dim = selected ? c.selectionText.withAlphaComponent(0.7) : c.textDim
        var labelAttrs: [NSAttributedString.Key: Any] = [.font: theme.list, .foregroundColor: textColor]
        if state == .undone {
            labelAttrs[.foregroundColor] = dim
            labelAttrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
        }
        let time = NSAttributedString(
            string: row.entry.map { HistoryView.time($0.at) } ?? "",
            attributes: [.font: theme.small, .foregroundColor: dim])
        let timeSize = time.size()
        time.draw(at: NSPoint(x: bounds.width - timeSize.width - 10, y: (h - timeSize.height) / 2))

        let text = NSMutableAttributedString(string: row.entry?.label ?? "Start", attributes: labelAttrs)
        if row.isCurrent {
            text.append(NSAttributedString(string: "  now", attributes: [.font: theme.small, .foregroundColor: selected ? textColor : c.go]))
        }
        if let first = row.entry?.changes.first {
            text.append(NSAttributedString(string: "   \(first)", attributes: [.font: theme.small, .foregroundColor: dim]))
        }
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingTail
        text.addAttribute(.paragraphStyle, value: para, range: NSRange(location: 0, length: text.length))
        let textHeight = text.size().height
        text.draw(with: NSRect(x: textX, y: (h - textHeight) / 2, width: bounds.width - textX - timeSize.width - 20, height: textHeight),
                  options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }
}
