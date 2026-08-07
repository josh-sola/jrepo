import AppKit

enum PlantState: String {
    /// Claude is doing something, including waiting on a subagent.
    case working
    /// Claude finished its turn: your move.
    case waiting
    /// Claude is blocked on a permission prompt or has gone quiet waiting for input.
    case attention

    init(raw: String) {
        self = PlantState(rawValue: raw) ?? .waiting
    }
}

struct Plant {
    var sessionID: String
    /// The directory or worktree name as recorded by the hook.
    var label: String
    /// What the overlay draws: shortened to keep the row tidy, then made unique.
    var display: String = ""
    /// How many subagents are running for this session, drawn as side buds.
    var agents: Int = 0
    /// How far gone the wilt is, from how long this session has been waiting.
    var waitStage: Int = 0
    var state: PlantState
    var createdAt: Double
    var hue: CGFloat = 0
    /// An explicit colour name from the launcher, e.g. "red". Reserved ahead of
    /// hash-derived hues so a collision can never steal a requested colour.
    var color: String? = nil
    /// 1-based row position from the launcher. Sessions without one sort to the
    /// end, in creation order.
    var tab: Int? = nil
}

/// Labels wider than this would push their plant's cell out of line with the
/// others, which is what makes a row of them hard to read at a glance.
private let maxLabelChars = 12

private func shorten(_ label: String) -> String {
    guard label.count > maxLabelChars else { return label }
    // Keep both ends: the tail of a branch or worktree name is usually the part
    // that distinguishes it.
    return label.prefix(6) + "…" + label.suffix(5)
}

/// How long a session must have been waiting for you before its plant wilts
/// further. Chosen so a session you are actively reading stays at stage 0, and one
/// you have forgotten ends up unmistakable.
private let wiltAfter: [Double] = [2 * 60, 10 * 60]

/// How long a subagent tally may stand without fresh news before it is treated as
/// stuck. Matches the bound in planter-state.
private let staleAgentSeconds: Double = 30 * 60

private func wiltStage(waitingSince since: Double, now: Double) -> Int {
    guard since > 0 else { return 0 }
    let age = now - since
    return wiltAfter.filter { age >= $0 }.count
}

/// Eight widely separated hues. Sessions pick one by hash, so a plant keeps its
/// colour for its whole life; collisions shift to the next free slot.
let plantHues: [CGFloat] = [0.02, 0.09, 0.15, 0.33, 0.47, 0.58, 0.72, 0.88]

/// The launcher's palette names, indexed the same as `plantHues` above, so
/// `PLANTER_COLOR=red` and a hashed slot of 0 land on the same hue.
private let colorSlots: [String: Int] = [
    "red": 0, "orange": 1, "yellow": 2, "green": 3,
    "cyan": 4, "blue": 5, "purple": 6, "pink": 7,
]

/// `~/.claude/sessions` is the authority whenever it answers; `claude agents
/// --json` only stands in when that directory is missing or unreadable, and it
/// misses sessions whose job record was never written.
private enum BackgroundSessions {
    private static var sessionsDir: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".claude/sessions")
    }

    static func current() -> Set<String> {
        fromSessionsDir() ?? fromAgentsCLI()
    }

    /// nil means the directory itself could not answer, so the caller should
    /// fall back rather than treat silence as "nothing is background".
    private static func fromSessionsDir() -> Set<String>? {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: sessionsDir.path) else { return nil }

        var ids = Set<String>()
        var sawRecord = false
        for name in names where name.hasSuffix(".json") {
            let url = sessionsDir.appendingPathComponent(name)
            guard let data = try? Data(contentsOf: url),
                  let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                  let sessionID = json["sessionId"] as? String,
                  let kind = json["kind"] as? String
            else { continue }
            sawRecord = true
            if kind == "bg" { ids.insert(sessionID) }
        }
        return sawRecord ? ids : nil
    }

    private static let ttl: Double = 5
    private static let timeout: Double = 2

    private static var cached: Set<String> = []
    private static var cachedAt: Double = 0

    private static func fromAgentsCLI() -> Set<String> {
        let now = Date().timeIntervalSince1970
        if now - cachedAt < ttl { return cached }
        cachedAt = now
        cached = fetchFromAgentsCLI()
        return cached
    }

    /// Fails open: a missing command, a timeout, or unexpected output all give
    /// an empty set, because a blank overlay is worse than the plants this
    /// hides.
    private static func fetchFromAgentsCLI() -> Set<String> {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["claude", "agents", "--json"]
        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()

        guard (try? process.run()) != nil else { return [] }

        // Read on another thread so a hung `claude` can be given up on instead
        // of blocking the draw loop indefinitely.
        let done = DispatchSemaphore(value: 0)
        var data = Data()
        DispatchQueue.global(qos: .utility).async {
            data = stdout.fileHandleForReading.readDataToEndOfFile()
            done.signal()
        }
        guard done.wait(timeout: .now() + timeout) == .success else {
            process.terminate()
            return []
        }
        process.waitUntilExit()

        guard process.terminationStatus == 0,
              let raw = try? JSONSerialization.jsonObject(with: data),
              let records = raw as? [[String: Any]]
        else { return [] }

        var ids = Set<String>()
        for record in records where record["kind"] as? String == "background" {
            if let sessionID = record["sessionId"] as? String { ids.insert(sessionID) }
        }
        return ids
    }
}

enum Store {
    static var dir: URL = {
        if let override = ProcessInfo.processInfo.environment["CLAUDE_PLANTER_DIR"] {
            return URL(fileURLWithPath: override)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".claude/planter")
    }()

    static var positionFile: URL { dir.appendingPathComponent("overlay-position.json") }
    static var orderFile: URL { dir.appendingPathComponent("order.json") }
    static var prefsFile: URL { dir.appendingPathComponent("prefs.json") }

    private static var reservedFiles: Set<String> {
        ["overlay-position.json", "order.json", "prefs.json"]
    }

    /// Reads every live plant, oldest session first. Deletes the state files of
    /// sessions whose process is gone, which is how crashed sessions disappear
    /// without a SessionEnd hook ever running.
    static func load() -> [Plant] {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: dir.path) else { return [] }

        let now = Date().timeIntervalSince1970
        let backgroundSessions = BackgroundSessions.current()
        var plants: [Plant] = []
        for name in names where name.hasSuffix(".json") && !reservedFiles.contains(name) {
            let url = dir.appendingPathComponent(name)
            guard let data = try? Data(contentsOf: url),
                  let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                  let sessionID = json["session_id"] as? String
            else { continue }

            if let pid = json["pid"] as? Int, pid > 1, !processAlive(pid) {
                try? fm.removeItem(at: url)
                continue
            }

            // A background dispatch rewrites this file on every event same as an
            // interactive one, so its plant is skipped here rather than deleted.
            if backgroundSessions.contains(sessionID) { continue }

            let state = PlantState(raw: (json["state"] as? String) ?? "waiting")
            let updated = (json["updated_at"] as? Double) ?? 0

            // A plant that was already waiting before this clock existed carries no
            // `since`, and nothing fires while a session waits, so it would never
            // get one — leaving the most neglected session looking like the
            // freshest. The file's last write is when it stopped, which is close
            // enough to stand in.
            var since = (json["since"] as? Double) ?? 0
            if since == 0, state != .working { since = updated }

            // The hook bounds a stuck agent tally on its next event, but a session
            // sitting idle sends none, so bound it here too.
            var agents = (json["agents"] as? Int) ?? 0
            let agentsAt = (json["agents_at"] as? Double) ?? 0
            if agents > 0, agentsAt > 0, now - agentsAt > staleAgentSeconds { agents = 0 }

            plants.append(Plant(
                sessionID: sessionID,
                label: (json["label"] as? String) ?? "claude",
                agents: agents,
                waitStage: wiltStage(waitingSince: since, now: now),
                state: state,
                createdAt: (json["created_at"] as? Double) ?? 0,
                color: json["color"] as? String,
                tab: json["tab"] as? Int
            ))
        }

        plants.sort { ($0.createdAt, $0.sessionID) < ($1.createdAt, $1.sessionID) }
        // Hues are assigned in creation order, before any reordering, so dragging a
        // plant along the row — or a later session claiming an earlier tab — never
        // changes an existing plant's colour.
        assignHues(&plants)
        // Display order: an explicit tab wins, then creation order for anyone
        // without one. applySavedOrder runs after and still has final say — a
        // ⌘-drag overrides everything, including an explicit tab.
        plants.sort {
            ($0.tab ?? Int.max, $0.createdAt, $0.sessionID) <
                ($1.tab ?? Int.max, $1.createdAt, $1.sessionID)
        }
        applySavedOrder(&plants)
        setDisplayLabels(&plants)
        return plants
    }

    /// Puts the plants in the order you dragged them into. Sessions that started
    /// since then have no saved place and follow, in creation order.
    private static func applySavedOrder(_ plants: inout [Plant]) {
        let order = loadOrder()
        guard !order.isEmpty else { return }

        var rank: [String: Int] = [:]
        for (i, id) in order.enumerated() where rank[id] == nil { rank[id] = i }

        plants = plants.enumerated().sorted { a, b in
            let ra = rank[a.element.sessionID] ?? Int.max
            let rb = rank[b.element.sessionID] ?? Int.max
            return ra == rb ? a.offset < b.offset : ra < rb
        }.map(\.element)
    }

    /// Whether labels were on last time. Nil means never chosen, so the default
    /// applies. Kept so that hiding them from the right-click menu survives a
    /// restart — otherwise a login item would bring them back every morning.
    static func loadShowLabels() -> Bool? {
        guard let data = try? Data(contentsOf: prefsFile),
              let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return nil }
        return json["show_labels"] as? Bool
    }

    static func saveShowLabels(_ show: Bool) {
        guard let data = try? JSONSerialization.data(withJSONObject: ["show_labels": show])
        else { return }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? data.write(to: prefsFile)
    }

    static func loadOrder() -> [String] {
        guard let data = try? Data(contentsOf: orderFile),
              let ids = (try? JSONSerialization.jsonObject(with: data)) as? [String]
        else { return [] }
        return ids
    }

    static func saveOrder(_ ids: [String]) {
        guard let data = try? JSONSerialization.data(withJSONObject: ids) else { return }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? data.write(to: orderFile)
    }

    private static func processAlive(_ pid: Int) -> Bool {
        // Signal 0 checks for existence without delivering anything. EPERM means
        // the process is alive but owned by someone else.
        if kill(pid_t(pid), 0) == 0 { return true }
        return errno == EPERM
    }

    /// Assigns hues in creation order, regardless of how `plants` is ordered when
    /// called — display order changes (a ⌘-drag, an explicit tab) must never
    /// recolour a plant. Explicit `color` requests are reserved first, so a
    /// hash-derived collision can only ever probe around them, never take one.
    private static func assignHues(_ plants: inout [Plant]) {
        let byCreation = plants.indices.sorted {
            (plants[$0].createdAt, plants[$0].sessionID) <
                (plants[$1].createdAt, plants[$1].sessionID)
        }

        var taken = Set<Int>()
        var reserved = Set<Int>()

        for i in byCreation {
            guard let name = plants[i].color, var slot = colorSlots[name] else { continue }
            var tries = 0
            while taken.contains(slot) && tries < plantHues.count {
                slot = (slot + 1) % plantHues.count
                tries += 1
            }
            taken.insert(slot)
            reserved.insert(i)
            plants[i].hue = plantHues[slot]
        }

        for i in byCreation where !reserved.contains(i) {
            var slot = abs(stableHash(plants[i].sessionID)) % plantHues.count
            var tries = 0
            while taken.contains(slot) && tries < plantHues.count {
                slot = (slot + 1) % plantHues.count
                tries += 1
            }
            taken.insert(slot)
            plants[i].hue = plantHues[slot]
        }
    }

    /// Shorten first, then disambiguate — otherwise two labels that shorten to the
    /// same thing would both be drawn identically. Sessions in the same directory
    /// get a stub of their session id, which at least stays put for their life.
    private static func setDisplayLabels(_ plants: inout [Plant]) {
        for i in plants.indices { plants[i].display = shorten(plants[i].label) }

        var counts: [String: Int] = [:]
        for plant in plants { counts[plant.display, default: 0] += 1 }
        for i in plants.indices where counts[plants[i].display]! > 1 {
            plants[i].display += "·" + String(plants[i].sessionID.prefix(2))
        }
    }

    /// Swift's Hasher is seeded per process, so plants would change colour on every
    /// restart. This one is stable.
    private static func stableHash(_ s: String) -> Int {
        var h = 5381
        for byte in s.utf8 { h = (h &* 33) &+ Int(byte) }
        return h
    }

    /// Where the row is pinned: its right edge and its bottom edge. Anchoring on
    /// the right is what makes a new session push the row leftwards, which keeps
    /// the overlay put when it lives at the right-hand end of a screen.
    struct Anchor {
        var right: CGFloat
        var bottom: CGFloat
    }

    /// - Parameter width: used only to convert a position saved by an older
    ///   version, which recorded the left edge instead.
    static func loadAnchor(width: CGFloat) -> Anchor? {
        guard let data = try? Data(contentsOf: positionFile),
              let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let y = json["y"] as? Double
        else { return nil }

        if let right = json["right"] as? Double {
            return Anchor(right: right, bottom: y)
        }
        if let left = json["x"] as? Double {
            // An older version saved the left edge. Convert once and write it back:
            // re-deriving it from the current width on every load would pin the
            // left edge for good, which is the behaviour this replaced.
            let migrated = Anchor(right: left + width, bottom: y)
            saveAnchor(migrated)
            return migrated
        }
        return nil
    }

    static func saveAnchor(_ anchor: Anchor) {
        let json: [String: Any] = ["right": anchor.right, "y": anchor.bottom]
        guard let data = try? JSONSerialization.data(withJSONObject: json) else { return }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? data.write(to: positionFile)
    }
}
