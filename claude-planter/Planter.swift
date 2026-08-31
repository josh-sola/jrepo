import AppKit

enum PlantState: String, CaseIterable {
    case working
    case waiting
    case attention

    init(raw: String) {
        self = PlantState(rawValue: raw) ?? .waiting
    }
}

struct Plant {
    /// Provider-qualified so session IDs from different agents cannot collide.
    var sessionID: String
    var provider: String = "claude"
    var rawSessionID: String = ""
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
    /// Index into the Palette.swift table. Sprites draw from `hue` alone, but
    /// the label needs the slot's own text colour, which a scalar hue can't
    /// recover on its own (two slots can round to the same one).
    var paletteSlot: Int = 0
    /// Where this plant starts its frame cycle. Taken from the session id rather
    /// than drawn at random, so a plant keeps its place for its whole life instead
    /// of jumping whenever a neighbour appears or the row is reordered.
    var phaseSeed: Int = 0
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

/// `~/.claude/sessions` is the authority whenever it answers; `claude agents
/// --json` only stands in when that directory is missing or unreadable, and it
/// misses sessions whose job record was never written.
private enum BackgroundSessions {
    /// Which sessions to hide as their own plants, plus which interactive session
    /// owns each one's work. Whether that work is running is decided elsewhere,
    /// from the background session's own state file.
    struct Snapshot {
        var backgroundIDs: Set<String> = []
        var owners: [String: String] = [:]
    }

    private static var sessionsDir: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".claude/sessions")
    }

    static func current() -> Snapshot {
        fromSessionsDir() ?? Snapshot(backgroundIDs: fromAgentsCLI())
    }

    private struct Record {
        var sessionID: String
        var kind: String
        var jobID: String?
        var cwd: String?
        var name: String?
    }

    /// nil means the directory itself could not answer, so the caller should
    /// fall back rather than treat silence as "nothing is background".
    private static func fromSessionsDir() -> Snapshot? {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: sessionsDir.path) else { return nil }

        var records: [Record] = []
        var sawRecord = false
        for name in names where name.hasSuffix(".json") {
            let url = sessionsDir.appendingPathComponent(name)
            guard let data = try? Data(contentsOf: url),
                  let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                  let sessionID = json["sessionId"] as? String,
                  let kind = json["kind"] as? String
            else { continue }
            sawRecord = true
            records.append(Record(
                sessionID: sessionID, kind: kind,
                jobID: json["jobId"] as? String,
                cwd: json["cwd"] as? String,
                name: json["name"] as? String
            ))
        }
        guard sawRecord else { return nil }

        let backgroundIDs = Set(records.filter { $0.kind == "bg" }.map(\.sessionID))
        let interactive = records.filter { $0.kind == "interactive" }

        var owners: [String: String] = [:]
        for job in records where job.kind == "bg" {
            guard let cwd = job.cwd, let name = job.name else { continue }
            let matches = interactive.filter { $0.cwd == cwd && $0.name == name }
            // A dispatch has no explicit parent link, so cwd+name is a guess.
            // Crediting it when more than one interactive session matches
            // could hang a bud on a plant that never started this work.
            guard matches.count == 1 else { continue }
            owners[job.sessionID] = matches[0].sessionID
        }

        return Snapshot(backgroundIDs: backgroundIDs, owners: owners)
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
        if let override = ProcessInfo.processInfo.environment["PLANTER_STATE_DIR"] {
            return URL(fileURLWithPath: override)
        }
        if let override = ProcessInfo.processInfo.environment["CLAUDE_PLANTER_DIR"] {
            return URL(fileURLWithPath: override)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".claude/planter")
    }()

    static var positionFile: URL { dir.appendingPathComponent("overlay-position.json") }
    static var orderFile: URL { dir.appendingPathComponent("order.json") }
    static var prefsFile: URL { dir.appendingPathComponent("prefs.json") }
    static var automaticColorsFile: URL { dir.appendingPathComponent("automatic-colors.json") }

    private static var reservedFiles: Set<String> {
        ["overlay-position.json", "order.json", "prefs.json", "automatic-colors.json"]
    }

    /// Reads every live plant, oldest session first. Deletes the state files of
    /// sessions whose process is gone, which is how crashed sessions disappear
    /// without a SessionEnd hook ever running.
    static func load() -> [Plant] {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: dir.path) else { return [] }

        let now = Date().timeIntervalSince1970

        var records: [(
            identity: String,
            provider: String,
            rawSessionID: String,
            json: [String: Any]
        )] = []
        for name in names where name.hasSuffix(".json") && !reservedFiles.contains(name) {
            let url = dir.appendingPathComponent(name)
            guard let data = try? Data(contentsOf: url),
                  let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
                  let rawSessionID = json["session_id"] as? String
            else { continue }

            let provider = (json["provider"] as? String) ?? "claude"
            let identity = (json["identity"] as? String) ?? "\(provider):\(rawSessionID)"

            if let pid = json["pid"] as? Int, pid > 1, !processAlive(pid) {
                try? fm.removeItem(at: url)
                continue
            }
            records.append((identity, provider, rawSessionID, json))
        }

        // Non-Claude records do not exist in Claude's session registry. Avoiding
        // this lookup for their rows keeps a missing Claude installation quiet.
        let sessions = records.contains { $0.provider == "claude" }
            ? BackgroundSessions.current() : BackgroundSessions.Snapshot()

        // The daemon's job file reports "blocked" or "done" while the session is
        // still taking turns, so the bud follows the same hook that drives every
        // other plant instead. An untouched file is no longer evidence either way.
        var attribution: [String: Int] = [:]
        for record in records
            where record.provider == "claude"
                && sessions.backgroundIDs.contains(record.rawSessionID) {
            guard let owner = sessions.owners[record.rawSessionID],
                  (record.json["state"] as? String) == "working",
                  let updated = record.json["updated_at"] as? Double,
                  now - updated <= staleAgentSeconds
            else { continue }
            attribution["claude:\(owner)", default: 0] += 1
        }

        var plants: [Plant] = []
        // A background dispatch rewrites its file on every event same as an
        // interactive one, so its plant is skipped here rather than deleted.
        for (sessionID, provider, rawSessionID, json) in records
            where provider != "claude" || !sessions.backgroundIDs.contains(rawSessionID) {
            var state = PlantState(raw: (json["state"] as? String) ?? "waiting")
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

            let attributed = attribution[sessionID] ?? 0
            agents += attributed
            // Buds only draw in the working pose, so a waiting plant with
            // attributed work must switch to it or the bud never shows. A
            // plant already blocked on you (attention) stays that way.
            if attributed > 0, state == .waiting { state = .working }

            plants.append(Plant(
                sessionID: sessionID,
                provider: provider,
                rawSessionID: rawSessionID,
                label: (json["label"] as? String) ?? provider,
                agents: agents,
                waitStage: wiltStage(waitingSince: since, now: now),
                state: state,
                createdAt: (json["created_at"] as? Double) ?? 0,
                phaseSeed: stableHash(sessionID),
                color: json["color"] as? String,
                tab: json["tab"] as? Int
            ))
        }

        plants.sort { ($0.createdAt, $0.sessionID) < ($1.createdAt, $1.sessionID) }
        var automaticColors = loadAutomaticColors()
        let savedAutomaticColors = automaticColors
        let liveSessionIDs = Set(plants.map(\.sessionID))
        pruneAutomaticColors(&automaticColors, keeping: liveSessionIDs)
        // Hues are assigned in creation order, before any reordering, so dragging a
        // plant along the row — or a later session claiming an earlier tab — never
        // changes an existing plant's colour.
        assignHues(&plants, automaticColors: &automaticColors)
        if automaticColors != savedAutomaticColors {
            saveAutomaticColors(automaticColors)
        }
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
            let currentA = rank[a.element.sessionID] ?? Int.max
            let currentB = rank[b.element.sessionID] ?? Int.max
            let legacyA = a.element.provider == "claude"
                ? rank[a.element.rawSessionID] ?? Int.max : Int.max
            let legacyB = b.element.provider == "claude"
                ? rank[b.element.rawSessionID] ?? Int.max : Int.max
            let resolvedA = min(currentA, legacyA)
            let resolvedB = min(currentB, legacyB)
            return resolvedA == resolvedB ? a.offset < b.offset : resolvedA < resolvedB
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
        // Merged rather than replaced: the pack name lives in this file too, and
        // only a hand edit puts it there.
        var prefs = loadPrefs()
        prefs["show_labels"] = show
        guard let data = try? JSONSerialization.data(withJSONObject: prefs) else { return }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? data.write(to: prefsFile)
    }

    private static func loadPrefs() -> [String: Any] {
        guard let data = try? Data(contentsOf: prefsFile),
              let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return [:] }
        return json
    }

    /// The globally configured pack name, nil if none is set. `--pack` overrides
    /// this for a single run without touching the file.
    static func loadPackName() -> String? {
        guard let data = try? Data(contentsOf: prefsFile),
              let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return nil }
        return json["pack"] as? String
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

    private static func loadAutomaticColors() -> [String: String] {
        guard let data = try? Data(contentsOf: automaticColorsFile),
              let colors = (try? JSONSerialization.jsonObject(with: data)) as? [String: String]
        else { return [:] }
        return colors
    }

    private static func saveAutomaticColors(_ colors: [String: String]) {
        guard let data = try? JSONSerialization.data(withJSONObject: colors) else { return }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? data.write(to: automaticColorsFile, options: .atomic)
    }

    private static func pruneAutomaticColors(
        _ colors: inout [String: String], keeping liveSessionIDs: Set<String>
    ) {
        colors = colors.filter { liveSessionIDs.contains($0.key) && paletteSlots[$0.value] != nil }
    }

    /// Initial assignments use creation order so display order cannot influence them.
    private static func assignHues(_ plants: inout [Plant], automaticColors: inout [String: String]) {
        let byCreation = plants.indices.sorted {
            (plants[$0].createdAt, plants[$0].sessionID) <
                (plants[$1].createdAt, plants[$1].sessionID)
        }

        var counts = Array(repeating: 0, count: palette.count)

        for i in byCreation {
            let sessionID = plants[i].sessionID
            if let name = plants[i].color, let slot = paletteSlots[name] {
                plants[i].hue = palette[slot].hue
                plants[i].paletteSlot = slot
                counts[slot] += 1
                automaticColors.removeValue(forKey: sessionID)
            } else if let name = automaticColors[sessionID], let slot = paletteSlots[name] {
                plants[i].hue = palette[slot].hue
                plants[i].paletteSlot = slot
                counts[slot] += 1
            }
        }

        for i in byCreation {
            let sessionID = plants[i].sessionID
            if paletteSlots[plants[i].color ?? ""] != nil || paletteSlots[automaticColors[sessionID] ?? ""] != nil {
                continue
            }
            let slot: Int
            let maximum = counts.max() ?? 0
            let eligible = counts.indices.filter { counts[$0] < maximum }
            let choices = eligible.isEmpty ? Array(counts.indices) : eligible
            slot = choices.randomElement()!
            automaticColors[sessionID] = palette[slot].name
            plants[i].hue = palette[slot].hue
            plants[i].paletteSlot = slot
            counts[slot] += 1
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
            let rawID = plants[i].sessionID.split(separator: ":", maxSplits: 1).last.map(String.init) ?? plants[i].sessionID
            let suffix = rawID.hasPrefix("thr_")
                ? String(rawID.dropFirst(4).prefix(4))
                : String(rawID.prefix(2))
            plants[i].display += "·" + suffix
        }
    }

    /// Swift's Hasher is seeded per process, so animation phases would jump on every
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
