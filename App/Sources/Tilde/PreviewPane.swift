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

        for v in [quickLook, textScroll] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
            NSLayoutConstraint.activate([
                v.topAnchor.constraint(equalTo: topAnchor),
                v.bottomAnchor.constraint(equalTo: bottomAnchor),
                v.leadingAnchor.constraint(equalTo: leadingAnchor),
                v.trailingAnchor.constraint(equalTo: trailingAnchor),
            ])
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show(_ row: RowView?) {
        generation += 1
        let current = generation
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
        textScroll.isHidden = true
        quickLook.isHidden = false
        quickLook.previewItem = path.map { URL(fileURLWithPath: $0) as NSURL }
    }

    private func showText(_ preview: TextPreview) {
        quickLook.previewItem = nil
        quickLook.isHidden = true
        textScroll.isHidden = false
        textView.textStorage?.setAttributedString(Self.attributed(preview))
        textView.scroll(.zero)
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
