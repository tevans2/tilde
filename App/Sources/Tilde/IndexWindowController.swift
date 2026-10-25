import AppKit
import TildeFFI

/// The Search Index window (⌘⇧I): how big the index is, its limit, the folders taking the
/// most room (with Exclude / Hide), and Rebuild / Clear.
@MainActor
final class IndexWindowController: NSWindowController, NSWindowDelegate {
    private let index: IndexService
    private let summary = NSTextField(wrappingLabelWithString: "")
    private var values: [String: NSTextField] = [:]
    private let folders = NSStackView()
    private let foldersNote = NSTextField(wrappingLabelWithString: "")
    private var timer: Timer?
    private var lastStatus: IndexStatus?

    private static let rows = ["Status", "Items", "Memory", "On disk", "Indexes", "Saved at"]

    init(index: IndexService) {
        self.index = index
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 620, height: 560),
            styleMask: [.titled, .closable, .resizable, .utilityWindow],
            backing: .buffered,
            defer: false
        )
        panel.title = "Search Index"
        panel.hidesOnDeactivate = false
        panel.minSize = NSSize(width: 480, height: 420)
        super.init(window: panel)
        panel.delegate = self
        build(in: panel)
        panel.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    // MARK: Layout

    private func build(in panel: NSPanel) {
        summary.font = .systemFont(ofSize: 13, weight: .medium)

        let grid = NSGridView(numberOfColumns: 2, rows: 0)
        grid.rowSpacing = 6
        grid.columnSpacing = 14
        for name in Self.rows {
            let key = NSTextField(labelWithString: name)
            key.textColor = .secondaryLabelColor
            key.alignment = .right
            let value = NSTextField(labelWithString: "—")
            value.font = Theme.shared.small
            value.lineBreakMode = .byTruncatingMiddle
            value.isSelectable = true
            value.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            values[name] = value
            grid.addRow(with: [key, value])
        }
        grid.column(at: 0).xPlacement = .trailing

        let foldersTitle = NSTextField(labelWithString: "Largest folders")
        foldersTitle.font = .systemFont(ofSize: 13, weight: .semibold)
        foldersNote.stringValue =
            "Exclude contents keeps the folder searchable but skips everything inside it. "
            + "Hide leaves it out entirely. Both are saved to your config and rebuild the index."
        foldersNote.textColor = .secondaryLabelColor
        foldersNote.font = .systemFont(ofSize: 11)

        folders.orientation = .vertical
        folders.alignment = .leading
        folders.spacing = 4
        let foldersScroll = NSScrollView()
        foldersScroll.hasVerticalScroller = true
        foldersScroll.drawsBackground = false
        foldersScroll.borderType = .noBorder
        let flipped = FlippedView()
        flipped.addSubview(folders)
        folders.translatesAutoresizingMaskIntoConstraints = false
        foldersScroll.documentView = flipped
        flipped.translatesAutoresizingMaskIntoConstraints = false

        let rebuild = NSButton(title: "Rebuild", target: self, action: #selector(rebuildIndex))
        rebuild.toolTip = "Re-read the config and index everything again"
        let clear = NSButton(title: "Clear", target: self, action: #selector(clearIndex))
        clear.toolTip = "Delete the saved index and empty it until you rebuild"
        let config = NSButton(title: "Edit Config…", target: self, action: #selector(editConfig))
        let refresh = NSButton(title: "Refresh", target: self, action: #selector(refreshAll))
        let buttons = NSStackView(views: [rebuild, clear, config, NSView(), refresh])
        buttons.orientation = .horizontal

        let stack = NSStackView(views: [summary, grid, foldersTitle, foldersNote, foldersScroll, buttons])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.setCustomSpacing(16, after: grid)
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 18, bottom: 16, right: 18)
        stack.translatesAutoresizingMaskIntoConstraints = false
        panel.contentView = NSView()
        panel.contentView!.addSubview(stack)

        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: panel.contentView!.topAnchor),
            stack.leadingAnchor.constraint(equalTo: panel.contentView!.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: panel.contentView!.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: panel.contentView!.bottomAnchor),
            summary.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -36),
            foldersNote.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -36),
            grid.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -36),
            foldersScroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -36),
            foldersScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 140),
            buttons.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -36),
            flipped.widthAnchor.constraint(equalTo: foldersScroll.contentView.widthAnchor),
            folders.topAnchor.constraint(equalTo: flipped.topAnchor),
            folders.leadingAnchor.constraint(equalTo: flipped.leadingAnchor),
            folders.trailingAnchor.constraint(equalTo: flipped.trailingAnchor),
            folders.bottomAnchor.constraint(equalTo: flipped.bottomAnchor),
        ])
    }

    // MARK: Showing

    override func showWindow(_ sender: Any?) {
        super.showWindow(sender)
        refreshAll()
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refreshInfo() }
        }
    }

    func windowWillClose(_ notification: Notification) {
        timer?.invalidate()
        timer = nil
    }

    @objc private func refreshAll() {
        refreshInfo()
        refreshFolders()
    }

    /// Stats are computed off the main thread (memory needs a pass over every item).
    private func refreshInfo() {
        let index = index
        Task.detached {
            let info = index.info()
            await MainActor.run { self.show(info) }
        }
    }

    private func refreshFolders() {
        let index = index
        Task.detached {
            let folders = index.largestFolders(limit: 12)
            await MainActor.run { self.show(folders) }
        }
    }

    private func show(_ info: IndexInfo) {
        let s = info.status
        let items = Int(s.items).formatted()
        let max = Int(info.maxItems).formatted()
        switch s.state {
        case .disabled:
            summary.stringValue = "Search indexing is turned off (enabled = false in the config)."
        case .cleared:
            summary.stringValue = "The index is cleared. Everywhere results are off until you rebuild."
        case .scanning:
            summary.stringValue = "Indexing… \(items) items so far."
        case .ready where s.capped:
            summary.stringValue =
                "The index is full: it stopped at \(max) items. Exclude large folders below, "
                + "or raise max_items in the config, then rebuild."
        case .ready:
            summary.stringValue = "Up to date. Changes on disk are picked up as they happen."
        }
        values["Status"]?.stringValue = switch s.state {
        case .disabled: "Off"
        case .cleared: "Cleared"
        case .scanning: "Indexing"
        case .ready: s.capped ? "Full" : "Ready"
        }
        values["Items"]?.stringValue = info.maxItems > 0 ? "\(items) of \(max) max" : items
        values["Memory"]?.stringValue = info.memoryBytes > 0 ? "≈ " + bytes(info.memoryBytes) : "—"
        values["On disk"]?.stringValue = info.diskBytes.map(bytes) ?? "Not saved yet"
        values["Indexes"]?.stringValue = info.roots.isEmpty ? "—" : info.roots.joined(separator: ", ")
        values["Saved at"]?.stringValue = info.cachePath

        // When a scan finishes, the largest folders are worth recomputing.
        if let last = lastStatus, last.state != s.state { refreshFolders() }
        lastStatus = s
    }

    private func show(_ list: [FolderInfo]) {
        folders.arrangedSubviews.forEach { $0.removeFromSuperview() }
        if list.isEmpty {
            let empty = NSTextField(labelWithString: "No folder holds a large share of the index.")
            empty.textColor = .secondaryLabelColor
            folders.addArrangedSubview(empty)
            return
        }
        for folder in list {
            let row = row(for: folder)
            folders.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: folders.widthAnchor).isActive = true
        }
    }

    private func row(for folder: FolderInfo) -> NSView {
        let name = NSTextField(labelWithString: folder.display)
        name.font = Theme.shared.small
        name.lineBreakMode = .byTruncatingMiddle
        name.toolTip = folder.path
        name.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        name.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let count = NSTextField(labelWithString: Int(folder.items).formatted())
        count.font = .monospacedDigitSystemFont(ofSize: 11.5, weight: .regular)
        count.textColor = .secondaryLabelColor
        count.alignment = .right
        count.widthAnchor.constraint(equalToConstant: 64).isActive = true
        let exclude = FolderButton(title: "Exclude contents", path: folder.path, hide: false, target: self,
                                   action: #selector(excludeFolder(_:)))
        let hide = FolderButton(title: "Hide", path: folder.path, hide: true, target: self,
                                action: #selector(excludeFolder(_:)))
        for b in [exclude, hide] {
            b.controlSize = .small
            b.bezelStyle = .rounded
        }
        let row = NSStackView(views: [name, count, exclude, hide])
        row.orientation = .horizontal
        row.distribution = .fill
        row.spacing = 8
        return row
    }

    // MARK: Actions

    @objc private func excludeFolder(_ sender: FolderButton) {
        do {
            try index.addExclusion(path: sender.path, hide: sender.hide)
            folders.arrangedSubviews.forEach { $0.removeFromSuperview() }
            refreshInfo()
        } catch {
            summary.stringValue = "Couldn't update the config: \(error.message)"
        }
    }

    @objc private func rebuildIndex() {
        let index = index
        Task.detached {
            index.rebuild()
            await MainActor.run { self.refreshAll() }
        }
    }

    @objc private func clearIndex() {
        let index = index
        Task.detached {
            index.clear()
            await MainActor.run { self.refreshAll() }
        }
    }

    @objc private func editConfig() {
        NSApp.sendAction(#selector(AppDelegate.openConfig(_:)), to: nil, from: self)
    }

    private func bytes(_ n: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: n), countStyle: .memory)
    }
}

/// A button that remembers which folder it acts on.
private final class FolderButton: NSButton {
    let path: String
    let hide: Bool

    init(title: String, path: String, hide: Bool, target: AnyObject, action: Selector) {
        self.path = path
        self.hide = hide
        super.init(frame: .zero)
        self.title = title
        self.target = target
        self.action = action
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }
}

/// Lets a stack view sit at the top of a scroll view.
private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}
