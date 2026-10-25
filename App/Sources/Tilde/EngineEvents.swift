import Foundation
import TildeFFI

@MainActor
protocol EngineEventsDelegate: AnyObject {
    func engineResultsReady(_ info: ResultsInfo)
    func engineFolderFailed(generation: UInt64, folder: String, message: String)
}

/// Receives callbacks from the Rust worker thread and forwards them to the main thread,
/// in order.
final class EngineEvents: EngineListener, @unchecked Sendable {
    // Only read and written on the main thread.
    nonisolated(unsafe) weak var delegate: EngineEventsDelegate?

    func resultsReady(info: ResultsInfo) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.delegate?.engineResultsReady(info)
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
