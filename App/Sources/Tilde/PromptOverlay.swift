import AppKit
import TildeFFI

/// One entry in the keys panel: what it does, its keys, and how to run it.
struct CommandItem {
    let label: String
    let keys: String
    let run: @MainActor () -> Void
}

struct CommandGroup {
    let title: String
    let items: [CommandItem]
}

/// A small panel over the window for one line of input: a folder picker (with fuzzy
/// results) for move and copy, a plain text prompt for rename, new folder and tags, a
/// choice of a few answers by key (what to do about name clashes), or the keys panel.
/// `⏎` confirms, `esc` cancels.
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
        /// Every key for where the window is, grouped: typing filters, `⏎` or a click runs
        /// the entry (after the panel closes).
        case commands(groups: [CommandGroup])
    }

    /// The keys panel's rows: group titles and entries.
    private enum CommandRow {
        case header(String)
        case item(CommandItem)
    }

    private var commandRows: [CommandRow] = []

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
    private let customPlaceholder: String?
    /// In a panel of its own (the file-dialog helper): no dimming, filling the panel.
    private let floating: Bool

    /// `hint` replaces the usual line of keys under the field, `placeholder` the text in
    /// the empty field.
    init(
        title: String, kind: Kind, hint: String? = nil, placeholder: String? = nil, floating: Bool = false,
        onClose: @escaping () -> Void = {}
    ) {
        self.kind = kind
        self.onClose = onClose
        customHint = hint
        customPlaceholder = placeholder
        self.floating = floating
        super.init(frame: .zero)
        self.title.stringValue = title
        build()
        switch kind {
        case .destination, .commands:
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
        switch kind {
        case .destination, .commands: true
        default: false
        }
    }

    private var isCommands: Bool {
        if case .commands = kind { return true }
        return false
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private func build() {
        wantsLayer = true
        layer?.backgroundColor = floating ? NSColor.clear.cgColor : NSColor.black.withAlphaComponent(0.25).cgColor

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
        field.placeholderString = customPlaceholder ?? {
            if case .destination = kind { return "Type a folder, or a path like ~/Documents" }
            if case .commands = kind { return "Type to find a key or an action" }
            return ""
        }()
        hint.font = theme.small
        hint.textColor = .tertiaryLabelColor
        hint.stringValue = customHint ?? {
            switch kind {
            case .destination: return "↑↓ choose   ⏎ confirm   esc cancel"
            case .text: return "⏎ confirm   esc cancel"
            case .choice: return "press a key   esc cancel"
            case .commands: return "↑↓ choose   ⏎ or click to run   esc close"
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
        // In the keys panel one click runs an entry, as a menu does.
        if case .commands = kind { table.action = #selector(clicked) }
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

        let isText = !hasList
        let isChoice: Bool = { if case .choice = kind { return true }; return false }()
        NSLayoutConstraint.activate([
            box.centerXAnchor.constraint(equalTo: centerXAnchor),
            box.topAnchor.constraint(equalTo: topAnchor, constant: floating ? 16 : 70),
            box.widthAnchor.constraint(equalTo: widthAnchor, multiplier: floating ? 0.94 : 0.55),
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
            scroll.heightAnchor.constraint(equalToConstant: isText ? 0 : (isCommands ? 14 : 8) * Theme.shared.rowHeight),
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
        case .commands:
            let row = table.selectedRow
            guard row >= 0, row < commandRows.count, case let .item(item) = commandRows[row] else { return NSSound.beep() }
            close()
            item.run()
        }
    }

    @objc private func clicked() {
        let row = table.clickedRow
        guard row >= 0, row < commandRows.count, case .item = commandRows[row] else { return }
        table.selectRowIndexes([row], byExtendingSelection: false)
        confirm()
    }

    private func refreshResults() {
        if case let .commands(groups) = kind {
            // Every word typed must appear in the entry or its keys.
            let words = field.stringValue.lowercased().split(separator: " ")
            commandRows = groups.flatMap { group -> [CommandRow] in
                let items = group.items.filter { item in
                    let text = "\(item.label) \(item.keys)".lowercased()
                    return words.allSatisfy { text.contains($0) }
                }
                return items.isEmpty ? [] : [.header(group.title)] + items.map(CommandRow.item)
            }
            table.reloadData()
            if let first = commandRows.firstIndex(where: { if case .item = $0 { true } else { false } }) {
                table.selectRowIndexes([first], byExtendingSelection: false)
                table.scrollRowToVisible(0)
            }
            return
        }
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
        if isCommands {
            // Over the group titles.
            var row = table.selectedRow + delta
            while row >= 0, row < commandRows.count, case .header = commandRows[row] { row += delta }
            guard row >= 0, row < commandRows.count else { return }
            table.selectRowIndexes([row], byExtendingSelection: false)
            table.scrollRowToVisible(row == 1 ? 0 : row)
            return
        }
        guard !results.isEmpty else { return }
        let row = min(max(table.selectedRow + delta, 0), results.count - 1)
        table.selectRowIndexes([row], byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    // MARK: Results table

    func numberOfRows(in tableView: NSTableView) -> Int { isCommands ? commandRows.count : results.count }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        guard isCommands else { return true }
        if case .item = commandRows[row] { return true }
        return false
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        guard isCommands, case .header = commandRows[row] else { return Theme.shared.rowHeight }
        return Theme.shared.rowHeight * 0.85
    }

    /// An entry: what it does, and its keys on the right; or a group's title.
    private func commandView(_ row: CommandRow) -> NSView {
        let theme = Theme.shared
        switch row {
        case let .header(title):
            let label = NSTextField(labelWithString: title.uppercased())
            label.font = theme.badge
            label.textColor = theme.colors.textDim
            let box = NSView()
            label.translatesAutoresizingMaskIntoConstraints = false
            box.addSubview(label)
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 8),
                label.bottomAnchor.constraint(equalTo: box.bottomAnchor, constant: -3),
            ])
            return box
        case let .item(item):
            let label = NSTextField(labelWithString: item.label)
            label.font = .systemFont(ofSize: theme.small.pointSize + 0.5)
            label.textColor = theme.colors.text
            label.lineBreakMode = .byTruncatingTail
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            let keys = NSTextField(labelWithString: item.keys)
            keys.font = theme.small
            keys.textColor = theme.colors.match
            keys.alignment = .right
            keys.setContentCompressionResistancePriority(.required, for: .horizontal)
            let box = NSView()
            for v in [label, keys] {
                v.translatesAutoresizingMaskIntoConstraints = false
                box.addSubview(v)
            }
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 16),
                label.centerYAnchor.constraint(equalTo: box.centerYAnchor),
                keys.leadingAnchor.constraint(greaterThanOrEqualTo: label.trailingAnchor, constant: 12),
                keys.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -10),
                keys.centerYAnchor.constraint(equalTo: box.centerYAnchor),
            ])
            return box
        }
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        if isCommands { return commandView(commandRows[row]) }
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
