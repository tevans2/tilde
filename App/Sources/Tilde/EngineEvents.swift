import Foundation
import TildeFFI

@MainActor
protocol EngineEventsDelegate: AnyObject {
    func engineResultsReady(generation: UInt64, folder: String, count: Int)
    func engineFolderFailed(generation: UInt64, folder: String, message: String)
}

/// Receives callbacks from the Rust worker thread and forwards them to the main thread,
/// in order.
final class EngineEvents: EngineListener, @unchecked Sendable {
    // Only read and written on the main thread.
    nonisolated(unsafe) weak var delegate: EngineEventsDelegate?

    func resultsReady(generation: UInt64, folder: String, count: UInt32) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.delegate?.engineResultsReady(generation: generation, folder: folder, count: Int(count))
            }
        }
    }

    func folderFailed(generation: UInt64, folder: String, message: String) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.delegate?.engineFolderFailed(generation: generation, folder: folder, message: message)
            }
        }
    }
}
