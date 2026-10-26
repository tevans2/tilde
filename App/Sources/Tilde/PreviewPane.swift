import AppKit
import PDFKit
import QuickLookThumbnailing
import Quartz
import TildeFFI

/// The preview on the right: code and other text files rendered by bat (syntax
/// highlighted, with line numbers), everything else, including Markdown, through Quick Look.
///
/// Quick Look shows an instant preview: a thumbnail rendered for the pane (images, the
/// first page of PDFs and documents, a frame of video), or the icon and details when there
/// isn't one. The full, interactive preview only appears on request (`p`). To make that
/// quick, it's loaded invisibly in the background once the selection rests, for files
/// where that's cheap and has no side effects: local, not in the cloud only, not video or
/// audio, up to 50 MB. Moving on stops it.
@MainActor
final class PreviewPane: NSView {
    private let quickLook = QLPreviewView(frame: .zero, style: .normal)!
    /// PDFs' full preview, drawn here rather than by Quick Look (which runs in another
    /// process and only moves a page at a time), so j / k can scroll smoothly.
    private let pdfView = PDFView()
    /// The full preview of the item showing is the PDF view.
    private var usingPDF = false
    /// Just shown: keep the PDF at the top while it settles (it rescales when the pane
    /// widens for PREVIEW, which would leave it partway down). Ends on the first scroll.
    private var pinPDFToTop = false
    /// How far down the PDF the top of the view was (0–1) before the pane changed width.
    /// It fits the width, so it scales evenly: the same fraction shows the same spot.
    private var pdfSpot: CGFloat?
    /// Whichever view shows the full preview.
    private var fullView: NSView { usingPDF ? pdfView : quickLook }
    private let textScroll = NSScrollView()
    private let textView = NSTextView()
    /// The item's Finder tags, under the preview (hidden when it has none).
    private let tagLine = NSTextField(labelWithString: "")
    private var tagHeight: NSLayoutConstraint!
    /// Stage one of Quick Look, and the "full preview is coming" indicator.
    private let thumbnail = NSImageView()
    private let loading = NSStackView()
    private let spinner = NSProgressIndicator()
    /// Under the icon when there's no thumbnail: name, kind, size, date.
    private let info = NSTextField(labelWithString: "")
    /// "p full preview", in the thumbnail stage.
    private let hint = NSTextField(labelWithString: "")
    private var thumbnailRequest: QLThumbnailGenerator.Request?
    private var fullWork: DispatchWorkItem?
    private var resizeWork: DispatchWorkItem?
    /// The full preview of the item showing: loading in the background, loaded (hidden),
    /// or showing.
    private enum Full { case none, loading, ready, shown }
    private var full = Full.none
    /// `p` was pressed: show the full preview as soon as it's loaded.
    private var revealWhenReady = false
    /// The thumbnail stage got a real thumbnail (not just the icon).
    private var gotThumbnail = false
    private var row: RowView?
    /// The item showing, and the one whose full Quick Look preview is in place (faded in).
    private(set) var shownPath: String?
    /// The showing item's size and modification time, to tell a real change from a refresh
    /// of the same row.
    private var shownStamp: (UInt64, Int64)?
    private(set) var fullPath: String?
    /// `p` asked for the full preview of the item showing: a late bat render mustn't
    /// replace it.
    private var fullRequested = false

    /// How long the selection must rest before the full preview loads in the background.
    private static let restBeforePreload: TimeInterval = 0.3
    /// Larger files aren't preloaded.
    private static let preloadLimit: UInt64 = 50_000_000
    /// After Quick Look attaches its content view, time for its first frame to draw.
    private static let settle: TimeInterval = 0.12
    /// The full preview fades in after this even if no content view shows up.
    private static let fullTimeout: TimeInterval = 1.5
    private static let crossfade: TimeInterval = 0.2
    /// Bumped on every selection change, so a slow bat render for an earlier selection is
    /// ignored.
    private var generation = 0

    private static let markdown: Set<String> = ["md", "markdown", "mdown", "mkd", "mdx"]
    private static let batSizeLimit: UInt64 = 5_000_000

    override init(frame: NSRect) {
        super.init(frame: frame)
        quickLook.autostarts = true
        quickLook.shouldCloseWithWindow = true
        pdfView.autoScales = true
        pdfView.displayMode = .singlePageContinuous
        pdfView.displaysPageBreaks = true
        pdfView.isHidden = true
        NotificationCenter.default.addObserver(
            forName: .PDFViewScaleChanged, object: pdfView, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                // After PDFKit's own layout for the new scale.
                DispatchQueue.main.async {
                    if let spot = self.pdfSpot {
                        self.restorePDFSpot(spot)
                    } else if self.pinPDFToTop, let scroll = self.pdfView.documentView?.enclosingScrollView {
                        Self.scrollToTop(scroll)
                    }
                }
            }
        }

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
        info.alignment = .center
        info.maximumNumberOfLines = 4
        info.isHidden = true
        hint.isHidden = true
        for v in [info, hint] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
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
        // Back to front: the thumbnail, the full preview fading in over it, text; then
        // the loading note on top.
        for v in [thumbnail, quickLook, pdfView, textScroll] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
            NSLayoutConstraint.activate([
                v.topAnchor.constraint(equalTo: topAnchor),
                v.bottomAnchor.constraint(equalTo: tagLine.topAnchor, constant: -4),
                v.leadingAnchor.constraint(equalTo: leadingAnchor),
                v.trailingAnchor.constraint(equalTo: trailingAnchor),
            ])
        }
        addSubview(loading, positioned: .above, relativeTo: nil)
        addSubview(hint, positioned: .above, relativeTo: nil)
        addSubview(info, positioned: .above, relativeTo: nil)
        NSLayoutConstraint.activate([
            hint.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            hint.bottomAnchor.constraint(equalTo: tagLine.topAnchor, constant: -6),
            info.centerXAnchor.constraint(equalTo: centerXAnchor),
            info.topAnchor.constraint(equalTo: centerYAnchor, constant: 80),
            info.widthAnchor.constraint(lessThanOrEqualTo: widthAnchor, constant: -40),
        ])
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
        // The list re-selects the same row whenever its folder is re-listed (something else
        // in it changed). Starting over would show "Loading" again for nothing.
        if let row, row.path == shownPath, shownStamp.map({ $0 == (row.size, row.modified) }) == true {
            return
        }
        generation += 1
        cancelQuickLook()
        self.row = row
        shownPath = row?.path
        shownStamp = row.map { ($0.size, $0.modified) }
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

    #if DEBUG
    /// For the snapshot hook: "loading", "ready", "shown", plus what's visible.
    var debugState: String {
        "full=\(full) thumbnail=\(!thumbnail.isHidden && thumbnail.image != nil) info=\(!info.isHidden) hint=\(!hint.isHidden) spinner=\(!loading.isHidden)"
    }
    #endif

    /// The pane is about to change width (PREVIEW, `f`): keep the PDF on the same spot
    /// rather than wherever rescaling leaves it.
    func holdPosition() {
        guard usingPDF, full == .shown, !pinPDFToTop,
              let scroll = pdfView.documentView?.enclosingScrollView, let document = scroll.documentView,
              document.frame.height > 0
        else { return }
        let clip = scroll.contentView.bounds
        let fromTop = document.isFlipped ? clip.minY : document.frame.height - clip.maxY
        let spot = fromTop / document.frame.height
        pdfSpot = spot
        // Also after this layout pass, in case the scale doesn't change (same width).
        DispatchQueue.main.async { [weak self] in
            guard let self, self.pdfSpot != nil else { return }
            self.window?.layoutIfNeeded()
            self.restorePDFSpot(spot)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.pdfSpot = nil }
    }

    private func restorePDFSpot(_ spot: CGFloat) {
        guard let scroll = pdfView.documentView?.enclosingScrollView, let document = scroll.documentView else { return }
        let clip = scroll.contentView
        let height = document.frame.height
        let fromTop = spot * height
        let maxY = max(height - clip.bounds.height, 0)
        let y = document.isFlipped ? fromTop : height - fromTop - clip.bounds.height
        clip.setBoundsOrigin(NSPoint(x: clip.bounds.origin.x, y: min(max(y, 0), maxY)))
        scroll.reflectScrolledClipView(clip)
    }

    /// Called when the full preview has appeared (after `p`).
    var onFullShown: (() -> Void)?

    /// What takes the keyboard when the preview is focused: the full preview, or bat's
    /// text. `nil` while the full preview is still loading.
    var focusTarget: NSView? {
        if full == .shown { return usingPDF ? pdfView : quickLook }
        if !textScroll.isHidden { return textView }
        return nil
    }

    enum Scroll { case down, up, pageDown, pageUp, top, bottom }

    /// Scrolls the focused preview. Quick Look draws in another process, so it gets the
    /// arrow and page keys it understands; bat's text is scrolled directly.
    func scroll(_ how: Scroll) {
        pinPDFToTop = false
        pdfSpot = nil
        if full == .shown, usingPDF, let scroll = pdfView.documentView?.enclosingScrollView {
            return Self.scroll(scroll, how)
        }
        if full == .shown, let window {
            let (code, char): (UInt16, Int) = switch how {
            case .down: (125, NSDownArrowFunctionKey)
            case .up: (126, NSUpArrowFunctionKey)
            case .pageDown: (121, NSPageDownFunctionKey)
            case .pageUp: (116, NSPageUpFunctionKey)
            case .top: (115, NSHomeFunctionKey)
            case .bottom: (119, NSEndFunctionKey)
            }
            let chars = String(UnicodeScalar(char)!)
            for type in [NSEvent.EventType.keyDown, .keyUp] {
                if let e = NSEvent.keyEvent(
                    with: type, location: .zero, modifierFlags: [.function, .numericPad], timestamp: ProcessInfo.processInfo.systemUptime,
                    windowNumber: window.windowNumber, context: nil, characters: chars, charactersIgnoringModifiers: chars,
                    isARepeat: false, keyCode: code)
                {
                    // Straight to the window (to the focused preview), not back through the
                    // app's key handling.
                    window.sendEvent(e)
                }
            }
            return
        }
        guard !textScroll.isHidden else { return }
        Self.scroll(textScroll, how)
    }

    private static func scrollToTop(_ scroll: NSScrollView) {
        guard let document = scroll.documentView else { return }
        let clip = scroll.contentView
        let maxY = max(document.frame.height - clip.bounds.height, 0)
        clip.setBoundsOrigin(NSPoint(x: clip.bounds.origin.x, y: document.isFlipped ? 0 : maxY))
        scroll.reflectScrolledClipView(clip)
    }

    /// Scrolls a scroll view by a few lines, a screen, or to an end, with a short glide.
    private static func scroll(_ scroll: NSScrollView, _ how: Scroll) {
        guard let document = scroll.documentView else { return }
        let clip = scroll.contentView
        let line: CGFloat = 56
        let page = max(clip.bounds.height - line, line)
        let maxY = max(document.frame.height - clip.bounds.height, 0)
        // In an unflipped document the top is at the far end.
        let sign: CGFloat = document.isFlipped ? 1 : -1
        let top: CGFloat = document.isFlipped ? 0 : maxY
        var y = clip.bounds.origin.y
        switch how {
        case .down: y += line * sign
        case .up: y -= line * sign
        case .pageDown: y += page * sign
        case .pageUp: y -= page * sign
        case .top: y = top
        case .bottom: y = maxY - top
        }
        let target = NSPoint(x: clip.bounds.origin.x, y: min(max(y, 0), maxY))
        NSAnimationContext.runAnimationGroup { context in
            context.duration = how == .down || how == .up ? 0.08 : 0.15
            clip.animator().setBoundsOrigin(target)
        } completionHandler: {
            MainActor.assumeIsolated { scroll.reflectScrolledClipView(clip) }
        }
    }

    /// The next `show` redraws even the same item (new tags, a new theme).
    func invalidate() {
        shownPath = nil
    }

    func close() {
        cancelQuickLook()
        quickLook.close()
    }

    /// `p`: the full Quick Look preview of the item showing, instead of the instant one
    /// (also instead of bat for code and text). Shows at once if it was preloaded, else
    /// with a spinner until it's loaded. Returns false when it's already showing (`p` then
    /// opens the Quick Look window).
    func loadFull(for path: String?) -> Bool {
        guard let path, path == shownPath, full != .shown else { return false }
        fullRequested = true
        revealWhenReady = true
        fullWork?.cancel()
        if !textScroll.isHidden {
            // From bat's text (which sits above the preview): straight to the spinner.
            textScroll.isHidden = true
            thumbnail.isHidden = true
        }
        switch full {
        case .ready:
            crossfadeToFull(URL(fileURLWithPath: path), generation: generation)
        case .loading:
            showSpinner()
        case .none:
            showSpinner()
            loadFull(URL(fileURLWithPath: path), generation: generation)
        case .shown:
            break
        }
        return true
    }

    private func showSpinner() {
        hint.isHidden = true
        loading.isHidden = false
        spinner.startAnimation(nil)
    }

    /// Stops loading anything for the previous item.
    private func cancelQuickLook() {
        fullWork?.cancel()
        fullWork = nil
        resizeWork?.cancel()
        full = .none
        revealWhenReady = false
        usingPDF = false
        pdfView.document = nil
        pdfView.isHidden = true
        pinPDFToTop = false
        hint.isHidden = true
        info.isHidden = true
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
        pdfView.isHidden = true
        thumbnail.isHidden = true
        thumbnail.image = nil
        quickLook.autostarts = true
        quickLook.alphaValue = 1
        quickLook.isHidden = false
        quickLook.previewItem = URL(fileURLWithPath: path) as NSURL
        fullPath = path
        full = .shown
    }

    /// The instant preview: a thumbnail for the pane, or the icon and details. The full
    /// preview starts loading in the background once the selection rests, if it's cheap.
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
        hint.attributedStringValue = NSAttributedString(
            string: "\(Keymap.shared.key(.quickLook, in: .select)) full preview",
            attributes: [.font: Theme.shared.small, .foregroundColor: Theme.shared.colors.textDim.withAlphaComponent(0.7)])
        hint.isHidden = false
        gotThumbnail = false
        requestThumbnail(url, generation: current)
        guard let row, Self.preloads(row) else { return }
        let rest = DispatchWorkItem { [weak self] in
            guard let self, current == self.generation, self.full == .none else { return }
            self.loadFull(url, generation: current)
        }
        fullWork = rest
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.restBeforePreload, execute: rest)
    }

    /// A thumbnail at the pane's size (quick, cached by Quick Look); the icon and details
    /// if the file has none.
    private func requestThumbnail(_ url: URL, generation current: Int) {
        if let request = thumbnailRequest { QLThumbnailGenerator.shared.cancel(request) }
        let scale = window?.backingScaleFactor ?? 2
        let size = CGSize(width: max(bounds.width, 64), height: max(bounds.height - 30, 64))
        let request = QLThumbnailGenerator.Request(
            fileAt: url, size: size, scale: scale, representationTypes: [.lowQualityThumbnail, .thumbnail])
        thumbnailRequest = request
        QLThumbnailGenerator.shared.generateRepresentations(
            for: request,
            update: Self.onThumbnail { [weak self] image, final in
                guard let self, current == self.generation, self.full != .shown else { return }
                if let image {
                    self.gotThumbnail = true
                    self.thumbnail.imageScaling = .scaleProportionallyUpOrDown
                    self.thumbnail.image = image
                    self.info.isHidden = true
                } else if final, !self.gotThumbnail {
                    self.showIconAndDetails(url)
                }
            })
    }

    /// For files without a thumbnail (archives, unknown types): the icon at icon size, and
    /// what the file is.
    private func showIconAndDetails(_ url: URL) {
        let icon = NSWorkspace.shared.icon(forFile: url.path)
        icon.size = NSSize(width: 128, height: 128)
        thumbnail.imageScaling = .scaleNone
        thumbnail.image = icon
        let theme = Theme.shared
        let kind = (try? url.resourceValues(forKeys: [.localizedTypeDescriptionKey]))?.localizedTypeDescription ?? ""
        let s = NSMutableAttributedString(
            string: url.lastPathComponent + "\n",
            attributes: [.font: theme.listBold, .foregroundColor: theme.colors.text])
        var details = [kind]
        if let row {
            details.append(ByteCountFormatter.string(fromByteCount: Int64(row.size), countStyle: .file))
            let date = Date(timeIntervalSince1970: TimeInterval(row.modified))
            details.append("modified " + date.formatted(date: .abbreviated, time: .shortened))
        }
        s.append(NSAttributedString(
            string: details.filter { !$0.isEmpty }.joined(separator: " · "),
            attributes: [.font: theme.small, .foregroundColor: theme.colors.textDim]))
        info.attributedStringValue = s
        info.isHidden = false
    }

    /// Whether to load the full preview in the background: only where it's cheap and has
    /// no side effects. Cloud-only files would be downloaded, network files read over the
    /// network, and video or audio decoded (and possibly heard).
    private static func preloads(_ row: RowView) -> Bool {
        guard row.kind == .file, row.size <= preloadLimit else { return false }
        switch row.category {
        case .video, .audio: return false
        default: break
        }
        let url = URL(fileURLWithPath: row.path)
        let keys: Set<URLResourceKey> = [.volumeIsLocalKey, .isUbiquitousItemKey, .ubiquitousItemDownloadingStatusKey]
        if let v = try? url.resourceValues(forKeys: keys) {
            if v.volumeIsLocal == false { return false }
            if v.isUbiquitousItem == true, v.ubiquitousItemDownloadingStatus != .current { return false }
        }
        // Placeholders from other file providers (Dropbox, OneDrive, …).
        var info = stat()
        if lstat(row.path, &info) == 0, info.st_flags & UInt32(SF_DATALESS) != 0 { return false }
        return true
    }

    /// Keeps the thumbnail sharp when the pane changes size.
    override func setFrameSize(_ newSize: NSSize) {
        let old = frame.size
        super.setFrameSize(newSize)
        guard gotThumbnail, full != .shown, let path = shownPath,
              abs(newSize.width - old.width) > 40 || abs(newSize.height - old.height) > 40
        else { return }
        resizeWork?.cancel()
        let current = generation
        let work = DispatchWorkItem { [weak self] in
            guard let self, current == self.generation else { return }
            self.requestThumbnail(URL(fileURLWithPath: path), generation: current)
        }
        resizeWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: work)
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

    /// Loads the full preview invisibly; it's shown once `p` asks for it (now or later)
    /// and Quick Look's content view (drawn by another process) has attached and had a
    /// moment to draw: showing it sooner shows an empty frame.
    private func loadFull(_ url: URL, generation current: Int) {
        guard full == .none else { return }
        full = .loading
        if url.pathExtension.lowercased() == "pdf", let document = PDFDocument(url: url), !document.isLocked {
            usingPDF = true
            pdfView.backgroundColor = Theme.shared.colors.background
            pdfView.document = document
            pdfView.alphaValue = 0
            pdfView.isHidden = false
            // Drawn in this process: ready after a layout pass.
            DispatchQueue.main.async { [weak self] in
                guard let self, current == self.generation, self.full == .loading else { return }
                self.full = .ready
                if self.revealWhenReady { self.crossfadeToFull(url, generation: current) }
            }
            return
        }
        // Nothing plays until it's shown.
        quickLook.autostarts = revealWhenReady
        quickLook.previewItem = url as NSURL
        quickLook.alphaValue = 0
        quickLook.isHidden = false
        #if DEBUG
        // `TILDE_DEBUG_PREVIEW_FOCUS=1`: act like a preview that takes the keyboard.
        if ProcessInfo.processInfo.environment["TILDE_DEBUG_PREVIEW_FOCUS"] == "1" {
            window?.makeFirstResponder(quickLook)
        }
        #endif
        let started = Date()
        func waitForContent() {
            guard current == generation, full == .loading else { return }
            let attached = quickLook.subviews.contains { !$0.subviews.isEmpty }
            if attached || Date().timeIntervalSince(started) > Self.fullTimeout {
                DispatchQueue.main.asyncAfter(deadline: .now() + (attached ? Self.settle : 0)) { [weak self] in
                    guard let self, current == self.generation, self.full == .loading else { return }
                    self.full = .ready
                    if self.revealWhenReady { self.crossfadeToFull(url, generation: current) }
                }
                return
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.03) { waitForContent() }
        }
        waitForContent()
    }

    /// The full preview fades in over the opaque thumbnail (which goes once it's covered),
    /// so nothing dims partway through.
    private func crossfadeToFull(_ url: URL, generation current: Int) {
        guard current == generation else { return }
        full = .shown
        hint.isHidden = true
        info.isHidden = true
        quickLook.autostarts = true
        // Laid out while hidden (and maybe at another width), the PDF can sit partway down:
        // start at the top of the first page.
        if usingPDF, let scroll = pdfView.documentView?.enclosingScrollView {
            window?.layoutIfNeeded()
            pdfView.layoutDocumentView()
            Self.scrollToTop(scroll)
            pinPDFToTop = true
            // Settled by then; never fight the reader after that.
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in self?.pinPDFToTop = false }
        }
        let thumbnailShowing = !thumbnail.isHidden && thumbnail.image != nil
        NSAnimationContext.runAnimationGroup { context in
            context.duration = thumbnailShowing ? Self.crossfade : 0.1
            context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            self.fullView.animator().alphaValue = 1
            self.loading.animator().alphaValue = 0
        } completionHandler: {
            MainActor.assumeIsolated {
                // Ready for the next item whatever happened meanwhile.
                self.loading.alphaValue = 1
                guard current == self.generation else { return }
                self.fullPath = url.path
                self.thumbnail.isHidden = true
                self.textScroll.isHidden = true
                self.info.isHidden = true
                self.onFullShown?()
                self.spinner.stopAnimation(nil)
                self.loading.isHidden = true
            }
        }
    }

    private func showText(_ preview: TextPreview) {
        cancelQuickLook()
        quickLook.previewItem = nil
        quickLook.isHidden = true
        pdfView.isHidden = true
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
