import AppKit
import TildeFFI

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
        switch category {
        case .folder: return .systemBlue
        case .app: return .systemIndigo
        case .image: return .systemGreen
        case .video: return .systemPink
        case .audio: return .systemMint
        case .pdf: return .systemRed
        case .document: return .systemTeal
        case .code: return .systemPurple
        case .text: return .systemGray
        case .archive: return .systemBrown
        case .other: return .systemGray
        }
    }

    /// Size for files, item count for folders.
    static func meta(_ row: RowView) -> String {
        switch row.kind {
        case .folder:
            guard let n = row.childCount else { return "" }
            return "\(n) item\(n == 1 ? "" : "s")"
        case .file:
            return ByteCountFormatter.string(fromByteCount: Int64(clamping: row.size), countStyle: .file)
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
}
