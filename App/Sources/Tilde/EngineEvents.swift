import Foundation
import TildeFFI

@MainActor
protocol EngineEventsDelegate: AnyObject {
    func engineResultsReady(generation: UInt64, folder: String, hereCount: Int, everywhereCount: Int)
    func engineFolderFailed(generation: UInt64, folder: String, message: String)
}

/// Receives callbacks from the Rust worker thread and forwards them to the main thread,
/// in order.
final class EngineEvents: EngineListener, @unchecked Sendable {
    // Only read and written on the main thread.
    nonisolated(unsafe) weak var delegate: EngineEventsDelegate?

    func resultsReady(generation: UInt64, folder: String, hereCount: UInt32, everywhereCount: UInt32) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.delegate?.engineResultsReady(
                    generation: generation, folder: folder, hereCount: Int(hereCount), everywhereCount: Int(everywhereCount))
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
