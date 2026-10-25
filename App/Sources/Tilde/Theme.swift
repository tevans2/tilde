import AppKit
import TildeFFI

extension NSFont {
    /// The bold face of the same family and size.
    var bold: NSFont { NSFontManager.shared.convert(self, toHaveTrait: .boldFontMask) }
}

/// Fonts and sizes derived from the user's config file.
@MainActor
final class Theme {
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
