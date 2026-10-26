import Foundation
import TildeFFI

@MainActor
protocol OpsEventsDelegate: AnyObject {
    func operationStarted(id: UInt64, label: String, total: Int)
    func operationProgress(id: UInt64, done: Int, total: Int, current: String)
    func operationBytes(id: UInt64, copied: UInt64, total: UInt64)
    func operationFinished(id: UInt64, outcome: OpOutcome)
}

/// Forwards file-operation callbacks from the Rust operations thread to the main thread.
final class OpsEvents: OpsListener, @unchecked Sendable {
    // Only read and written on the main thread.
    nonisolated(unsafe) weak var delegate: OpsEventsDelegate?

    func started(id: UInt64, label: String, total: UInt32) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { self.delegate?.operationStarted(id: id, label: label, total: Int(total)) }
        }
    }

    func progress(id: UInt64, done: UInt32, total: UInt32, current: String) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                self.delegate?.operationProgress(id: id, done: Int(done), total: Int(total), current: current)
            }
        }
    }

    func bytes(id: UInt64, copied: UInt64, total: UInt64) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { self.delegate?.operationBytes(id: id, copied: copied, total: total) }
        }
    }

    func finished(id: UInt64, outcome: OpOutcome) {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { self.delegate?.operationFinished(id: id, outcome: outcome) }
        }
    }
}
