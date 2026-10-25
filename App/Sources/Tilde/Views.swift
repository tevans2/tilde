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
        crumbs.maximumNumberOfLines = 1
        crumbs.usesSingleLineMode = true
        crumbs.cell?.wraps = false
        crumbs.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        crumbs.setContentHuggingPriority(.required, for: .horizontal)

        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
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
        let font = Theme.shared.bar
        // Attributed strings ignore the field's line break mode; deep paths keep their end.
        let truncate = NSMutableParagraphStyle()
        truncate.lineBreakMode = .byTruncatingHead
        let c = Theme.shared.colors
        let sep: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: c.textDim.withAlphaComponent(0.55), .paragraphStyle: truncate]
        let seg: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: c.textDim, .paragraphStyle: truncate]
        if display.hasPrefix("/") { s.append(NSAttributedString(string: "/", attributes: sep)) }
        for part in parts {
            s.append(NSAttributedString(string: part, attributes: seg))
            s.append(NSAttributedString(string: " / ", attributes: sep))
        }
        crumbs.attributedStringValue = s
    }

    /// `@ Activity`, then the filter (`/ Downloaded`, `/ Inbox`).
    func setActivity(_ filter: ActivityFilter) {
        let font = Theme.shared.bar
        let c = Theme.shared.colors
        let s = NSMutableAttributedString(string: "@ Activity", attributes: [.font: font, .foregroundColor: c.text])
        let sep: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: c.textDim.withAlphaComponent(0.55)]
        let seg: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: c.textDim]
        var parts: [String] = []
        if filter.origin != .all { parts.append(Format.filterName(filter.origin)) }
        if filter.inbox { parts.append("Inbox") }
        for part in parts {
            s.append(NSAttributedString(string: " / ", attributes: sep))
            s.append(NSAttributedString(string: part, attributes: seg))
        }
        s.append(NSAttributedString(string: " / ", attributes: sep))
        crumbs.attributedStringValue = s
    }

    func applyTheme(_ theme: Theme) {
        field.font = theme.bar
        wantsLayer = true
        needsDisplay = true
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = Theme.shared.colors.bar.cgColor
        }
    }

    func setMode(_ mode: Mode) {
        field.placeholderString = switch mode {
        case .go: "type to search"
        case .select: "press / to search"
        case .edit: "editing this folder as text"
        }
        field.textColor = mode == .go ? Theme.shared.colors.text : Theme.shared.colors.textDim
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
        intercellSpacing = NSSize(width: 0, height: 0)
        style = .inset
        backgroundColor = .clear
        allowsEmptySelection = true
        focusRingType = .none
        columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        floatsGroupRows = false
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

    /// The theme's selection colour instead of the system accent.
    override func drawSelection(in dirtyRect: NSRect) {
        Theme.shared.colors.selection.setFill()
        NSBezierPath(roundedRect: bounds.insetBy(dx: 6, dy: 1), xRadius: 6, yRadius: 6).fill()
    }
}

final class RowCellView: NSTableCellView {
    static let identifier = NSUserInterfaceItemIdentifier("RowCell")

    private let shelfDot = NSTextField(labelWithString: "●")
    /// Activity rows: where the item came from.
    private let origin = NSTextField(labelWithString: "")
    private var originWidth: NSLayoutConstraint!
    private let badge = Badge()
    private let name = NSTextField(labelWithString: "")
    /// Where an Everywhere result lives; trimmed from the start so the nearest folders stay.
    private let location = NSTextField(labelWithString: "")
    private let meta = NSTextField(labelWithString: "")
    private let age = NSTextField(labelWithString: "")
    private var row: RowView?

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
        name.lineBreakMode = .byTruncatingTail
        name.maximumNumberOfLines = 1
        name.cell?.wraps = false
        name.cell?.truncatesLastVisibleLine = true
        // The name gives way only after the location has.
        name.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(740), for: .horizontal)
        location.lineBreakMode = .byTruncatingHead
        location.maximumNumberOfLines = 1
        location.cell?.wraps = false
        location.setContentCompressionResistancePriority(NSLayoutConstraint.Priority(250), for: .horizontal)
        location.setContentHuggingPriority(.defaultLow, for: .horizontal)
        for f in [meta, age] {
            f.textColor = .secondaryLabelColor
            f.alignment = .right
            f.setContentCompressionResistancePriority(.required, for: .horizontal)
        }
        shelfDot.textColor = Theme.shared.colors.shelf
        shelfDot.font = .systemFont(ofSize: 8)
        shelfDot.alignment = .center
        origin.lineBreakMode = .byClipping
        for v in [shelfDot, origin, badge, name, location, meta, age] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            shelfDot.leadingAnchor.constraint(equalTo: leadingAnchor),
            shelfDot.centerYAnchor.constraint(equalTo: centerYAnchor),
            shelfDot.widthAnchor.constraint(equalToConstant: 12),
            origin.leadingAnchor.constraint(equalTo: shelfDot.trailingAnchor, constant: 2),
            origin.centerYAnchor.constraint(equalTo: centerYAnchor),
            badge.leadingAnchor.constraint(equalTo: origin.trailingAnchor, constant: 2),
            badge.centerYAnchor.constraint(equalTo: centerYAnchor),
            badge.widthAnchor.constraint(equalToConstant: 40),
            badge.heightAnchor.constraint(equalTo: heightAnchor, multiplier: 0.62),
            name.leadingAnchor.constraint(equalTo: badge.trailingAnchor, constant: 10),
            name.centerYAnchor.constraint(equalTo: centerYAnchor),
            location.leadingAnchor.constraint(equalTo: name.trailingAnchor, constant: 10),
            location.firstBaselineAnchor.constraint(equalTo: name.firstBaselineAnchor),
            meta.leadingAnchor.constraint(greaterThanOrEqualTo: location.trailingAnchor, constant: 12),
            meta.centerYAnchor.constraint(equalTo: centerYAnchor),
            age.leadingAnchor.constraint(equalTo: meta.trailingAnchor, constant: 12),
            age.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            age.centerYAnchor.constraint(equalTo: centerYAnchor),
            age.widthAnchor.constraint(greaterThanOrEqualToConstant: 34),
        ])
        originWidth = origin.widthAnchor.constraint(equalToConstant: 0)
        originWidth.isActive = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func configure(with row: RowView, onShelf: Bool = false) {
        self.row = row
        shelfDot.isHidden = !onShelf
        let theme = Theme.shared
        meta.font = theme.small
        age.font = theme.small
        origin.font = theme.small
        if let o = row.origin {
            origin.stringValue = Format.originName(o)
            // Wide enough for the longest label, so names line up.
            originWidth.constant = ("Screenshot " as NSString).size(withAttributes: [.font: theme.small]).width.rounded(.up)
        } else {
            origin.stringValue = ""
            originWidth.constant = 0
        }
        badge.set(text: Format.badgeText(row), color: Format.badgeColor(row.category), font: theme.badge)
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
        let c = Theme.shared.colors
        shelfDot.textColor = c.shelf
        // Attributed strings ignore the field's line break mode, so set it here too.
        let oneLine = NSMutableParagraphStyle()
        oneLine.lineBreakMode = .byTruncatingTail
        let base: [NSAttributedString.Key: Any] = [
            .font: Theme.shared.list,
            .foregroundColor: selected ? c.selectionText : c.text,
            .paragraphStyle: oneLine,
        ]
        let s = NSMutableAttributedString(string: row.name + (row.kind == .folder ? "/" : ""), attributes: base)
        let hit: [NSAttributedString.Key: Any] = [
            .font: Theme.shared.listBold,
            .foregroundColor: selected ? c.matchSelected : c.match,
        ]
        let length = (row.name as NSString).length
        for r in row.highlights where Int(r.start + r.length) <= length {
            s.addAttributes(hit, range: NSRange(location: Int(r.start), length: Int(r.length)))
        }
        if let o = row.origin { origin.textColor = selected ? c.selectionText : Format.originColor(o) }
        location.isHidden = row.location.isEmpty
        if !row.location.isEmpty {
            // Everywhere and Activity rows show where the item lives, dimmed, with matched
            // characters.
            let truncateHead = NSMutableParagraphStyle()
            truncateHead.lineBreakMode = .byTruncatingHead
            let dim: [NSAttributedString.Key: Any] = [
                .font: Theme.shared.small,
                .foregroundColor: selected ? c.selectionText.withAlphaComponent(0.7) : c.textDim,
                .paragraphStyle: truncateHead,
            ]
            let location = NSMutableAttributedString(string: row.location, attributes: dim)
            let locationHit: [NSAttributedString.Key: Any] = [
                .font: Theme.shared.small.bold,
                .foregroundColor: selected ? c.matchSelected : c.match,
            ]
            let locationLength = (row.location as NSString).length
            for r in row.locationHighlights where Int(r.start + r.length) <= locationLength {
                location.addAttributes(locationHit, range: NSRange(location: Int(r.start), length: Int(r.length)))
            }
            if !row.source.isEmpty {
                location.append(NSAttributedString(string: " · \(row.source)", attributes: dim))
            }
            if row.filed {
                var filed = dim
                filed[.foregroundColor] = selected ? c.selectionText : c.select
                location.append(NSAttributedString(string: " · filed", attributes: filed))
            }
            self.location.attributedStringValue = location
        }
        name.attributedStringValue = s
        meta.textColor = selected ? c.selectionText : c.textDim
        age.textColor = meta.textColor
    }
}

/// The "Everywhere" divider between folder results and index results.
final class SectionHeaderView: NSTableCellView {
    static let identifier = NSUserInterfaceItemIdentifier("SectionHeader")
    private let label = NSTextField(labelWithString: "")

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
        label.textColor = .secondaryLabelColor
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            label.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -3),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func configure(title: String, count: Int) {
        label.font = Theme.shared.badge
        label.textColor = Theme.shared.colors.textDim
        label.stringValue = "\(title.uppercased())  \(count.formatted())"
    }
}

/// Small coloured type label ("PDF", "DIR").
final class Badge: NSView {
    private let label = NSTextField(labelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 4
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

    func set(text: String, color: NSColor, font: NSFont) {
        label.font = font
        label.textColor = Theme.shared.colors.badgeLabel
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

// MARK: Shelf bar

/// What's collected with `x`, and the actions that apply to it.
@MainActor
final class ShelfBar: NSView {
    private let label = NSTextField(labelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        label.lineBreakMode = .byTruncatingMiddle
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func update(paths: [String]) {
        let theme = Theme.shared
        let names = paths.prefix(4).map { ($0 as NSString).lastPathComponent }.joined(separator: ", ")
        let more = paths.count > 4 ? " +\(paths.count - 4)" : ""
        let s = NSMutableAttributedString(
            string: "SHELF \(paths.count)  ",
            attributes: [.font: theme.badge, .foregroundColor: theme.colors.shelf])
        s.append(NSAttributedString(string: names + more, attributes: [.font: theme.small, .foregroundColor: theme.colors.text]))
        s.append(NSAttributedString(
            string: "    m move  c copy  d trash  z zip  y paths  esc clear",
            attributes: [.font: theme.small, .foregroundColor: theme.colors.textDim]))
        label.attributedStringValue = s
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = Theme.shared.colors.shelf.withAlphaComponent(0.1).cgColor
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
    /// A running operation ("Copying 2/5 · file.dmg"); shown instead of the summary.
    private var activity: String?
    private var lastSummary = ""

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        pillBackground.wantsLayer = true
        pillBackground.layer?.cornerRadius = 4
        modePill.textColor = .white
        for f in [hints, right] {
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

    private var mode: Mode = .go

    func applyTheme(_ theme: Theme) {
        modePill.font = theme.badge
        hints.font = theme.small
        right.font = theme.small
    }

    func update(mode: Mode, summary: String, inActivity: Bool = false) {
        switch mode {
        case .select where inActivity:
            modePill.stringValue = "SELECT"
            hints.stringValue = "1 all  2 downloads  3 screenshots  4 edited  5 airdrop  i inbox   tab reveal   m move   d trash   @ back"
        case .go:
            modePill.stringValue = "GO"
            hints.stringValue = "↑↓ ⌃j ⌃k move   ⏎ open   tab ⌃l go in   ⌫ ⌃h up   esc select"
        case .select:
            modePill.stringValue = "SELECT"
            hints.stringValue = "j k move   h l out / in   x collect   m move   c copy   d trash   r rename   e edit   u undo   / search"
        case .edit:
            modePill.stringValue = "EDIT"
            hints.stringValue = "rename: edit a name   trash: delete the line   move: type a path   copy: duplicate a line   new: add a line (end with / for a folder)"
        }
        self.mode = mode
        needsDisplay = true
        lastSummary = summary
        if flashWork == nil { right.stringValue = activity ?? lastSummary }
    }

    func setActivity(_ text: String?) {
        activity = text
        if flashWork == nil {
            right.stringValue = text ?? lastSummary
            right.textColor = text == nil ? Theme.shared.colors.textDim : Theme.shared.colors.text
        }
    }

    /// Shows a message on the right for a few seconds.
    func flash(_ message: String) {
        flashWork?.cancel()
        right.stringValue = message
        right.textColor = Theme.shared.colors.text
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.flashWork = nil
            self.right.stringValue = self.activity ?? self.lastSummary
            self.right.textColor = self.activity == nil ? Theme.shared.colors.textDim : Theme.shared.colors.text
        }
        flashWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: work)
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            let c = Theme.shared.colors
            layer?.backgroundColor = c.bar.cgColor
            let pill = switch mode {
            case .go: c.go
            case .select: c.select
            case .edit: c.match
            }
            pillBackground.layer?.backgroundColor = pill.cgColor
            modePill.textColor = c.badgeLabel
            hints.textColor = c.textDim
            if flashWork == nil && activity == nil { right.textColor = c.textDim }
        }
    }
}
