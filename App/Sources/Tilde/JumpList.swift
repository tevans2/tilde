import Foundation

/// Somewhere the window can show.
enum Place: Equatable {
    case folder(String)
    case activity
}

/// A jump list entry: where the window was, and what it showed there.
struct Jump: Equatable {
    let place: Place
    /// Path of the highlighted item.
    let select: String?
    /// What was typed in the bar, so the same view comes back.
    let query: String
}

/// `⌃o` / `⌃i`, browser style: every move away records where it left; going back and
/// forward moves through those without recording; a new move drops what was backed out
/// of. One per window.
struct JumpList {
    static let limit = 100

    /// Places left, newest last, and places backed out of, newest last.
    private(set) var back: [Jump] = []
    private(set) var forward: [Jump] = []

    var canGoBack: Bool { !back.isEmpty }
    var canGoForward: Bool { !forward.isEmpty }

    /// Leaving `here` for `target` (nothing to record if it's the same place).
    mutating func leave(_ here: Jump, for target: Place) {
        guard here.place != target else { return }
        back.append(here)
        if back.count > Self.limit { back.removeFirst() }
        forward.removeAll()
    }

    /// The place to go to, recording `here` on the other side. Places `exists` rejects (a
    /// folder deleted since) are skipped. `nil` when there's nowhere to go.
    mutating func step(back goingBack: Bool, from here: Jump, exists: (Place) -> Bool) -> Jump? {
        while let target = goingBack ? back.popLast() : forward.popLast() {
            guard exists(target.place) else { continue }
            if goingBack { forward.append(here) } else { back.append(here) }
            return target
        }
        return nil
    }

    /// A move to somewhere else failed (it couldn't be opened): forget the entry it made
    /// for `place`, where the window still is.
    mutating func moveFailed(stayingAt place: Place) {
        if back.last?.place == place { back.removeLast() }
    }
}
