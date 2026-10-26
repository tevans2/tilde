import AppKit
import TildeFFI

extension NSFont {
    /// The bold face of the same family and size.
    var bold: NSFont { NSFontManager.shared.convert(self, toHaveTrait: .boldFontMask) }
}

/// Fonts, colours and preview settings derived from the user's config file.
@MainActor
final class Theme {
    /// Colours from `[theme]`. Anything the config doesn't set is the system colour, and
    /// colours follow light and dark appearance.
    struct Colors {
        let background, bar, border, text, textDim: NSColor
        let selection, selectionText, match, matchSelected: NSColor
        let go, select, shelf, badgeLabel: NSColor
        let badges: [TildeFFI.Category: NSColor]

        func badge(_ category: TildeFFI.Category) -> NSColor { badges[category] ?? .systemGray }
    }

    private(set) static var shared = Theme(loadConfig())

    /// Re-reads the config file.
    static func reload() -> Theme {
        shared = Theme(loadConfig())
        return shared
    }

    let configPath: String
    /// Problems to tell the user about (bad config, missing font).
    let warnings: [String]

    let list: NSFont
    let listBold: NSFont
    let bar: NSFont
    let small: NSFont
    let badge: NSFont
    let rowHeight: CGFloat

    let previewVisible: Bool
    /// Share of the window's width.
    let previewWidth: Double
    let previewBat: Bool
    let batThemeLight: String
    /// Empty uses bat's own config.
    let batThemeDark: String
    let batMaxLines: Int
    /// EDIT runs in Neovim.
    let editorNvim: Bool
    /// The file-dialog helper's hotkey (nil when it's off).
    let dialogHotkey: HotkeyInfo?
    let dialogConfirm: Bool

    let colors: Colors
    /// `nil` follows the system.
    let appearance: NSAppearance?

    init(_ load: ConfigLoad) {
        configPath = load.path
        var warnings = load.error.map { [$0] } ?? []
        let requested = load.config.fontFamily
        let family = Self.resolveFamily(requested)
        if family == nil {
            warnings.append("Font “\(requested)” isn't installed, using the system monospaced font.")
        }
        self.warnings = warnings

        let size = CGFloat(load.config.fontSize)
        func font(_ size: CGFloat, bold: Bool = false) -> NSFont {
            if let family,
               let f = NSFontManager.shared.font(
                   withFamily: family, traits: bold ? .boldFontMask : [], weight: bold ? 9 : 5, size: size)
            {
                return f
            }
            return .monospacedSystemFont(ofSize: size, weight: bold ? .bold : .regular)
        }
        list = font(size)
        listBold = font(size, bold: true)
        bar = font(size + 2)
        small = font(max(size - 1.5, 8))
        badge = font(max(size - 4, 7.5), bold: true)
        rowHeight = (size * 2).rounded()

        previewVisible = load.config.previewVisible
        previewWidth = load.config.previewWidth
        previewBat = load.config.previewBat
        batThemeLight = load.config.batThemeLight
        batThemeDark = load.config.batThemeDark
        batMaxLines = Int(load.config.batMaxLines)
        editorNvim = load.config.editorNvim
        dialogHotkey = load.config.dialogHotkey
        dialogConfirm = load.config.dialogConfirm

        appearance = switch load.config.appearance {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
        let light = load.config.lightColors
        let dark = load.config.darkColors
        /// A colour that switches with the appearance, or `fallback` where unset.
        func color(_ key: String, _ fallback: NSColor) -> NSColor {
            let (l, d) = (light[key], dark[key])
            guard l != nil || d != nil else { return fallback }
            return NSColor(name: nil) { appearance in
                let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
                guard let rgba = isDark ? (d ?? l) : (l ?? d) else { return fallback }
                return NSColor(rgba: rgba)
            }
        }
        colors = Colors(
            background: color("background", .windowBackgroundColor),
            bar: color("bar", .windowBackgroundColor),
            border: color("border", .separatorColor),
            text: color("text", .labelColor),
            textDim: color("text_dim", .secondaryLabelColor),
            selection: color("selection", .selectedContentBackgroundColor),
            selectionText: color("selection_text", .alternateSelectedControlTextColor),
            match: color("match", .systemOrange),
            matchSelected: color("match_selected", .systemYellow),
            go: color("go", .systemBlue),
            select: color("select", .systemGreen),
            shelf: color("shelf", .systemGreen),
            badgeLabel: color("badge_label", .white),
            badges: [
                .folder: color("badge_folder", .systemBlue),
                .app: color("badge_app", .systemIndigo),
                .image: color("badge_image", .systemGreen),
                .video: color("badge_video", .systemPink),
                .audio: color("badge_audio", .systemMint),
                .pdf: color("badge_pdf", .systemRed),
                .document: color("badge_document", .systemTeal),
                .code: color("badge_code", .systemPurple),
                .text: color("badge_text", .systemGray),
                .archive: color("badge_archive", .systemBrown),
                .other: color("badge_other", .systemGray),
            ])
    }

    /// Exact (case-insensitive) family match, else the shortest installed family whose
    /// name starts with the request, ignoring spaces and dashes. "JetBrains Mono" finds
    /// "JetBrainsMono Nerd Font".
    static func resolveFamily(_ requested: String) -> String? {
        let families = NSFontManager.shared.availableFontFamilies
        if let exact = families.first(where: { $0.caseInsensitiveCompare(requested) == .orderedSame }) {
            return exact
        }
        func normalized(_ s: String) -> String { s.lowercased().filter { !$0.isWhitespace && $0 != "-" } }
        let want = normalized(requested)
        guard !want.isEmpty else { return nil }
        return families.filter { normalized($0).hasPrefix(want) }.min { $0.count < $1.count }
    }
}
