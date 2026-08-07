import AppKit

/// A glyph's hue: riding the session's own hue (with an offset, for a pack
/// that wants a variant), or fixed regardless of session.
enum PaletteHue {
    case session(offset: CGFloat)
    case literal(CGFloat)
}

struct GlyphColor {
    let hue: PaletteHue
    let saturation: CGFloat
    let brightness: CGFloat
    let alpha: CGFloat
}

/// A pose key is "<state>-<level>", e.g. "working-1"; level means subagent
/// count for `working` and wilt stage otherwise.
struct Pack {
    let width: Int
    let height: Int
    let glyphs: [Character: GlyphColor]
    let poses: [String: [[String]]]
    /// The columns the art actually uses, across every frame. Plants are laid out
    /// against this rather than the full canvas, so a pack's transparent margins
    /// don't hold its plants apart.
    let inkMinX: Int
    let inkWidth: Int
    /// How many points one of this pack's pixels is drawn at. Resolution belongs to
    /// the art rather than to the viewer: a pack drawn at twice the detail wants
    /// half the pixel size to come out the same size on screen, and only the pack
    /// knows that. Nil leaves the caller's own scale alone.
    let scale: CGFloat?

    init(width: Int, height: Int, glyphs: [Character: GlyphColor],
         poses: [String: [[String]]], scale: CGFloat? = nil) {
        self.width = width
        self.height = height
        self.glyphs = glyphs
        self.poses = poses
        self.scale = scale
        (inkMinX, inkWidth) = Pack.ink(width: width, poses: poses)
    }

    private static func ink(width: Int, poses: [String: [[String]]]) -> (Int, Int) {
        var lo = width, hi = -1
        for frames in poses.values {
            for frame in frames {
                for row in frame {
                    for (x, ch) in row.enumerated() where ch != "." && x < width {
                        lo = min(lo, x)
                        hi = max(hi, x)
                    }
                }
            }
        }
        guard hi >= lo else { return (0, width) }
        return (lo, hi - lo + 1)
    }

    func palette(hue: CGFloat) -> [Character: NSColor] {
        glyphs.mapValues { glyph in
            let resolvedHue: CGFloat
            switch glyph.hue {
            case .session(let offset):
                let raw = hue + offset
                resolvedHue = raw - floor(raw)
            case .literal(let value):
                resolvedHue = value
            }
            return NSColor(
                hue: resolvedHue, saturation: glyph.saturation,
                brightness: glyph.brightness, alpha: glyph.alpha
            )
        }
    }

    /// Falls back to another pose rather than nothing, so the caller always has
    /// something to draw. Sorted so a pack missing a pose looks the same twice.
    func frames(state: PlantState, level: Int) -> [[String]] {
        let key = "\(state.rawValue)-\(level)"
        if let frames = poses[key], !frames.isEmpty { return frames }
        return poses.sorted { $0.key < $1.key }.first(where: { !$0.value.isEmpty })?.value ?? [[]]
    }

    /// Rasterises one frame at native pixel size. Callers scale it up with
    /// interpolation off so the pixels stay square.
    func image(rows: [String], hue: CGFloat) -> NSImage {
        let colors = palette(hue: hue)
        var bytes = [UInt8](repeating: 0, count: width * height * 4)

        for (y, row) in rows.enumerated() where y < height {
            for (x, ch) in row.enumerated() where x < width {
                guard let color = colors[ch],
                      let rgb = color.usingColorSpace(.sRGB) else { continue }
                let i = (y * width + x) * 4
                let a = rgb.alphaComponent
                // Premultiplied alpha, which is what CGImage expects here.
                bytes[i + 0] = UInt8(rgb.redComponent * a * 255)
                bytes[i + 1] = UInt8(rgb.greenComponent * a * 255)
                bytes[i + 2] = UInt8(rgb.blueComponent * a * 255)
                bytes[i + 3] = UInt8(a * 255)
            }
        }

        let data = CFDataCreate(nil, bytes, bytes.count)!
        let provider = CGDataProvider(data: data)!
        let cg = CGImage(
            width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
            provider: provider, decode: nil, shouldInterpolate: false,
            intent: .defaultIntent
        )!
        return NSImage(cgImage: cg, size: NSSize(width: width, height: height))
    }
}

// Pixel art for the plants, on a 20x16 grid. Each plant is a potted flowering plant:
// blooming and swaying while Claude works, wilted when it needs you. Frames are
// built by stacking layers, so the pot stays put while the plant above it moves.
// One character per pixel, top row first. Later layers win.
//
//   .  transparent          f  petal            F  petal, shaded
//   g  leaf / stem          G  leaf, shaded     c  flower centre
//   s  soil                 p  pot              #  outline
//   w  glyph fill
//
// Rows shorter than 20 are padded with transparent pixels, so trailing dots are
// only there to keep the art readable in source.
enum Sprites {
    fileprivate static let width = 20
    fileprivate static let height = 16

    // The pot never moves, so every plant sits on the same baseline.
    private static let pot = [
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....#ssssss#........",
        "....#pppppp#........",
        ".....#pppp#.........",
        ".....#pppp#.........",
        ".....######.........",
    ]

    // Upright, in bloom, leaves out.
    private static let bloomA = [
        "....................",
        "......FFFF..........",
        ".....FfccfF.........",
        ".....FfccfF.........",
        "......FFFF..........",
        ".......Gg...........",
        ".......GggggG.......",
        ".......GgGG.........",
        "...GgggGg...........",
        ".....GGGg...........",
        ".......Gg...........",
    ]

    // The same plant leaning a pixel to the right: the two frames read as a sway.
    private static let bloomB = [
        "....................",
        ".......FFFF.........",
        "......FfccfF........",
        "......FfccfF........",
        ".......FFFF.........",
        "........Gg..........",
        "........GggggG......",
        ".......GgGG.........",
        "...GgggGg...........",
        ".....GGGg...........",
        ".......Gg...........",
    ]

    // Wilted: half the height, the bloom closed back into a bud, leaves hanging
    // with their tips curled under, and two petals dropped onto the soil. The
    // silhouette differs from the bloom at a glance, which is the whole point.
    private static let wilt = [
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        ".......FF...........",
        "......FFFF..........",
        ".......Gg...........",
        "......GGgG..........",
        "....GggGgggG........",
        "...Gg..Gg..gG.......",
        "....#sFssFs#........",
    ]

    // Wilting is progressive: the longer a session waits for you, the worse its
    // plant looks. A row of identically wilted plants cannot tell you which one you
    // have been ignoring longest, which is the thing you actually want to know.
    // Stage 1: the flower has dropped off, leaving a bare tip, and a third petal
    // has fallen.
    private static let wilt1 = [
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        ".......GG...........",
        ".......Gg...........",
        "......GGgG..........",
        "....GggGgggG........",
        "...Gg..Gg..gG.......",
        "....#FsFsFs#........",
    ]

    // Stage 2: collapsed onto the soil, most of the petals gone.
    private static let wilt2 = [
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        ".......G............",
        "...GggGgGGgg........",
        "....#FFssFF#........",
    ]

    /// How far gone a wilted plant is: 0 fresh, 1 flower dropped, 2 collapsed.
    private static let wiltStages = 3

    private static func wiltMap(_ stage: Int) -> [String] {
        [wilt, wilt1, wilt2][min(max(stage, 0), wiltStages - 1)]
    }

    // White body, dark outline, so it reads against a dark desktop and a light one
    // alike. Kept inside the plant's own columns, in the space the wilted plant
    // leaves above itself: a glyph sticking out to the side would widen every
    // plant's cell, including the ones not showing it.
    private static let bangGlyph = [
        "..........###.......",
        "..........#w#.......",
        "..........#w#.......",
        "..........#w#.......",
        "..........###.......",
        "....................",
        "..........###.......",
        "..........#w#.......",
        "..........###.......",
    ]

    // A side bloom on its own stalk for each running subagent, so a session that
    // has delegated its work looks different from one doing the work itself. They
    // sit in the open space either side of the flower, where they can be seen —
    // seedlings down in the soil were too small to read at this scale — and stay
    // inside the plant's existing columns so they cost no width.
    private static let maxBuds = 2

    private static let budRight = [
        "....................",
        "............F.......",
        "...........FcF......",
        "............F.......",
        "..........G.........",
        ".........G..........",
    ]

    private static let budLeft = [
        "....................",
        "....................",
        "....................",
        "....................",
        "....................",
        "....F...............",
        "...FcF..............",
        "....F.G.............",
    ]

    // MARK: frames

    /// Side buds are only drawn on a blooming plant: a session with agents running
    /// is never wilted anyway, because Stop keeps it blooming until the last one
    /// finishes.
    fileprivate static func buildPoses() -> [String: [[String]]] {
        var poses: [String: [[String]]] = [:]
        for n in 0...maxBuds {
            let buds = [budRight, budLeft].prefix(n)
            poses["working-\(n)"] = [composite([pot, bloomA] + buds), composite([pot, bloomB] + buds)]
        }
        for s in 0..<wiltStages {
            poses["waiting-\(s)"] = [composite([pot, wiltMap(s)])]
            poses["attention-\(s)"] = [composite([pot, wiltMap(s), bangGlyph])]
        }
        return poses
    }

    // MARK: layer maths

    private static func composite(_ layers: [[String]]) -> [String] {
        var grid = [[Character]](repeating: [Character](repeating: ".", count: width), count: height)
        for layer in layers {
            for (y, row) in layer.enumerated() where y < height {
                for (x, ch) in row.enumerated() where x < width {
                    if ch != "." { grid[y][x] = ch }
                }
            }
        }
        return grid.map { String($0) }
    }

    // MARK: colour

    /// Colours for one plant. The flower and the pot carry the plant's hue, so each
    /// session is its own variety; leaves and soil stay the colours of leaves and
    /// soil, which is what keeps it looking like a plant.
    fileprivate static let glyphs: [Character: GlyphColor] = [
        "f": GlyphColor(hue: .session(offset: 0), saturation: 0.66, brightness: 1.00, alpha: 1),
        "F": GlyphColor(hue: .session(offset: 0), saturation: 0.84, brightness: 0.80, alpha: 1),
        "c": GlyphColor(hue: .literal(0.13), saturation: 0.50, brightness: 1.00, alpha: 1),
        "g": GlyphColor(hue: .literal(0.33), saturation: 0.62, brightness: 0.74, alpha: 1),
        "G": GlyphColor(hue: .literal(0.34), saturation: 0.76, brightness: 0.44, alpha: 1),
        "s": GlyphColor(hue: .literal(0.08), saturation: 0.52, brightness: 0.34, alpha: 1),
        "p": GlyphColor(hue: .session(offset: 0), saturation: 0.42, brightness: 0.82, alpha: 1),
        "#": GlyphColor(hue: .session(offset: 0), saturation: 0.58, brightness: 0.32, alpha: 1),
        "w": GlyphColor(hue: .literal(0), saturation: 0.00, brightness: 1.00, alpha: 1),
    ]
}

extension Pack {
    static let builtin = Pack(
        width: Sprites.width, height: Sprites.height,
        glyphs: Sprites.glyphs, poses: Sprites.buildPoses()
    )
}

// MARK: - User packs

/// Why a pack on disk was rejected. Every case names one check from the pack
/// format; the message is what lands on stderr next to the pack's name.
enum PackLoadError: Error, CustomStringConvertible {
    case configMissing
    case sizeMissing
    case sizeInvalid(String)
    case scaleInvalid(String)
    case paletteLine(String)
    case dotPaletted
    case dirUnreadable
    case poseMissing(String)
    case poseConflict(String)
    case strayFrame(String)
    case frameUnreadable(String)
    case rowCount(String, expected: Int, found: Int)
    case rowTooLong(String)
    case unmappedGlyph(Character, String)

    var description: String {
        switch self {
        case .configMissing: return "has no readable pack.conf"
        case .sizeMissing: return "pack.conf has no size"
        case .sizeInvalid(let line): return "pack.conf has an invalid size: \(line)"
        case .scaleInvalid(let line): return "pack.conf has an invalid scale: \(line)"
        case .paletteLine(let line): return "pack.conf has an invalid palette line: \(line)"
        case .dotPaletted: return "pack.conf gives '.' a palette entry"
        case .dirUnreadable: return "has no readable pack directory"
        case .poseMissing(let pose): return "is missing pose \(pose)"
        case .poseConflict(let pose): return "has both a bare and suffixed file for pose \(pose)"
        case .strayFrame(let file): return "has a frame file no pose uses: \(file)"
        case .frameUnreadable(let file): return "has no readable frame \(file)"
        case .rowCount(let file, let expected, let found):
            return "\(file) has \(found) rows, expected \(expected)"
        case .rowTooLong(let file): return "\(file) has a row longer than the declared width"
        case .unmappedGlyph(let ch, let pose): return "pose \(pose) uses glyph '\(ch)' with no palette entry"
        }
    }
}

/// Resolves the pack a user asked for, loading and validating it from disk.
/// Any failure at all falls back to `Pack.builtin` in its entirety — a
/// half-loaded pack must never reach the renderer.
enum PackLoader {
    /// The fixed 3x3 pose grid: state × level are planter's, not the pack's.
    private static let levels = 0...2

    static func resolve(name: String?) -> Pack {
        guard let name = name, !name.isEmpty else { return .builtin }
        do {
            return try load(name: name)
        } catch let error as PackLoadError {
            warn(name: name, error.description)
            return .builtin
        } catch {
            warn(name: name, "\(error)")
            return .builtin
        }
    }

    private static func warn(name: String, _ message: String) {
        FileHandle.standardError.write(
            "planter: pack \"\(name)\" \(message) — using built-in\n".data(using: .utf8)!
        )
    }

    private static func directory(name: String) -> URL {
        let base: URL
        if let configHome = ProcessInfo.processInfo.environment["XDG_CONFIG_HOME"], !configHome.isEmpty {
            base = URL(fileURLWithPath: configHome)
        } else {
            base = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config")
        }
        return base.appendingPathComponent("planter").appendingPathComponent(name)
    }

    private static func load(name: String) throws -> Pack {
        let dir = directory(name: name)
        guard let text = try? String(contentsOf: dir.appendingPathComponent("pack.conf"), encoding: .utf8)
        else { throw PackLoadError.configMissing }

        let (width, height, glyphs, scale) = try parseConf(text)
        let poses = try loadPoses(dir: dir, width: width, height: height, glyphs: glyphs)
        return Pack(width: width, height: height, glyphs: glyphs, poses: poses, scale: scale)
    }

    // MARK: pack.conf

    private static func parseConf(_ text: String) throws -> (Int, Int, [Character: GlyphColor], CGFloat?) {
        var width: Int?
        var height: Int?
        var scale: CGFloat?
        var glyphs: [Character: GlyphColor] = [:]

        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            guard !line.isEmpty else { continue }

            let sides = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }

            // A palette line is recognized by shape, not by a leading '#', so a
            // glyph literally named '#' can still be given a colour.
            guard sides.count == 2,
                  sides[0] == "size" || sides[0] == "scale" || sides[0].count == 1
            else {
                if line.hasPrefix("#") { continue }
                throw PackLoadError.paletteLine(line)
            }
            let key = sides[0], value = sides[1]

            if key == "size" {
                let dims = value.split(separator: " ").compactMap { Int($0) }
                guard dims.count == 2, dims[0] > 0, dims[1] > 0 else { throw PackLoadError.sizeInvalid(line) }
                (width, height) = (dims[0], dims[1])
                continue
            }

            if key == "scale" {
                guard let n = Double(value), n > 0 else { throw PackLoadError.scaleInvalid(line) }
                scale = CGFloat(n)
                continue
            }

            guard let glyph = key.first else { throw PackLoadError.paletteLine(line) }
            if glyph == "." { throw PackLoadError.dotPaletted }

            let fields = value.split(separator: " ").map(String.init)
            guard fields.count == 3 || fields.count == 4,
                  let hue = parseHue(fields[0]),
                  let saturation = Double(fields[1]), let brightness = Double(fields[2])
            else { throw PackLoadError.paletteLine(line) }
            let alpha: Double
            if fields.count == 4 {
                guard let a = Double(fields[3]) else { throw PackLoadError.paletteLine(line) }
                alpha = a
            } else {
                alpha = 1
            }

            glyphs[glyph] = GlyphColor(
                hue: hue, saturation: CGFloat(saturation), brightness: CGFloat(brightness), alpha: CGFloat(alpha)
            )
        }

        guard let w = width, let h = height else { throw PackLoadError.sizeMissing }
        return (w, h, glyphs, scale)
    }

    private static func parseHue(_ field: String) -> PaletteHue? {
        if field == "session" { return .session(offset: 0) }
        if field.hasPrefix("session+"), let offset = Double(field.dropFirst(8)) {
            return .session(offset: CGFloat(offset))
        }
        if field.hasPrefix("session-"), let offset = Double(field.dropFirst(8)) {
            return .session(offset: -CGFloat(offset))
        }
        guard let literal = Double(field) else { return nil }
        return .literal(CGFloat(literal))
    }

    // MARK: frame files

    private static func loadPoses(
        dir: URL, width: Int, height: Int, glyphs: [Character: GlyphColor]
    ) throws -> [String: [[String]]] {
        guard let entries = try? FileManager.default.contentsOfDirectory(atPath: dir.path)
        else { throw PackLoadError.dirUnreadable }
        let frameFiles = Set(entries.filter { $0.hasSuffix(".txt") })

        var poses: [String: [[String]]] = [:]
        var claimed: Set<String> = []
        for state in PlantState.allCases {
            for level in levels {
                let pose = "\(state.rawValue)-\(level)"
                let bare = "\(pose).txt"
                let suffixed = frameFiles.filter { $0.hasPrefix("\(pose)-") }.sorted()

                let files: [String]
                switch (frameFiles.contains(bare), suffixed.isEmpty) {
                case (true, true): files = [bare]
                case (false, false): files = suffixed
                case (true, false): throw PackLoadError.poseConflict(pose)
                case (false, true):
                    // Level 0 is the state itself and has to be drawn — nothing can
                    // stand in for "blocked on you". The levels above it are only
                    // gradations within that state, so a pack that doesn't count
                    // subagents or grade its wilt can leave them out and get level 0.
                    guard level > 0, let base = poses["\(state.rawValue)-0"] else {
                        throw PackLoadError.poseMissing(pose)
                    }
                    poses[pose] = base
                    continue
                }

                claimed.formUnion(files)
                poses[pose] = try files.map { try loadFrame(dir.appendingPathComponent($0), width: width, height: height) }
            }
        }

        // Falling back on an absent pose would otherwise swallow a misspelt frame
        // name, handing back a pack that quietly draws the wrong art.
        if let stray = frameFiles.subtracting(claimed).sorted().first {
            throw PackLoadError.strayFrame(stray)
        }

        try validate(poses: poses, glyphs: glyphs)
        return poses
    }

    private static func loadFrame(_ url: URL, width: Int, height: Int) throws -> [String] {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else {
            throw PackLoadError.frameUnreadable(url.lastPathComponent)
        }
        var rows = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if rows.last == "" { rows.removeLast() }
        guard rows.count == height else {
            throw PackLoadError.rowCount(url.lastPathComponent, expected: height, found: rows.count)
        }
        return try rows.map { row in
            guard row.count <= width else { throw PackLoadError.rowTooLong(url.lastPathComponent) }
            return row.count < width ? row + String(repeating: ".", count: width - row.count) : row
        }
    }

    private static func validate(poses: [String: [[String]]], glyphs: [Character: GlyphColor]) throws {
        for (pose, frames) in poses.sorted(by: { $0.key < $1.key }) {
            for frame in frames {
                for row in frame {
                    for ch in row where ch != "." && glyphs[ch] == nil {
                        throw PackLoadError.unmappedGlyph(ch, pose)
                    }
                }
            }
        }
    }
}
