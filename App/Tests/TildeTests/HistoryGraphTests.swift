import Testing
import TildeFFI
@testable import Tilde

struct HistoryGraphTests {
    private func entry(_ id: UInt64, parent: UInt64?, state: HistoryState = .applied, head: Bool = false) -> HistoryEntry {
        HistoryEntry(
            id: id, label: "op \(id)", at: Int64(id), changed: Int64(id), state: state, parent: parent, isHead: head,
            changes: [], canUndo: state == .applied, canRedo: state == .undone)
    }

    @Test func aLineStaysInOneLane() {
        let rows = HistoryGraph.layout([entry(1, parent: nil), entry(2, parent: 1), entry(3, parent: 2, head: true)])
        #expect(rows.map { $0.entry?.id } == [3, 2, 1, nil], "newest first, then the start")
        #expect(rows.allSatisfy { $0.lane == 0 && $0.lanes == 1 })
        #expect(rows[0].isCurrent && !rows[3].isCurrent)
        #expect(!rows[0].above.contains(0) && rows[0].below.contains(0), "the tip has nothing above")
        #expect(rows[3].above.contains(0) && rows[3].below.isEmpty, "the start ends the line")
    }

    @Test func aBranchGetsItsOwnLaneAndJoinsWhereItStarted() {
        // 1 ─ 2 (undone)
        //  └─ 3 (current)
        let rows = HistoryGraph.layout([
            entry(1, parent: nil), entry(2, parent: 1, state: .undone), entry(3, parent: 1, head: true),
        ])
        #expect(rows.map { $0.entry?.id } == [3, 2, 1, nil])
        #expect(rows[0].lane == 0 && rows[1].lane == 1)
        #expect(rows[1].above.contains(0) && rows[1].below.contains(0), "the main line passes the branch")
        #expect(rows[2].lane == 0 && rows[2].joins == [1], "the branch joins 1")
        #expect(rows.allSatisfy { $0.lanes == 2 })
    }

    @Test func withNothingCurrentTheStartIs() {
        let rows = HistoryGraph.layout([entry(1, parent: nil, state: .undone)])
        #expect(rows.last?.entry == nil && rows.last?.isCurrent == true)
    }

    @Test func parentsOutsideTheListCountAsTheStart() {
        // An old parent dropped from the history leaves its child as a root.
        let rows = HistoryGraph.layout([entry(5, parent: 4, head: true)])
        #expect(rows.count == 2 && rows[1].above.contains(0))
    }
}
