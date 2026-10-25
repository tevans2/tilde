import AppKit
import TildeFFI

/// A small panel over the window for one line of input: a folder picker (with fuzzy
/// results) for move and copy, or a plain text prompt for rename and new folder.
/// `⏎` confirms, `esc` cancels.
@MainActor
final class PromptOverlay: NSView, NSTextFieldDelegate, NSTableViewDataSource, NSTableViewDelegate {
    enum Kind {
        /// Fuzzy folder search; `onPick` gets the chosen folder path.
        case destination(search: (String) -> [Destination], onPick: (String) -> Void)
        /// Free text; `onSubmit` gets the text.
        case text(initial: String, selection: NSRange?, onSubmit: (String) -> Void)
    }

    private let kind: Kind
    private let onClose: () -> Void
    /// Set by whoever shows the overlay, to restore focus when it closes.
    var onCloseExtra: (() -> Void)?
    private let box = NSView()
    private let title = NSTextField(labelWithString: "")
    private let field = NSTextField()
    private let hint = NSTextField(labelWithString: "")
    private let table = NSTableView()
    private let scroll = NSScrollView()
    private var results: [Destination] = []

    init(title: String, kind: Kind, onClose: @escaping () -> Void = {}) {
        self.kind = kind
        self.onClose = onClose
        super.init(frame: .zero)
        self.title.stringValue = title
        build()
        switch kind {
        case .destination:
            refreshResults()
        case let .text(initial, _, _):
            field.stringValue = initial
            scroll.isHidden = true
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private func build() {
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.withAlphaComponent(0.25).cgColor

        box.wantsLayer = true
        box.layer?.cornerRadius = 10
        box.layer?.backgroundColor = Theme.shared.colors.bar.cgColor
        box.layer?.borderColor = Theme.shared.colors.border.cgColor
        box.layer?.borderWidth = 1
        box.shadow = NSShadow()
        box.layer?.shadowOpacity = 0.3
        box.layer?.shadowRadius = 16

        let theme = Theme.shared
        title.font = theme.small
        title.textColor = Theme.shared.colors.textDim
        field.textColor = Theme.shared.colors.text
        field.font = theme.bar
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.usesSingleLineMode = true
        field.cell?.wraps = false
        field.cell?.isScrollable = true
        field.delegate = self
        field.placeholderString = {
            if case .destination = kind { return "Type a folder, or a path like ~/Documents" }
            return ""
        }()
        hint.font = theme.small
        hint.textColor = .tertiaryLabelColor
        hint.stringValue = {
            if case .destination = kind { return "↑↓ choose   ⏎ confirm   esc cancel" }
            return "⏎ confirm   esc cancel"
        }()

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("dest"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = theme.rowHeight
        table.style = .plain
        table.backgroundColor = .clear
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(confirm)
        scroll.documentView = table
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true

        let separator = NSBox()
        separator.boxType = .custom
        separator.borderWidth = 0
        separator.fillColor = Theme.shared.colors.border
        for v in [title, field, separator, scroll, hint] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            box.addSubview(v)
        }
        box.translatesAutoresizingMaskIntoConstraints = false
        addSubview(box)

        let isText: Bool = { if case .text = kind { return true }; return false }()
        NSLayoutConstraint.activate([
            box.centerXAnchor.constraint(equalTo: centerXAnchor),
            box.topAnchor.constraint(equalTo: topAnchor, constant: 70),
            box.widthAnchor.constraint(equalTo: widthAnchor, multiplier: 0.55),
            box.widthAnchor.constraint(greaterThanOrEqualToConstant: 380),
            title.topAnchor.constraint(equalTo: box.topAnchor, constant: 10),
            title.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 14),
            title.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -14),
            field.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 6),
            field.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 14),
            field.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -14),
            separator.topAnchor.constraint(equalTo: field.bottomAnchor, constant: 8),
            separator.leadingAnchor.constraint(equalTo: box.leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: box.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: separator.bottomAnchor, constant: 4),
            scroll.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 6),
            scroll.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -6),
            scroll.heightAnchor.constraint(equalToConstant: isText ? 0 : 8 * Theme.shared.rowHeight),
            hint.topAnchor.constraint(equalTo: scroll.bottomAnchor, constant: 6),
            hint.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 14),
            hint.bottomAnchor.constraint(equalTo: box.bottomAnchor, constant: -10),
        ])
    }

    /// Puts the cursor in the field (selecting part of the text for rename).
    func focus() {
        window?.makeFirstResponder(field)
        if case let .text(_, selection?, _) = kind {
            field.currentEditor()?.selectedRange = selection
        }
    }

    // Clicking outside the box cancels.
    override func mouseDown(with event: NSEvent) {
        if !box.frame.contains(convert(event.locationInWindow, from: nil)) { close() }
    }

    private func close() {
        removeFromSuperview()
        onClose()
        onCloseExtra?()
    }

    @objc private func confirm() {
        switch kind {
        case let .destination(_, onPick):
            let row = table.selectedRow
            guard row >= 0, row < results.count else { return NSSound.beep() }
            let path = results[row].path
            close()
            onPick(path)
        case let .text(_, _, onSubmit):
            let text = field.stringValue
            close()
            onSubmit(text)
        }
    }

    private func refreshResults() {
        guard case let .destination(search, _) = kind else { return }
        results = search(field.stringValue)
        table.reloadData()
        if !results.isEmpty { table.selectRowIndexes([0], byExtendingSelection: false) }
    }

    func controlTextDidChange(_ obj: Notification) {
        refreshResults()
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            confirm()
        case #selector(NSResponder.cancelOperation(_:)):
            close()
        case #selector(NSResponder.moveDown(_:)):
            move(1)
        case #selector(NSResponder.moveUp(_:)):
            move(-1)
        default:
            return false
        }
        return true
    }

    /// For ⌃j / ⌃k from the window.
    func moveSelection(by delta: Int) {
        move(delta)
    }

    private func move(_ delta: Int) {
        guard !results.isEmpty else { return }
        let row = min(max(table.selectedRow + delta, 0), results.count - 1)
        table.selectRowIndexes([row], byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    // MARK: Results table

    func numberOfRows(in tableView: NSTableView) -> Int { results.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let label = NSTextField(labelWithString: "")
        label.lineBreakMode = .byTruncatingHead
        let d = results[row]
        // Long paths keep their end, which is the part that tells folders apart.
        let truncate = NSMutableParagraphStyle()
        truncate.lineBreakMode = .byTruncatingHead
        let s = NSMutableAttributedString(
            string: d.display,
            attributes: [.font: Theme.shared.list, .foregroundColor: Theme.shared.colors.text, .paragraphStyle: truncate])
        let length = (d.display as NSString).length
        for r in d.highlights where Int(r.start + r.length) <= length {
            s.addAttributes([.font: Theme.shared.listBold, .foregroundColor: Theme.shared.colors.match],
                            range: NSRange(location: Int(r.start), length: Int(r.length)))
        }
        label.attributedStringValue = s
        return label
    }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        AlwaysEmphasizedRowView()
    }
}
