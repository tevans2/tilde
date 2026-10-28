import AppKit

/// A window opened by `tilde --cd` (App/bin/tilde): when it closes, the folder it was on
/// goes back down a named pipe the command is waiting on, and the terminal comes to the
/// front again.
struct TerminalReturn {
    /// The named pipe `tilde --cd` reads.
    let reply: String
    /// The terminal app's bundle identifier, to bring it back to the front.
    let app: String?

    /// Only a named pipe of this user's: a `tilde://` link from anywhere else can't make
    /// Tilde write to a file.
    static func isReplyPipe(_ path: String) -> Bool {
        var info = stat()
        guard lstat(path, &info) == 0 else { return false }
        return info.st_mode & S_IFMT == S_IFIFO && info.st_uid == getuid()
    }

    func finish(folder: String) {
        if Self.isReplyPipe(reply) {
            // Non-blocking: if the command was interrupted, nobody's reading.
            let fd = open(reply, O_WRONLY | O_NONBLOCK)
            if fd >= 0 {
                let line = folder + "\n"
                _ = line.withCString { write(fd, $0, strlen($0)) }
                close(fd)
            }
        }
        if let app, let terminal = NSRunningApplication.runningApplications(withBundleIdentifier: app).first {
            terminal.activate()
        }
    }
}
