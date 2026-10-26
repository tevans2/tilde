import AppKit
import Quartz
import TildeFFI

/// The preview on the right: code and other text files rendered by bat (syntax
/// highlighted, with line numbers), everything else, including Markdown, through Quick Look.
@MainActor
final class PreviewPane: NSView {
    private let quickLook = QLPreviewView(frame: .zero, style: .normal)!
    private let textScroll = NSScrollView()
    private let textView = NSTextView()
    /// The item's Finder tags, under the preview (hidden when it has none).
    private let tagLine = NSTextField(labelWithString: "")
    private var tagHeight: NSLayoutConstraint!
    /// Bumped on every selection change, so a slow bat render for an earlier selection is
    /// ignored.
    private var generation = 0

    private static let markdown: Set<String> = ["md", "markdown", "mdown", "mkd", "mdx"]
    private static let batSizeLimit: UInt64 = 5_000_000

    override init(frame: NSRect) {
        super.init(frame: frame)
        quickLook.autostarts = true
        quickLook.shouldCloseWithWindow = true

        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = true
        textView.drawsBackground = false
        textView.textContainerInset = NSSize(width: 12, height: 10)
        // Code shouldn't wrap; scroll sideways instead.
        textView.isHorizontallyResizable = true
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.textContainer?.widthTracksTextView = false
        textView.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textScroll.documentView = textView
        textScroll.hasVerticalScroller = true
        textScroll.hasHorizontalScroller = true
        textScroll.drawsBackground = false
        textScroll.isHidden = true

        tagLine.translatesAutoresizingMaskIntoConstraints = false
        tagLine.lineBreakMode = .byTruncatingTail
        addSubview(tagLine)
        tagHeight = tagLine.heightAnchor.constraint(equalToConstant: 0)
        NSLayoutConstraint.activate([
            tagLine.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            tagLine.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            tagLine.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -6),
            tagHeight,
        ])
        for v in [quickLook, textScroll] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
            NSLayoutConstraint.activate([
                v.topAnchor.constraint(equalTo: topAnchor),
                v.bottomAnchor.constraint(equalTo: tagLine.topAnchor, constant: -4),
                v.leadingAnchor.constraint(equalTo: leadingAnchor),
                v.trailingAnchor.constraint(equalTo: trailingAnchor),
            ])
        }
    }

    /// Finder's colour tags, by name.
    private static let tagColors: [String: NSColor] = [
        "red": .systemRed, "orange": .systemOrange, "yellow": .systemYellow, "green": .systemGreen,
        "blue": .systemBlue, "purple": .systemPurple, "gray": .systemGray, "grey": .systemGray,
    ]

    private func showTags(_ path: String?) {
        let tags = path.map { commonTags(paths: [$0]) } ?? []
        tagHeight.constant = tags.isEmpty ? 0 : 18
        guard !tags.isEmpty else { return tagLine.stringValue = "" }
        let theme = Theme.shared
        let s = NSMutableAttributedString()
        for (i, tag) in tags.enumerated() {
            if i > 0 { s.append(NSAttributedString(string: "   ")) }
            let dot = Self.tagColors[tag.lowercased()] ?? theme.colors.textDim
            s.append(NSAttributedString(string: "● ", attributes: [.font: theme.small, .foregroundColor: dot]))
            s.append(NSAttributedString(string: tag, attributes: [.font: theme.small, .foregroundColor: theme.colors.text]))
        }
        tagLine.attributedStringValue = s
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    // Some Quick Look providers don't accept keyboard focus themselves. The pane is
    // a fallback so focus can still leave the search field and Ctrl-H can return to it.
    override var acceptsFirstResponder: Bool { true }

    func focus() {
        guard let window else { return }
        let target: NSView = textScroll.isHidden ? quickLook : textView
        if !window.makeFirstResponder(target) { window.makeFirstResponder(self) }
    }

    private var containsFocus: Bool {
        guard let responder = window?.firstResponder as? NSView else { return false }
        return responder === self || responder.isDescendant(of: self)
    }

    func show(_ row: RowView?) {
        generation += 1
        let current = generation
        showTags(row?.path)
        guard let row else { return showQuickLook(nil) }
        let theme = Theme.shared
        guard theme.previewBat, Self.usesBat(row) else { return showQuickLook(row.path) }
        // bat themes are light or dark; match the window.
        let isDark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let batTheme = isDark ? theme.batThemeDark : theme.batThemeLight
        let (path, maxLines) = (row.path, UInt32(theme.batMaxLines))
        Task.detached {
            let preview = batPreview(path: path, theme: batTheme, maxLines: maxLines)
            await MainActor.run {
                guard current == self.generation else { return }
                if let preview { self.showText(preview) } else { self.showQuickLook(path) }
            }
        }
    }

    func close() {
        quickLook.close()
    }

    /// Code, plain text and unknown types (bat checks they're really text). Markdown goes
    /// to Quick Look.
    private static func usesBat(_ row: RowView) -> Bool {
        guard row.kind == .file, row.size <= batSizeLimit, !markdown.contains(row.ext) else { return false }
        switch row.category {
        case .code, .text, .other: return true
        default: return false
        }
    }

    private func showQuickLook(_ path: String?) {
        let restoreFocus = containsFocus
        textScroll.isHidden = true
        quickLook.isHidden = false
        quickLook.previewItem = path.map { URL(fileURLWithPath: $0) as NSURL }
        if restoreFocus { focus() }
    }

    private func showText(_ preview: TextPreview) {
        let restoreFocus = containsFocus
        quickLook.previewItem = nil
        quickLook.isHidden = true
        textScroll.isHidden = false
        textView.textStorage?.setAttributedString(Self.attributed(preview))
        textView.scroll(.zero)
        if restoreFocus { focus() }
    }

    private static func attributed(_ preview: TextPreview) -> NSAttributedString {
        let theme = Theme.shared
        let font = theme.small
        let s = NSMutableAttributedString(
            string: preview.text, attributes: [.font: font, .foregroundColor: theme.colors.text])
        let length = (preview.text as NSString).length
        let bold = font.bold
        let italic = NSFontManager.shared.convert(font, toHaveTrait: .italicFontMask)
        for span in preview.spans where Int(span.start + span.length) <= length {
            let range = NSRange(location: Int(span.start), length: Int(span.length))
            var attrs: [NSAttributedString.Key: Any] = [:]
            if let fg = span.fg {
                attrs[.foregroundColor] = NSColor(rgba: fg).withAlphaComponent(span.dim ? 0.6 : 1)
            }
            if let bg = span.bg { attrs[.backgroundColor] = NSColor(rgba: bg) }
            if span.bold { attrs[.font] = bold }
            if span.italic { attrs[.font] = italic }
            if span.underline { attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue }
            s.addAttributes(attrs, range: range)
        }
        return s
    }
}

extension NSColor {
    /// From `0xRRGGBBAA`.
    convenience init(rgba: UInt32) {
        self.init(
            srgbRed: CGFloat((rgba >> 24) & 0xff) / 255,
            green: CGFloat((rgba >> 16) & 0xff) / 255,
            blue: CGFloat((rgba >> 8) & 0xff) / 255,
            alpha: CGFloat(rgba & 0xff) / 255)
    }
}
