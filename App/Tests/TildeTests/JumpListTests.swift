import Testing
@testable import Tilde

struct JumpListTests {
    private func at(_ folder: String, _ query: String = "", select: String? = nil) -> Jump {
        Jump(place: .folder(folder), select: select, query: query)
    }

    private let everything: (Place) -> Bool = { _ in true }

    @Test func goesBackAndForward() {
        var list = JumpList()
        list.leave(at("/a", "rep", select: "/a/report.pdf"), for: .folder("/a/b"))
        list.leave(at("/a/b"), for: .activity)
        #expect(list.canGoBack && !list.canGoForward)

        let back = list.step(back: true, from: Jump(place: .activity, select: nil, query: ""), exists: everything)
        #expect(back == at("/a/b"))
        let back2 = list.step(back: true, from: at("/a/b"), exists: everything)
        #expect(back2 == at("/a", "rep", select: "/a/report.pdf"), "query and highlight come back")
        #expect(list.step(back: true, from: at("/a"), exists: everything) == nil)

        let forward = list.step(back: false, from: at("/a"), exists: everything)
        #expect(forward == at("/a/b"))
        #expect(list.canGoForward, "Activity is still ahead")
    }

    @Test func aNewMoveDropsWhatWasBackedOutOf() {
        var list = JumpList()
        list.leave(at("/a"), for: .folder("/b"))
        _ = list.step(back: true, from: at("/b"), exists: everything)
        #expect(list.canGoForward)
        list.leave(at("/a"), for: .folder("/c"))
        #expect(!list.canGoForward)
    }

    @Test func staysPutIsNotAJump() {
        var list = JumpList()
        list.leave(at("/a"), for: .folder("/a"))
        #expect(!list.canGoBack)
    }

    @Test func skipsFoldersThatAreGone() {
        var list = JumpList()
        list.leave(at("/gone"), for: .folder("/b"))
        list.leave(at("/b"), for: .folder("/c"))
        let exists: (Place) -> Bool = { $0 != .folder("/b") }
        #expect(list.step(back: true, from: at("/c"), exists: exists) == at("/gone"))
    }

    @Test func keepsTheNewestHundred() {
        var list = JumpList()
        for i in 0..<150 { list.leave(at("/\(i)"), for: .folder("/\(i + 1)")) }
        #expect(list.back.count == JumpList.limit)
        #expect(list.back.first == at("/50"))
    }

    @Test func aFailedMoveForgetsItsEntry() {
        var list = JumpList()
        list.leave(at("/a"), for: .folder("/locked"))
        list.moveFailed(stayingAt: .folder("/a"))
        #expect(!list.canGoBack)
    }
}
