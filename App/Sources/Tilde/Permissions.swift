import AppKit
import ApplicationServices

/// Full Disk Access and the folders macOS asks about (docs/SPEC.md §7).
enum Permissions {
    /// Folders macOS asks about before an app reads them, even without Full Disk Access.
    static let protectedFolders = ["Desktop", "Documents", "Downloads"]

    /// Whether Tilde has Full Disk Access: it can read a file only that allows.
    static var hasFullDiskAccess: Bool {
        let fd = Darwin.open(NSHomeDirectory() + "/Library/Application Support/com.apple.TCC/TCC.db", O_RDONLY)
        guard fd >= 0 else { return false }
        Darwin.close(fd)
        return true
    }

    static func openFullDiskAccessSettings() {
        openPane("Privacy_AllFiles")
    }

    /// Asks macOS to show its Accessibility prompt (once; after that, Settings).
    static func askForAccessibility() {
        // The value of kAXTrustedCheckOptionPrompt.
        AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary)
    }

    static func openAccessibilitySettings() {
        openPane("Privacy_Accessibility")
    }

    static func openFilesAndFoldersSettings() {
        openPane("Privacy_FilesAndFolders")
    }

    private static func openPane(_ pane: String) {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)") {
            NSWorkspace.shared.open(url)
        }
    }

    /// The name macOS shows in Privacy & Security for this process. Run from a terminal
    /// (not as an app), permissions belong to the terminal.
    static var grantee: String {
        guard Bundle.main.bundleIdentifier == nil else { return "Tilde" }
        let env = ProcessInfo.processInfo.environment
        // Set by the app that started the shell (also inside tmux, which isn't an app).
        if let id = env["__CFBundleIdentifier"], let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: id) {
            return FileManager.default.displayName(atPath: url.path).replacingOccurrences(of: ".app", with: "")
        }
        switch env["TERM_PROGRAM"] {
        case "Apple_Terminal": return "Terminal"
        case "iTerm.app": return "iTerm"
        case "tmux", "screen", nil, "": return "your terminal app"
        case let name?: return name
        }
    }

    /// Reads each protected folder so macOS asks now, while the reason is on screen, rather
    /// than in the middle of a search. Blocks while a question is showing, so it's called
    /// off the main thread. Returns which folders can be read.
    nonisolated static func askForFolders() -> [(String, Bool)] {
        protectedFolders.map { name in
            let path = NSHomeDirectory() + "/" + name
            return (name, (try? FileManager.default.contentsOfDirectory(atPath: path)) != nil)
        }
    }
}
