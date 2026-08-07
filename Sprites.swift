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

    /// The columns the art actually uses, across every frame. Plants are laid out
    /// against this rather than the full 20 columns, so the transparent margins
    /// don't hold them apart — which is what made a label-less row look spread out.
    static let (inkMinX, inkWidth): (Int, Int) = {
        var lo = width, hi = -1
        for frames in Pack.builtin.poses.values {
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
    }()

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
