import AppKit
import TildeFFI

/// The folder as text (EDIT mode). Each line starts with a hidden `/<id> ` prefix that ties
/// it to an item: the prefix gets no glyphs, the cursor can't enter it, and edits can't cut
/// into it, but it's real text, so cut, copy and paste carry it along.
@MainActor
final class FolderEditor: NSView, NSTextViewDelegate, NSLayoutManagerDelegate {
    var onChange: (() -> Void)?
    var onApply: (() -> Void)?
    var onCancel: (() -> Void)?
    /// Quick mode (rename or one new line): `⏎` applies instead of starting a new line.
    var quick = false

    private let scroll = NSScrollView()
    private let textView: EditorTextView
    private let layoutManager = NSLayoutManager()
    private let storage = NSTextStorage()

    override init(frame: NSRect) {
        // TextKit 1, for the glyph hiding below.
        let container = NSTextContainer(size: NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = true
        storage.addLayoutManager(layoutManager)
        layoutManager.addTextContainer(container)
        textView = EditorTextView(frame: .zero, textContainer: container)
        super.init(frame: frame)

        layoutManager.delegate = self
        textView.delegate = self
        textView.isRichText = false
        textView.allowsUndo = true
        textView.drawsBackground = false
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.textContainerInset = NSSize(width: 16, height: 10)
        textView.isVerticallyResizable = true
        textView.autoresizingMask = [.width]

        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    var text: String { textView.string }

    /// Shows `text` with the cursor on line `line` (0-based). `selectName` selects the name
    /// up to its extension (rename); `newLineBelow` opens an empty line after it.
    func load(_ text: String, line: Int?, selectName: Bool, newLineBelow: Bool) {
        let theme = Theme.shared
        let paragraph = NSMutableParagraphStyle()
        paragraph.minimumLineHeight = theme.rowHeight
        paragraph.maximumLineHeight = theme.rowHeight
        let attrs: [NSAttributedString.Key: Any] = [
            .font: theme.list, .foregroundColor: theme.colors.text, .paragraphStyle: paragraph,
        ]
        textView.typingAttributes = attrs
        textView.insertionPointColor = theme.colors.text
        textView.selectedTextAttributes = [.backgroundColor: theme.colors.selection, .foregroundColor: theme.colors.selectionText]
        storage.setAttributedString(NSAttributedString(string: text, attributes: attrs))
        textView.undoManager?.removeAllActions()

        let ns = text as NSString
        var lineRange = NSRange(location: 0, length: 0)
        if let line {
            var start = 0
            for _ in 0..<line where start < ns.length {
                start = NSMaxRange(ns.lineRange(for: NSRange(location: start, length: 0)))
            }
            lineRange = ns.lineRange(for: NSRange(location: min(start, ns.length), length: 0))
        }
        let lineText = ns.substring(with: lineRange).trimmingCharacters(in: .newlines)
        let prefix = Self.prefixLength(lineText)
        if newLineBelow {
            // An empty folder: the first line is the new one.
            if ns.length > 0 {
                let lineEnd = lineRange.location + (lineText as NSString).length
                textView.setSelectedRange(NSRange(location: lineEnd, length: 0))
                textView.insertText("\n", replacementRange: NSRange(location: lineEnd, length: 0))
            }
        } else if selectName {
            let name = (lineText as NSString).substring(from: prefix)
            var stem = (name as NSString).length
            if name.hasSuffix("/") {
                stem -= 1
            } else if let dot = name.lastIndex(of: "."), dot != name.startIndex {
                stem = (String(name[..<dot]) as NSString).length
            }
            textView.setSelectedRange(NSRange(location: lineRange.location + prefix, length: stem))
        } else {
            textView.setSelectedRange(NSRange(location: lineRange.location + prefix, length: 0))
        }
        textView.scrollRangeToVisible(textView.selectedRange())
        onChange?()
    }

    func focus() {
        window?.makeFirstResponder(textView)
    }

    /// Tints lines (0-based) that have problems.
    func markErrors(_ lines: Set<Int>) {
        let ns = textView.string as NSString
        layoutManager.removeTemporaryAttribute(.backgroundColor, forCharacterRange: NSRange(location: 0, length: ns.length))
        var line = 0
        var start = 0
        while start < ns.length {
            let range = ns.lineRange(for: NSRange(location: start, length: 0))
            if lines.contains(line) {
                layoutManager.addTemporaryAttribute(
                    .backgroundColor, value: NSColor.systemRed.withAlphaComponent(0.18), forCharacterRange: range)
            }
            start = NSMaxRange(range)
            line += 1
        }
    }

    // MARK: Hidden prefixes

    /// Length of a line's `/<id> ` prefix, or 0 (mirrors `tilde_edit::prefix_len`).
    nonisolated static func prefixLength(_ line: String) -> Int {
        let u = Array(line.utf16.prefix(16))
        guard u.first == 0x2F else { return 0 } // "/"
        var i = 1
        while i < u.count, (0x30...0x39).contains(u[i]) { i += 1 }
        guard i > 1, i < u.count, u[i] == 0x20 else { return 0 } // at least one digit, then a space
        return i + 1
    }

    /// The hidden range of the line containing `index`.
    private func prefixRange(at index: Int) -> NSRange {
        let ns = textView.string as NSString
        guard index <= ns.length else { return NSRange(location: 0, length: 0) }
        let line = ns.lineRange(for: NSRange(location: min(index, max(ns.length - 1, 0)), length: 0))
        let length = Self.prefixLength(ns.substring(with: line))
        return NSRange(location: line.location, length: length)
    }

    /// Prefix characters get no glyphs. Reads the text through the layout manager it's
    /// given (layout runs on the main thread, but the protocol isn't isolated).
    nonisolated func layoutManager(
        _ layoutManager: NSLayoutManager,
        shouldGenerateGlyphs glyphs: UnsafePointer<CGGlyph>,
        properties props: UnsafePointer<NSLayoutManager.GlyphProperty>,
        characterIndexes charIndexes: UnsafePointer<Int>,
        font aFont: NSFont,
        forGlyphRange glyphRange: NSRange
    ) -> Int {
        guard let ns = layoutManager.textStorage?.string as NSString? else { return 0 }
        var properties = Array(UnsafeBufferPointer(start: props, count: glyphRange.length))
        var cachedLine = NSRange(location: NSNotFound, length: 0)
        var cachedPrefix = 0
        for i in 0..<glyphRange.length {
            let index = charIndexes[i]
            guard index < ns.length else { continue }
            if !NSLocationInRange(index, cachedLine) {
                cachedLine = ns.lineRange(for: NSRange(location: index, length: 0))
                cachedPrefix = Self.prefixLength(ns.substring(with: cachedLine))
            }
            if index < cachedLine.location + cachedPrefix {
                properties[i] = .null
            }
        }
        properties.withUnsafeBufferPointer { buffer in
            layoutManager.setGlyphs(glyphs, properties: buffer.baseAddress!, characterIndexes: charIndexes, font: aFont, forGlyphRange: glyphRange)
        }
        return glyphRange.length
    }

    // MARK: Keeping prefixes intact

    func textView(
        _ textView: NSTextView, willChangeSelectionFromCharacterRange old: NSRange, toCharacterRange new: NSRange
    ) -> NSRange {
        guard new.length == 0 else { return new }
        let prefix = prefixRange(at: new.location)
        guard prefix.length > 0, new.location >= prefix.location, new.location < NSMaxRange(prefix) else { return new }
        // Moving left out of a name goes to the end of the previous line; otherwise land
        // just after the prefix.
        if old.length == 0, old.location == NSMaxRange(prefix), new.location < old.location, prefix.location > 0 {
            return NSRange(location: prefix.location - 1, length: 0)
        }
        return NSRange(location: NSMaxRange(prefix), length: 0)
    }

    func textView(_ textView: NSTextView, shouldChangeTextIn range: NSRange, replacementString text: String?) -> Bool {
        let ns = textView.string as NSString
        // Every prefix the edit touches must be removed whole (deleting lines), not cut into.
        var start = ns.lineRange(for: NSRange(location: min(range.location, ns.length), length: 0)).location
        while start <= NSMaxRange(range), start < ns.length {
            let line = ns.lineRange(for: NSRange(location: start, length: 0))
            let prefix = NSRange(location: line.location, length: Self.prefixLength(ns.substring(with: line)))
            if prefix.length > 0 {
                let overlap = NSIntersectionRange(prefix, range)
                let insertsInside = range.length == 0 && range.location > prefix.location && range.location < NSMaxRange(prefix)
                if (overlap.length > 0 && overlap.length < prefix.length) || insertsInside {
                    NSSound.beep()
                    return false
                }
            }
            start = NSMaxRange(line)
        }
        return true
    }

    func textDidChange(_ notification: Notification) {
        onChange?()
    }

    func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.cancelOperation(_:)):
            onCancel?()
            return true
        case #selector(NSResponder.insertNewline(_:)) where quick:
            onApply?()
            return true
        default:
            return false
        }
    }
}

/// The editor's text view (a subclass so it can be told apart and extended).
final class EditorTextView: NSTextView {}

/// What applying the edit will do, shown where the preview normally is.
@MainActor
final class ChangesView: NSView {
    private let textView = NSTextView()
    private let scroll = NSScrollView()

    override init(frame: NSRect) {
        super.init(frame: frame)
        textView.isEditable = false
        textView.isSelectable = false
        textView.drawsBackground = false
        textView.textContainerInset = NSSize(width: 14, height: 12)
        // Track the scroll view's width; grow downwards.
        textView.isVerticallyResizable = true
        textView.autoresizingMask = [.width]
        textView.textContainer?.widthTracksTextView = true
        textView.minSize = NSSize(width: 0, height: 0)
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show(_ preview: EditPreview, quick: Bool) {
        let theme = Theme.shared
        let c = theme.colors
        let s = NSMutableAttributedString()
        func add(_ text: String, _ font: NSFont, _ color: NSColor) {
            s.append(NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color]))
        }
        if !preview.errors.isEmpty {
            add("FIX FIRST\n", theme.badge, .systemRed)
            for e in preview.errors {
                add("line \(e.line + 1)  ", theme.small, .systemRed)
                add("\(e.message)\n", theme.small, c.text)
            }
            add("\n", theme.small, c.text)
        }
        let n = preview.changes.count
        add(n == 0 ? "NO CHANGES YET\n" : "PENDING CHANGES  \(n)\n", theme.badge, c.textDim)
        if n == 0 {
            add("Edit names to rename, delete lines to trash, type a path to move, duplicate a "
                + "line to copy, add a line to create (end it with / for a folder).\n", theme.small, c.textDim)
        }
        for change in preview.changes {
            add(change.kind.uppercased().padding(toLength: 8, withPad: " ", startingAt: 0), theme.badge, color(change.kind))
            add("\(change.description)\n", theme.small, c.text)
        }
        add(quick ? "\n⏎ apply   esc cancel" : "\n⌘S apply   esc discard", theme.small, c.textDim)
        textView.textStorage?.setAttributedString(s)
    }

    private func color(_ kind: String) -> NSColor {
        let c = Theme.shared.colors
        switch kind {
        case "rename": return c.go
        case "move": return c.badge(.code)
        case "copy": return c.badge(.document)
        case "trash": return .systemRed
        default: return c.select
        }
    }
}
