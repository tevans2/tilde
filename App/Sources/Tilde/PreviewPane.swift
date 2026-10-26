import AppKit
import QuickLookThumbnailing
import Quartz
import TildeFFI

/// The preview on the right: code and other text files rendered by bat (syntax
/// highlighted, with line numbers), everything else, including Markdown, through Quick Look.
///
/// Quick Look comes in two stages: a thumbnail straight away (with a spinner), then the
/// full, interactive preview once Quick Look has rendered the file at full size. Moving
/// on cancels both; the full stage only starts once the selection has rested, so scrolling
/// past files never starts heavy loads.
@MainActor
final class PreviewPane: NSView {
    private let quickLook = QLPreviewView(frame: .zero, style: .normal)!
    private let textScroll = NSScrollView()
    private let textView = NSTextView()
    /// The item's Finder tags, under the preview (hidden when it has none).
    private let tagLine = NSTextField(labelWithString: "")
    private var tagHeight: NSLayoutConstraint!
    /// Stage one of Quick Look, and the "full preview is coming" indicator.
    private let thumbnail = NSImageView()
    private let loading = NSStackView()
    private let spinner = NSProgressIndicator()
    private var thumbnailRequest: QLThumbnailGenerator.Request?
    private var fullWork: DispatchWorkItem?
    /// For the current item: the selection has rested, and the full-size thumbnail is in.
    private var rested = false
    /// The item showing, and the one whose full Quick Look preview is in place (faded in).
    private(set) var shownPath: String?
    private(set) var fullPath: String?
    /// `p` asked for the full preview of the item showing: a late bat render mustn't
    /// replace it.
    private var fullRequested = false
    private var fullSizeReady = false

    /// How long the selection must rest before the full preview starts loading.
    private static let restBeforeFull: TimeInterval = 0.25
    /// The full preview shows after this even if no full-size thumbnail arrives.
    private static let fullTimeout: TimeInterval = 1.5
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
        thumbnail.imageScaling = .scaleProportionallyUpOrDown
        thumbnail.isHidden = true
        // Fit the pane: a 2048 px thumbnail mustn't make the pane (and window) that big.
        for axis in [NSLayoutConstraint.Orientation.horizontal, .vertical] {
            thumbnail.setContentCompressionResistancePriority(.init(1), for: axis)
            thumbnail.setContentHuggingPriority(.init(1), for: axis)
        }
        spinner.style = .spinning
        spinner.controlSize = .small
        let loadingLabel = NSTextField(labelWithString: "Loading full preview…")
        loadingLabel.font = .systemFont(ofSize: 11)
        loadingLabel.textColor = .secondaryLabelColor
        loading.orientation = .horizontal
        loading.spacing = 6
        loading.addArrangedSubview(spinner)
        loading.addArrangedSubview(loadingLabel)
        loading.isHidden = true
        loading.translatesAutoresizingMaskIntoConstraints = false
        addSubview(loading)
        NSLayoutConstraint.activate([
            loading.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            loading.topAnchor.constraint(equalTo: topAnchor, constant: 8),
        ])
        for v in [quickLook, textScroll, thumbnail] as [NSView] {
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

    func show(_ row: RowView?) {
        generation += 1
        cancelQuickLook()
        shownPath = row?.path
        fullPath = nil
        fullRequested = false
        let current = generation
        showTags(row?.path)
        guard let row else { return showQuickLook(nil) }
        // Folders and packages (apps) only ever get their icon and details from Quick
        // Look, which is quick: no thumbnail stage or spinner.
        if row.kind == .folder || row.kind == .package {
            return showQuickLookNow(row.path)
        }
        let theme = Theme.shared
        guard theme.previewBat, Self.usesBat(row) else { return showQuickLook(row.path) }
        // bat themes are light or dark; match the window.
        let isDark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let batTheme = isDark ? theme.batThemeDark : theme.batThemeLight
        let (path, maxLines) = (row.path, UInt32(theme.batMaxLines))
        Task.detached {
            let preview = batPreview(path: path, theme: batTheme, maxLines: maxLines)
            await MainActor.run {
                guard current == self.generation, !self.fullRequested else { return }
                if let preview { self.showText(preview) } else { self.showQuickLook(path) }
            }
        }
    }

    func close() {
        cancelQuickLook()
        quickLook.close()
    }

    /// `p`: the full Quick Look preview of the item showing, now: without waiting for the
    /// selection to rest, and instead of bat for code and text. Returns false when it's
    /// already in place (`p` then opens the Quick Look window).
    func loadFull(for path: String?) -> Bool {
        guard let path, path == shownPath, fullPath != path else { return false }
        fullRequested = true
        // Already on its way in.
        if (quickLook.previewItem as? NSURL)?.path == path { return true }
        fullWork?.cancel()
        rested = true
        fullSizeReady = true
        if !textScroll.isHidden {
            textScroll.isHidden = true
            loading.isHidden = false
            spinner.startAnimation(nil)
        }
        showFullPreview(URL(fileURLWithPath: path), generation: generation)
        return true
    }

    /// Stops loading anything for the previous item.
    private func cancelQuickLook() {
        fullWork?.cancel()
        fullWork = nil
        if let request = thumbnailRequest { QLThumbnailGenerator.shared.cancel(request) }
        thumbnailRequest = nil
        spinner.stopAnimation(nil)
        loading.isHidden = true
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

    /// Straight to the full Quick Look view, for items where there's nothing to stage.
    private func showQuickLookNow(_ path: String) {
        textScroll.isHidden = true
        thumbnail.isHidden = true
        thumbnail.image = nil
        quickLook.alphaValue = 1
        quickLook.isHidden = false
        quickLook.previewItem = URL(fileURLWithPath: path) as NSURL
        fullPath = path
    }

    private func showQuickLook(_ path: String?) {
        textScroll.isHidden = true
        // The last item's full preview goes at once; it shouldn't keep loading or playing.
        quickLook.previewItem = nil
        quickLook.isHidden = true
        guard let path else {
            thumbnail.isHidden = true
            return
        }
        let current = generation
        let url = URL(fileURLWithPath: path)
        thumbnail.image = nil
        thumbnail.isHidden = false
        loading.isHidden = false
        spinner.startAnimation(nil)

        // Stage one: icon, then quick thumbnail, then a full-size one, as they arrive.
        let scale = window?.backingScaleFactor ?? 2
        let size = CGSize(width: max(bounds.width, 64), height: max(bounds.height, 64))
        let request = QLThumbnailGenerator.Request(
            fileAt: url, size: size, scale: scale, representationTypes: [.icon, .lowQualityThumbnail, .thumbnail])
        thumbnailRequest = request
        rested = false
        fullSizeReady = false
        QLThumbnailGenerator.shared.generateRepresentations(
            for: request,
            update: Self.onThumbnail { [weak self] image, final in
                guard let self, current == self.generation else { return }
                // Only real thumbnails: a file icon stretched to fill the pane would just
                // shrink back when the full preview (which shows it at icon size) arrives.
                if let image { self.thumbnail.image = image }
                if final {
                    self.fullSizeReady = true
                    self.startFull(url, generation: current)
                }
            })
        // Stage two waits for the selection to rest, and for the full-size thumbnail (or
        // a timeout, for files Quick Look has no thumbnail for).
        let rest = DispatchWorkItem { [weak self] in
            guard let self, current == self.generation else { return }
            self.rested = true
            self.startFull(url, generation: current)
        }
        fullWork = rest
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.restBeforeFull, execute: rest)
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.fullTimeout) { [weak self] in
            guard let self, current == self.generation else { return }
            self.fullSizeReady = true
            self.startFull(url, generation: current)
        }
    }

    /// The thumbnail callback, which Quick Look calls on its own queue: hands each image to
    /// `body` on the main thread (`final` once the full-size one, or nothing more, is in).
    nonisolated private static func onThumbnail(
        _ body: @escaping @MainActor (NSImage?, _ final: Bool) -> Void
    ) -> @Sendable (QLThumbnailRepresentation?, QLThumbnailRepresentation.RepresentationType, (any Error)?) -> Void {
        { rep, type, _ in
            // CGImage crosses threads; NSImage is made on the main thread. Icons aren't
            // passed on (see the caller).
            let image = type == .icon ? nil : rep?.cgImage
            let final = type == .thumbnail || rep == nil
            DispatchQueue.main.async {
                MainActor.assumeIsolated { body(image.map { NSImage(cgImage: $0, size: .zero) }, final) }
            }
        }
    }

    private func startFull(_ url: URL, generation current: Int) {
        guard current == generation, rested, fullSizeReady else { return }
        showFullPreview(url, generation: current)
    }

    /// Stage two: the interactive preview (scrolling PDFs, playing video), faded in over
    /// the thumbnail once it has had a moment to draw.
    private func showFullPreview(_ url: URL, generation current: Int) {
        guard quickLook.previewItem == nil || (quickLook.previewItem as? NSURL)?.path != url.path else { return }
        quickLook.previewItem = url as NSURL
        quickLook.alphaValue = 0
        quickLook.isHidden = false
        #if DEBUG
        // `TILDE_DEBUG_PREVIEW_FOCUS=1`: act like a preview that takes the keyboard.
        if ProcessInfo.processInfo.environment["TILDE_DEBUG_PREVIEW_FOCUS"] == "1" {
            window?.makeFirstResponder(quickLook)
        }
        #endif
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
            guard let self, current == self.generation else { return }
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.12
                self.quickLook.animator().alphaValue = 1
            } completionHandler: {
                MainActor.assumeIsolated {
                    guard current == self.generation else { return }
                    self.fullPath = url.path
                    self.thumbnail.isHidden = true
                    self.spinner.stopAnimation(nil)
                    self.loading.isHidden = true
                }
            }
        }
    }

    private func showText(_ preview: TextPreview) {
        cancelQuickLook()
        quickLook.previewItem = nil
        quickLook.isHidden = true
        thumbnail.isHidden = true
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
