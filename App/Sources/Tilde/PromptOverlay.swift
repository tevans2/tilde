import AppKit
import TildeFFI

/// A small panel over the window for one line of input: a folder picker (with fuzzy
/// results) for move and copy, a plain text prompt for rename, new folder and tags, or a
/// choice of a few answers by key (what to do about name clashes). `⏎` confirms, `esc`
/// cancels.
@MainActor
final class PromptOverlay: NSView, NSTextFieldDelegate, NSTableViewDataSource, NSTableViewDelegate {
    enum Kind {
        /// Fuzzy folder search; `onPick` gets the chosen folder path.
        case destination(search: (String) -> [Destination], onPick: (String) -> Void)
        /// Free text; `onSubmit` gets the text.
        case text(initial: String, selection: NSRange?, onSubmit: (String) -> Void)
        /// One of `options` (key, label), chosen by its key or a click; `⏎` picks the
        /// first. `onChoose` gets the option's index.
        case choice(options: [(key: String, label: String)], onChoose: (Int) -> Void)
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
    private let buttons = NSStackView()
    private let customHint: String?

    /// `hint` replaces the usual line of keys under the field.
    init(title: String, kind: Kind, hint: String? = nil, onClose: @escaping () -> Void = {}) {
        self.kind = kind
        self.onClose = onClose
        customHint = hint
        super.init(frame: .zero)
        self.title.stringValue = title
        build()
        switch kind {
        case .destination:
            refreshResults()
        case let .text(initial, _, _):
            field.stringValue = initial
            scroll.isHidden = true
        case let .choice(options, _):
            field.isHidden = true
            scroll.isHidden = true
            for (i, option) in options.enumerated() {
                let button = NSButton(title: "\(option.label)  \(option.key)", target: self, action: #selector(chose(_:)))
                button.tag = i
                button.bezelStyle = .rounded
                button.keyEquivalent = i == 0 ? "\r" : ""
                buttons.addArrangedSubview(button)
            }
        }
    }

    /// Has a list of results that up and down move through.
    var hasList: Bool {
        if case .destination = kind { return true }
        return false
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
        hint.stringValue = customHint ?? {
            switch kind {
            case .destination: return "↑↓ choose   ⏎ confirm   esc cancel"
            case .text: return "⏎ confirm   esc cancel"
            case .choice: return "press a key   esc cancel"
            }
        }()
        title.lineBreakMode = .byWordWrapping
        title.maximumNumberOfLines = 3
        title.preferredMaxLayoutWidth = 500
        buttons.orientation = .horizontal
        buttons.spacing = 8

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
        for v in [title, field, separator, scroll, buttons, hint] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            box.addSubview(v)
        }
        box.translatesAutoresizingMaskIntoConstraints = false
        addSubview(box)

        let isText: Bool = { if case .destination = kind { return false }; return true }()
        let isChoice: Bool = { if case .choice = kind { return true }; return false }()
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
            field.heightAnchor.constraint(equalToConstant: isChoice ? 0 : field.intrinsicContentSize.height),
            buttons.topAnchor.constraint(equalTo: field.bottomAnchor, constant: isChoice ? 4 : 0),
            buttons.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 14),
            buttons.heightAnchor.constraint(equalToConstant: isChoice ? 28 : 0),
            separator.topAnchor.constraint(equalTo: buttons.bottomAnchor, constant: 8),
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
        if case .choice = kind {
            window?.makeFirstResponder(self)
            return
        }
        window?.makeFirstResponder(field)
        if case let .text(_, selection?, _) = kind {
            field.currentEditor()?.selectedRange = selection
        }
    }

    override var acceptsFirstResponder: Bool {
        if case .choice = kind { return true }
        return false
    }

    /// Choices are picked by their key.
    override func keyDown(with event: NSEvent) {
        guard case let .choice(options, _) = kind else { return super.keyDown(with: event) }
        if event.keyCode == 53 { return close() }
        if event.keyCode == 36 || event.keyCode == 76 { return choose(0) }
        let typed = event.charactersIgnoringModifiers?.lowercased()
        if let i = options.firstIndex(where: { $0.key == typed }) { choose(i) } else { NSSound.beep() }
    }

    @objc private func chose(_ sender: NSButton) {
        choose(sender.tag)
    }

    private func choose(_ index: Int) {
        guard case let .choice(_, onChoose) = kind else { return }
        close()
        onChoose(index)
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
        case .choice:
            choose(0)
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
