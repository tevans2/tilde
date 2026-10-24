import AppKit
import TildeFFI

// MARK: Path bar

/// Breadcrumbs for the current folder followed by the search field.
@MainActor
final class PathBar: NSView {
    let field = FocusReportingField()
    private let crumbs = NSTextField(labelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        crumbs.lineBreakMode = .byTruncatingHead
        crumbs.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        crumbs.setContentHuggingPriority(.required, for: .horizontal)

        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 15)
        field.cell?.isScrollable = true
        field.cell?.wraps = false
        field.setContentHuggingPriority(.defaultLow, for: .horizontal)

        for v in [crumbs, field] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            crumbs.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            crumbs.centerYAnchor.constraint(equalTo: centerYAnchor),
            crumbs.widthAnchor.constraint(lessThanOrEqualTo: widthAnchor, multiplier: 0.6),
            field.leadingAnchor.constraint(equalTo: crumbs.trailingAnchor, constant: 4),
            field.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            field.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func setFolder(_ path: String, home: String) {
        var display = path
        if path == home {
            display = "~"
        } else if path.hasPrefix(home + "/") {
            display = "~" + path.dropFirst(home.count)
        }
        let parts = display.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
        let s = NSMutableAttributedString()
        let font = NSFont.systemFont(ofSize: 15)
        let sep: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.tertiaryLabelColor]
        let seg: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.secondaryLabelColor]
        if display.hasPrefix("/") { s.append(NSAttributedString(string: "/", attributes: sep)) }
        for part in parts {
            s.append(NSAttributedString(string: part, attributes: seg))
            s.append(NSAttributedString(string: " / ", attributes: sep))
        }
        crumbs.attributedStringValue = s
    }

    func setMode(_ mode: Mode) {
        field.placeholderString = mode == .go ? "type to search" : "press / to search"
        field.textColor = mode == .go ? .labelColor : .secondaryLabelColor
    }
}

/// A text field that reports when it becomes first responder.
final class FocusReportingField: NSTextField {
    var onFocus: (() -> Void)?

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok { onFocus?() }
        return ok
    }
}

// MARK: Results

/// The results list. Reports focus and gives SELECT-mode key handling first refusal.
final class ResultsTableView: NSTableView {
    var onFocus: (() -> Void)?
    var onKey: ((NSEvent) -> Bool)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("main"))
        column.resizingMask = .autoresizingMask
        addTableColumn(column)
        headerView = nil
        rowHeight = 26
        intercellSpacing = NSSize(width: 0, height: 0)
        style = .inset
        backgroundColor = .clear
        allowsEmptySelection = true
        focusRingType = .none
        columnAutoresizingStyle = .uniformColumnAutoresizingStyle
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok { onFocus?() }
        return ok
    }

    override func keyDown(with event: NSEvent) {
        if onKey?(event) != true { super.keyDown(with: event) }
    }
}

/// Keeps the selection blue while typing in the path bar (GO mode).
final class AlwaysEmphasizedRowView: NSTableRowView {
    override var isEmphasized: Bool {
        get { true }
        set {}
    }
}

final class RowCellView: NSTableCellView {
    static let identifier = NSUserInterfaceItemIdentifier("RowCell")

    private let badge = Badge()
    private let name = NSTextField(labelWithString: "")
    private let meta = NSTextField(labelWithString: "")
    private let age = NSTextField(labelWithString: "")
    private var row: RowView?

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
        name.lineBreakMode = .byTruncatingMiddle
        name.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        for f in [meta, age] {
            f.font = .monospacedDigitSystemFont(ofSize: 11.5, weight: .regular)
            f.textColor = .secondaryLabelColor
            f.alignment = .right
            f.setContentCompressionResistancePriority(.required, for: .horizontal)
        }
        for v in [badge, name, meta, age] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            badge.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
            badge.centerYAnchor.constraint(equalTo: centerYAnchor),
            badge.widthAnchor.constraint(equalToConstant: 36),
            badge.heightAnchor.constraint(equalToConstant: 16),
            name.leadingAnchor.constraint(equalTo: badge.trailingAnchor, constant: 10),
            name.centerYAnchor.constraint(equalTo: centerYAnchor),
            meta.leadingAnchor.constraint(greaterThanOrEqualTo: name.trailingAnchor, constant: 12),
            meta.centerYAnchor.constraint(equalTo: centerYAnchor),
            age.leadingAnchor.constraint(equalTo: meta.trailingAnchor, constant: 12),
            age.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            age.centerYAnchor.constraint(equalTo: centerYAnchor),
            age.widthAnchor.constraint(equalToConstant: 34),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func configure(with row: RowView) {
        self.row = row
        badge.set(text: Format.badgeText(row), color: Format.badgeColor(row.category))
        meta.stringValue = Format.meta(row)
        age.stringValue = Format.age(row.modified)
        renderName()
    }

    override var backgroundStyle: NSView.BackgroundStyle {
        didSet { renderName() }
    }

    private func renderName() {
        guard let row else { return }
        let selected = backgroundStyle == .emphasized
        let base: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 13),
            .foregroundColor: selected ? NSColor.alternateSelectedControlTextColor : NSColor.labelColor,
        ]
        let s = NSMutableAttributedString(string: row.name + (row.kind == .folder ? "/" : ""), attributes: base)
        let hit: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 13, weight: .bold),
            .foregroundColor: selected ? NSColor.systemYellow : NSColor.systemOrange,
        ]
        let length = (row.name as NSString).length
        for r in row.highlights where Int(r.start + r.length) <= length {
            s.addAttributes(hit, range: NSRange(location: Int(r.start), length: Int(r.length)))
        }
        name.attributedStringValue = s
        meta.textColor = selected ? .alternateSelectedControlTextColor : .secondaryLabelColor
        age.textColor = meta.textColor
    }
}

/// Small coloured type label ("PDF", "DIR").
final class Badge: NSView {
    private let label = NSTextField(labelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 4
        label.font = .systemFont(ofSize: 9, weight: .semibold)
        label.textColor = .white
        label.alignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: centerXAnchor),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            label.widthAnchor.constraint(lessThanOrEqualTo: widthAnchor, constant: -2),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private var color: NSColor = .systemGray

    func set(text: String, color: NSColor) {
        label.stringValue = text
        self.color = color
        needsDisplay = true
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        // Resolve the dynamic colour for the current appearance.
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = color.cgColor
        }
    }
}

// MARK: Status bar

@MainActor
final class StatusBar: NSView {
    private let modePill = NSTextField(labelWithString: "")
    private let pillBackground = NSView()
    private let hints = NSTextField(labelWithString: "")
    private let right = NSTextField(labelWithString: "")
    private var flashWork: DispatchWorkItem?
    private var lastSummary = ""

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        pillBackground.wantsLayer = true
        pillBackground.layer?.cornerRadius = 4
        modePill.font = .systemFont(ofSize: 10, weight: .bold)
        modePill.textColor = .white
        for f in [hints, right] {
            f.font = .systemFont(ofSize: 11.5)
            f.textColor = .secondaryLabelColor
            f.lineBreakMode = .byTruncatingTail
        }
        hints.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        for v in [pillBackground, modePill, hints, right] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            pillBackground.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            pillBackground.centerYAnchor.constraint(equalTo: centerYAnchor),
            pillBackground.heightAnchor.constraint(equalToConstant: 17),
            modePill.leadingAnchor.constraint(equalTo: pillBackground.leadingAnchor, constant: 7),
            modePill.trailingAnchor.constraint(equalTo: pillBackground.trailingAnchor, constant: -7),
            modePill.centerYAnchor.constraint(equalTo: pillBackground.centerYAnchor),
            hints.leadingAnchor.constraint(equalTo: pillBackground.trailingAnchor, constant: 10),
            hints.centerYAnchor.constraint(equalTo: centerYAnchor),
            right.leadingAnchor.constraint(greaterThanOrEqualTo: hints.trailingAnchor, constant: 12),
            right.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            right.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private var modeColor: NSColor = .systemBlue

    func update(mode: Mode, count: Int, filter: String) {
        switch mode {
        case .go:
            modePill.stringValue = "GO"
            modeColor = .systemBlue
            hints.stringValue = "↑↓ move   ⏎ open   tab or / go in   ⌫ up   esc select"
        case .select:
            modePill.stringValue = "SELECT"
            modeColor = .systemGreen
            hints.stringValue = "j k move   h l out / in   ⏎ open   g G top / bottom   / search   esc clear filter"
        }
        needsDisplay = true
        let items = "\(count) item\(count == 1 ? "" : "s")"
        lastSummary = filter.isEmpty ? items : "\(items) matching “\(filter)”"
        if flashWork == nil { right.stringValue = lastSummary }
    }

    /// Shows a message on the right for a few seconds.
    func flash(_ message: String) {
        flashWork?.cancel()
        right.stringValue = message
        right.textColor = .labelColor
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.flashWork = nil
            self.right.stringValue = self.lastSummary
            self.right.textColor = .secondaryLabelColor
        }
        flashWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: work)
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = NSColor.windowBackgroundColor.cgColor
            pillBackground.layer?.backgroundColor = modeColor.cgColor
        }
    }
}
