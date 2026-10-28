import AppKit
import TildeFFI

// MARK: Path bar

/// Breadcrumbs for the current folder followed by the search field. It's also the
/// window's title bar: the window buttons sit at its leading edge (`leadingInset`).
/// For the mouse, each folder in the crumbs can be clicked to go there, and files can be
/// dropped on it.
@MainActor
final class PathBar: NSView {
    let field = FocusReportingField()
    private let crumbs = NSStackView()
    private var crumbsLeading: NSLayoutConstraint!
    /// A click on a folder in the crumbs.
    var onNavigate: ((String) -> Void)?
    /// What a drop of `info` on a folder would do, and doing it.
    var dropOperation: ((NSDraggingInfo, String) -> NSDragOperation)?
    var onDrop: ((NSDraggingInfo, String) -> Bool)?

    /// Room for the window buttons.
    var leadingInset: CGFloat = 0 {
        didSet { crumbsLeading.constant = 14 + leadingInset }
    }

    // Dragging the empty bar moves the window, as a title bar does.
    override var mouseDownCanMoveWindow: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        crumbs.orientation = .horizontal
        crumbs.spacing = 0
        crumbs.alignment = .centerY
        crumbs.detachesHiddenViews = true
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
        crumbsLeading = crumbs.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14)
        NSLayoutConstraint.activate([
            crumbsLeading,
            crumbs.centerYAnchor.constraint(equalTo: centerYAnchor),
            crumbs.widthAnchor.constraint(lessThanOrEqualTo: widthAnchor, multiplier: 0.6),
            field.leadingAnchor.constraint(equalTo: crumbs.trailingAnchor, constant: 4),
            field.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            field.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Replaces the crumbs: text pieces, each with the folder it goes to (or none).
    private func setCrumbs(_ pieces: [(text: String, path: String?, strong: Bool)], rootFirst: Bool = false) {
        for v in crumbs.arrangedSubviews { v.removeFromSuperview() }
        let font = Theme.shared.bar
        let c = Theme.shared.colors
        let sep: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: c.textDim.withAlphaComponent(0.55)]
        for (i, piece) in pieces.enumerated() {
            let crumb = Crumb(path: piece.path, bar: self)
            crumb.attributedTitle = NSAttributedString(string: piece.text, attributes: [
                .font: font, .foregroundColor: piece.strong ? c.text : c.textDim,
            ])
            // The root is its own separator: "/ Volumes / …".
            let slash = NSTextField(labelWithAttributedString: NSAttributedString(
                string: rootFirst && i == 0 ? " " : " / ", attributes: sep))
            for v in [crumb, slash] as [NSView] {
                v.setContentCompressionResistancePriority(.required, for: .horizontal)
                crumbs.addArrangedSubview(v)
                // Deep paths keep their end: the root-most pieces give way first.
                crumbs.setVisibilityPriority(NSStackView.VisibilityPriority(rawValue: Float(500 + i)), for: v)
            }
        }
    }

    func setFolder(_ path: String, home: String) {
        var pieces: [(String, String?, Bool)] = []
        var base: String
        var rest: Substring
        if path == home || path.hasPrefix(home + "/") {
            pieces.append(("~", home, false))
            base = home
            rest = path.dropFirst(home.count)
        } else {
            pieces.append(("/", "/", false))
            base = ""
            rest = Substring(path)
        }
        for part in rest.split(separator: "/") {
            base += "/" + part
            pieces.append((String(part), base, false))
        }
        setCrumbs(pieces.map { (text: $0.0, path: $0.1, strong: false) }, rootFirst: pieces.first?.0 == "/")
    }

    /// A page that isn't a folder: "History /".
    func setTitle(_ title: String) {
        setCrumbs([(text: title, path: nil, strong: true)])
    }

    /// `@ Activity`, then the filter (`/ Downloaded`, `/ Inbox`).
    func setActivity(_ filter: ActivityFilter) {
        var pieces: [(text: String, path: String?, strong: Bool)] = [(text: "@ Activity", path: nil, strong: true)]
        if filter.origin != .all { pieces.append((text: Format.filterName(filter.origin), path: nil, strong: false)) }
        if filter.inbox { pieces.append((text: "Inbox", path: nil, strong: false)) }
        setCrumbs(pieces)
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
        case .select: "press \(Keymap.shared.key(.search, in: .select)) to search"
        case .edit: "editing this folder as text"
        }
        field.textColor = mode == .go ? Theme.shared.colors.text : Theme.shared.colors.textDim
    }
}

/// One folder in the path bar: a click goes there; files dropped on it move (or copy) in.
@MainActor
private final class Crumb: NSButton {
    let path: String?
    private weak var bar: PathBar?
    private var targeted = false {
        didSet { needsDisplay = true }
    }

    init(path: String?, bar: PathBar) {
        self.path = path
        self.bar = bar
        super.init(frame: .zero)
        isBordered = false
        setButtonType(.momentaryChange)
        focusRingType = .none
        refusesFirstResponder = true
        if path != nil {
            target = self
            action = #selector(go)
            registerForDraggedTypes([.fileURL])
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func resetCursorRects() {
        if path != nil { addCursorRect(bounds, cursor: .pointingHand) }
    }

    @objc private func go() {
        if let path { bar?.onNavigate?(path) }
    }

    override func draw(_ dirtyRect: NSRect) {
        if targeted {
            Theme.shared.colors.selection.setFill()
            NSBezierPath(roundedRect: bounds.insetBy(dx: -3, dy: 0), xRadius: 5, yRadius: 5).fill()
        }
        super.draw(dirtyRect)
    }

    override func draggingEntered(_ sender: any NSDraggingInfo) -> NSDragOperation {
        let op = path.flatMap { bar?.dropOperation?(sender, $0) } ?? []
        targeted = !op.isEmpty
        return op
    }

    override func draggingUpdated(_ sender: any NSDraggingInfo) -> NSDragOperation {
        path.flatMap { bar?.dropOperation?(sender, $0) } ?? []
    }

    override func draggingExited(_ sender: (any NSDraggingInfo)?) {
        targeted = false
    }

    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        targeted = false
        guard let path else { return false }
        return bar?.onDrop?(sender, path) ?? false
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
///
/// For the mouse (none of it shows while using keys): a `⋯` button on the row under the
/// pointer opens that row's menu; ⌘-click and ⇧-click put rows on the shelf.
final class ResultsTableView: NSTableView {
    var onFocus: (() -> Void)?
    var onKey: ((NSEvent) -> Bool)?
    /// Whether a row can have the `⋯` button (not section headers).
    var canHover: ((Int) -> Bool)?
    /// The `⋯` button on a row was clicked; the menu shows under `button`.
    var onMoreClick: ((Int, NSView) -> Void)?
    /// ⌘-click (`extend` false) or ⇧-click (`extend` true) on a row.
    var onShelfClick: ((Int, _ extend: Bool) -> Void)?
    /// A drag left the list.
    var onDragExit: (() -> Void)?
    private let moreButton = NSButton()
    private var hoverRow = -1
    private var tracking: NSTrackingArea?

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
        moreButton.image = NSImage(systemSymbolName: "ellipsis.circle", accessibilityDescription: "Actions")
        moreButton.isBordered = false
        moreButton.imageScaling = .scaleProportionallyDown
        moreButton.refusesFirstResponder = true
        moreButton.target = self
        moreButton.action = #selector(moreClicked)
        moreButton.isHidden = true
        moreButton.toolTip = "Actions (or right-click the row)"
        addSubview(moreButton)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                  owner: self, userInfo: nil)
        addTrackingArea(area)
        tracking = area
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        showMoreButton(row: row(at: convert(event.locationInWindow, from: nil)))
    }

    /// The `⋯` button on `row`, as the pointer moving over it shows.
    func showMoreButton(row: Int) {
        guard row >= 0, canHover?(row) ?? true else { return hideMoreButton() }
        guard row != hoverRow || moreButton.isHidden else { return }
        hoverRow = row
        let rect = rect(ofRow: row)
        let size = min(rect.height - 6, 20)
        moreButton.contentTintColor = Theme.shared.colors.textDim
        moreButton.frame = NSRect(x: rect.maxX - size - 14, y: rect.midY - size / 2, width: size, height: size)
        // Above the row views, which the table adds after it.
        addSubview(moreButton, positioned: .above, relativeTo: nil)
        moreButton.isHidden = false
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        hideMoreButton()
    }

    override func scrollWheel(with event: NSEvent) {
        hideMoreButton()
        super.scrollWheel(with: event)
    }

    /// The `⋯` button goes as soon as the keyboard is used.
    func hideMoreButton() {
        moreButton.isHidden = true
        hoverRow = -1
    }

    @objc private func moreClicked() {
        guard hoverRow >= 0 else { return }
        onMoreClick?(hoverRow, moreButton)
    }

    override func mouseDown(with event: NSEvent) {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let row = row(at: convert(event.locationInWindow, from: nil))
        if row >= 0, flags.contains(.command) || flags.contains(.shift), let onShelfClick {
            onShelfClick(row, flags.contains(.shift))
            return
        }
        super.mouseDown(with: event)
    }

    override func draggingExited(_ sender: (any NSDraggingInfo)?) {
        super.draggingExited(sender)
        onDragExit?()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok { onFocus?() }
        return ok
    }

    /// The paths to drag when a drag starts on the row showing `path`: the whole shelf if
    /// the row is on it. `nil` drags just the row.
    var dragGroup: ((String) -> [String]?)?

    override func beginDraggingSession(
        with items: [NSDraggingItem], event: NSEvent, source: NSDraggingSource
    ) -> NSDraggingSession {
        guard items.count == 1, let url = items[0].item as? NSURL, let path = url.path,
              let group = dragGroup?(path), group.count > 1
        else { return super.beginDraggingSession(with: items, event: event, source: source) }
        // A pile of file icons under the pointer, one per shelved item.
        let at = convert(event.locationInWindow, from: nil)
        let pile = group.enumerated().map { i, p in
            let item = NSDraggingItem(pasteboardWriter: URL(fileURLWithPath: p) as NSURL)
            let offset = CGFloat(min(i, 4)) * 4
            item.setDraggingFrame(NSRect(x: at.x - 16 + offset, y: at.y - 16 + offset, width: 32, height: 32),
                                  contents: NSWorkspace.shared.icon(forFile: p))
            return item
        }
        let session = super.beginDraggingSession(with: pile, event: event, source: source)
        session.draggingFormation = .pile
        return session
    }

    override func keyDown(with event: NSEvent) {
        hideMoreButton()
        if onKey?(event) != true { super.keyDown(with: event) }
    }
}

/// Which part of Clean Up shows, for the status bar's hints.
enum CleanupPage {
    case categories, items, trash
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

    // Drawn directly rather than built from text fields: Auto Layout measuring a handful
    // of fields per row cost ~0.5 ms a row, enough to drop frames when a fast scroll
    // brings in a screenful at once.
    private var row: RowView?
    private var onShelf = false
    // Built in configure and when the selection changes; draw only lays them out.
    private var name = NSAttributedString()
    private var nameWidth: CGFloat = 0
    private var location = NSAttributedString()
    private var origin = NSAttributedString()
    private var badgeText = NSAttributedString()
    private var badgeColor = NSColor.systemGray
    private var meta = NSAttributedString()
    private var metaWidth: CGFloat = 0
    private var age = NSAttributedString()
    private var ageWidth: CGFloat = 0

    private static let shelfWidth: CGFloat = 12
    private static let badgeWidth: CGFloat = 40

    init() {
        super.init(frame: .zero)
        identifier = Self.identifier
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    func configure(with row: RowView, onShelf: Bool = false) {
        self.row = row
        self.onShelf = onShelf
        setAccessibilityLabel(row.name)
        render()
    }

    override var backgroundStyle: NSView.BackgroundStyle {
        didSet { if oldValue != backgroundStyle { render() } }
    }

    private func render() {
        guard let row else { return }
        let theme = Theme.shared
        let c = theme.colors
        let selected = backgroundStyle == .emphasized

        let s = NSMutableAttributedString(
            string: row.name + (row.kind == .folder ? "/" : ""),
            attributes: [.font: theme.list, .foregroundColor: selected ? c.selectionText : c.text])
        let hit: [NSAttributedString.Key: Any] = [
            .font: theme.listBold, .foregroundColor: selected ? c.matchSelected : c.match,
        ]
        let length = (row.name as NSString).length
        for r in row.highlights where Int(r.start + r.length) <= length {
            s.addAttributes(hit, range: NSRange(location: Int(r.start), length: Int(r.length)))
        }
        name = s
        nameWidth = s.size().width.rounded(.up)

        if row.location.isEmpty {
            location = NSAttributedString()
        } else {
            // Everywhere and Activity rows show where the item lives, dimmed, with matched
            // characters.
            let dim: [NSAttributedString.Key: Any] = [
                .font: theme.small,
                .foregroundColor: selected ? c.selectionText.withAlphaComponent(0.7) : c.textDim,
            ]
            let l = NSMutableAttributedString(string: row.location, attributes: dim)
            let locationHit: [NSAttributedString.Key: Any] = [
                .font: theme.small.bold, .foregroundColor: selected ? c.matchSelected : c.match,
            ]
            let locationLength = (row.location as NSString).length
            for r in row.locationHighlights where Int(r.start + r.length) <= locationLength {
                l.addAttributes(locationHit, range: NSRange(location: Int(r.start), length: Int(r.length)))
            }
            if !row.source.isEmpty {
                l.append(NSAttributedString(string: " · \(row.source)", attributes: dim))
            }
            if row.filed {
                var filed = dim
                filed[.foregroundColor] = selected ? c.selectionText : c.select
                l.append(NSAttributedString(string: " · filed", attributes: filed))
            }
            location = l
        }

        origin = row.origin.map {
            NSAttributedString(string: Format.originName($0), attributes: [
                .font: theme.small, .foregroundColor: selected ? c.selectionText : Format.originColor($0),
            ])
        } ?? NSAttributedString()
        badgeText = NSAttributedString(string: Format.badgeText(row), attributes: [
            .font: theme.badge, .foregroundColor: c.badgeLabel,
        ])
        badgeColor = Format.badgeColor(row.category)
        let dimText: [NSAttributedString.Key: Any] = [
            .font: theme.small, .foregroundColor: selected ? c.selectionText : c.textDim,
        ]
        meta = NSAttributedString(string: Format.meta(row), attributes: dimText)
        metaWidth = meta.size().width.rounded(.up)
        age = NSAttributedString(string: Format.age(row.modified), attributes: dimText)
        ageWidth = max(age.size().width.rounded(.up), 34)
        needsDisplay = true
    }

    /// Wide enough for the longest origin label, so names line up in Activity.
    private static var originWidth: (font: NSFont, width: CGFloat)?
    private func originColumnWidth(_ font: NSFont) -> CGFloat {
        if let cached = Self.originWidth, cached.font == font { return cached.width }
        let width = ("Screenshot " as NSString).size(withAttributes: [.font: font]).width.rounded(.up)
        Self.originWidth = (font, width)
        return width
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let row else { return }
        let theme = Theme.shared
        let h = bounds.height
        // Every text shares the name's baseline, centred in the row.
        let font = theme.list
        let baseline = ((h - (font.ascender - font.descender)) / 2 + font.ascender).rounded()
        func draw(_ text: NSAttributedString, x: CGFloat, width: CGFloat, truncateHead: Bool = false) {
            guard width > 4, text.length > 0 else { return }
            let f = text.attribute(.font, at: 0, effectiveRange: nil) as? NSFont ?? font
            let style = NSMutableParagraphStyle()
            style.lineBreakMode = truncateHead ? .byTruncatingHead : .byTruncatingTail
            let t = NSMutableAttributedString(attributedString: text)
            t.addAttribute(.paragraphStyle, value: style, range: NSRange(location: 0, length: t.length))
            let rect = NSRect(x: x, y: baseline - f.ascender, width: width, height: f.ascender - f.descender + 2)
            t.draw(with: rect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        }

        var x: CGFloat = 0
        if onShelf {
            let dot = NSRect(x: (Self.shelfWidth - 6) / 2, y: (h - 6) / 2, width: 6, height: 6)
            theme.colors.shelf.setFill()
            NSBezierPath(ovalIn: dot).fill()
        }
        x += Self.shelfWidth + 2
        if row.origin != nil {
            let w = originColumnWidth(theme.small)
            draw(origin, x: x, width: w)
            x += w
        }
        x += 2

        let badgeHeight = (h * 0.62).rounded()
        let badge = NSRect(x: x, y: ((h - badgeHeight) / 2).rounded(), width: Self.badgeWidth, height: badgeHeight)
        badgeColor.setFill()
        NSBezierPath(roundedRect: badge, xRadius: 4, yRadius: 4).fill()
        let badgeFont = theme.badge
        let badgeTextWidth = min(badgeText.size().width.rounded(.up), Self.badgeWidth - 2)
        let badgeBaseline = badge.midY + (badgeFont.capHeight / 2).rounded()
        badgeText.draw(with: NSRect(x: badge.midX - badgeTextWidth / 2, y: badgeBaseline - badgeFont.ascender,
                                    width: badgeTextWidth, height: badgeFont.ascender - badgeFont.descender + 2),
                       options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
        x = badge.maxX + 10

        // Right side: age against the edge, size or count before it.
        let ageX = bounds.width - 8 - ageWidth
        draw(age, x: ageX, width: ageWidth)
        let metaX = ageX - 12 - metaWidth
        draw(meta, x: metaX, width: metaWidth)

        // The location gives way first, then the name truncates.
        let available = metaX - 12 - x
        let nameW = min(nameWidth, max(available, 0))
        draw(name, x: x, width: nameW + 1)
        let locationX = x + nameW + 10
        // Paths keep their end, which tells folders apart; a matching line its start.
        draw(location, x: locationX, width: available - nameW - 10, truncateHead: row.section != .contents)
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
            string: "    " + [(KeyAction.move, "move"), (.copy, "copy"), (.yank, "yank"), (.trash, "trash"), (.tag, "tag"),
                                 (.compress, "zip"), (.clear, "clear")]
                .compactMap { action, label in
                    let key = Keymap.shared.key(action, in: .select)
                    return key.isEmpty ? nil : "\(key) \(label)"
                }
                .joined(separator: "  "),
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
    /// A click on the mode and hints: the keys panel.
    var onHelp: (() -> Void)?
    /// A click on the summary on the right, with the point to show a menu at.
    var onSummaryClick: ((NSPoint) -> Void)?
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

    // The keys panel and the summary's menu are for the mouse: nothing changes for keys.
    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if right.frame.insetBy(dx: -6, dy: -6).contains(point) {
            onSummaryClick?(NSPoint(x: right.frame.minX, y: right.frame.maxY + 4))
        } else if point.x < max(hints.frame.maxX, pillBackground.frame.maxX) + 8, mode != .edit {
            onHelp?()
        }
    }

    override func resetCursorRects() {
        for view in [pillBackground, hints, right] as [NSView] {
            addCursorRect(view.frame, cursor: .pointingHand)
        }
    }

    func applyTheme(_ theme: Theme) {
        modePill.font = theme.badge
        hints.font = theme.small
        right.font = theme.small
    }

    func update(mode: Mode, summary: String, inActivity: Bool = false, inHistory: Bool = false, inPreview: Bool = false,
                inCleanup: CleanupPage? = nil) {
        let keys = Keymap.shared
        /// "m move   c copy" from the keymap, leaving out unbound actions.
        func hint(_ items: [(KeyAction, String)], all: Bool = false, gap: String = "   ") -> String {
            items.compactMap { action, label in
                let key = all ? keys.keys(action, in: mode) : keys.key(action, in: mode)
                return key.isEmpty ? nil : "\(key) \(label)"
            }.joined(separator: gap)
        }
        let move = "\(keys.key(.down, in: .select)) \(keys.key(.up, in: .select))"
        // The essentials; `?` (or a click here) lists every key.
        let more = keys.key(.help, in: mode == .go ? .go : .select)
        let help = more.isEmpty ? "" : "   \(more) keys"
        switch mode {
        case .select where inPreview:
            modePill.stringValue = "PREVIEW"
            hints.stringValue = "\(move) scroll   " + hint([(.previewSource, "source"), (.previewFull, "whole window"), (.quickLook, "back")]) + help
        case .select where inCleanup != nil:
            modePill.stringValue = "CLEAN UP"
            switch inCleanup! {
            case .categories:
                hints.stringValue = "\(move) move   1–6 category   " + hint([(.enter, "open"), (.cleanup, "close")]) + help
            case .items, .trash:
                hints.stringValue = "\(move) move   " + hint([(.shelf, "collect"), (.trash, "trash"), (.parent, "back")]) + help
            }
        case .select where inHistory:
            modePill.stringValue = "HISTORY"
            hints.stringValue = "\(move) move   " + hint([(.undo, "undo"), (.redo, "redo")]) + "   ⏎ go to that state   esc back" + help
        case .select where inActivity:
            modePill.stringValue = "SELECT"
            hints.stringValue = "\(move) move   " + hint([(.filterDownloads, "downloads"), (.inbox, "inbox"), (.enter, "reveal"), (.activity, "back")]) + help
        case .go:
            modePill.stringValue = "GO"
            let goMove = [keys.key(.down, in: .go), keys.key(.up, in: .go)].filter { !$0.isEmpty }.joined(separator: " ")
            hints.stringValue = (goMove.isEmpty ? "" : "\(goMove) move   ") + hint([(.open, "open"), (.enter, "go in"), (.selectMode, "select")]) + help
        case .select:
            modePill.stringValue = "SELECT"
            let inOut = "\(keys.key(.parent, in: .select)) \(keys.key(.enter, in: .select))"
            hints.stringValue = "\(move) move   \(inOut) out / in   " + hint([
                (.shelf, "collect"), (.move, "move"), (.trash, "trash"), (.quickLook, "preview"), (.goto, "go to"), (.search, "search"),
            ]) + help
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
