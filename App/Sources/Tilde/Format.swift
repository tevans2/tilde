import AppKit
import TildeFFI

@MainActor
enum Format {
    static func badgeText(_ row: RowView) -> String {
        switch row.kind {
        case .folder: return "DIR"
        case .symlink: return "LINK"
        case .package where row.category == .app: return "APP"
        default:
            let ext = row.ext.uppercased()
            switch ext {
            case "": return "FILE"
            case "SWIFT": return "SWFT"
            case "NUMBERS": return "NUM"
            default: return String(ext.prefix(4))
            }
        }
    }

    static func badgeColor(_ category: TildeFFI.Category) -> NSColor {
        Theme.shared.colors.badge(category)
    }

    /// Size for files, item count for folders.
    static func meta(_ row: RowView) -> String {
        switch row.kind {
        case .folder:
            guard let n = row.childCount else { return "" }
            return "\(n) item\(n == 1 ? "" : "s")"
        case .file:
            let f = ByteCountFormatter()
            f.countStyle = .file
            f.allowsNonnumericFormatting = false // "0 bytes", not "Zero KB"
            return f.string(fromByteCount: Int64(clamping: row.size))
        case .package, .symlink, .other:
            return ""
        }
    }

    /// Compact age: now, 5m, 3h, 2d, 4w, 6mo, 2y.
    static func age(_ modified: Int64, now: Date = Date()) -> String {
        let minutes = max(0, (now.timeIntervalSince1970 - Double(modified)) / 60)
        switch minutes {
        case ..<1: return "now"
        case ..<60: return "\(Int(minutes))m"
        case ..<1_440: return "\(Int(minutes / 60))h"
        case ..<10_080: return "\(Int(minutes / 1_440))d"
        case ..<86_400: return "\(Int(minutes / 10_080))w"
        case ..<525_600: return "\(Int(minutes / 43_200))mo"
        default: return "\(Int(minutes / 525_600))y"
        }
    }

    static func originName(_ origin: ActivityOrigin) -> String {
        switch origin {
        case .download: "Download"
        case .screenshot: "Screenshot"
        case .airDrop: "AirDrop"
        case .created: "Created"
        case .edited: "Edited"
        }
    }

    static func originColor(_ origin: ActivityOrigin) -> NSColor {
        let c = Theme.shared.colors
        switch origin {
        case .download: return c.go
        case .screenshot: return c.badge(.image)
        case .airDrop: return c.shelf
        case .edited: return c.badge(.code)
        case .created: return c.textDim
        }
    }

    static func filterName(_ filter: OriginFilter) -> String {
        switch filter {
        case .all: "All"
        case .downloads: "Downloaded"
        case .screenshots: "Screenshots"
        case .edited: "Edited"
        case .airDrop: "AirDrop"
        }
    }
}

extension Error {
    /// The text to show for an error. UniFFI's errors describe themselves as Swift values
    /// (`OpError.Invalid(message: …)`), so this unwraps their message.
    var message: String {
        switch self {
        case let e as OpError:
            switch e { case let .Invalid(message): return message }
        case let e as ConfigError:
            switch e { case let .Io(message): return message }
        default:
            return localizedDescription
        }
    }
}
