import AppKit

/// Mirrors the canonical /palette.json at the repo root, one row per entry in
/// the same order — tools/check-palette regex-extracts the table below and
/// compares it against that file, so each line's formatting must match it
/// exactly. Names are what state files persist (a session's own state file,
/// automatic-colors.json, PLANTER_COLOR), so their spelling is load-bearing.
/// The order is load-bearing too: it fixes each entry's index, and
/// Planter.swift's slot bookkeeping — usage counts, automatic assignment,
/// pruning — indexes into this table by that position.
struct PaletteColor {
    let name: String
    let hueDegrees: Int
    let textHex: String

    /// Sprites.swift's `palette(hue:)` takes a scalar 0...1 session hue, not
    /// degrees, so this is where the conversion happens.
    var hue: CGFloat { CGFloat(hueDegrees) / 360 }

    var text: NSColor { NSColor(paletteHex: textHex) }
}

let palette: [PaletteColor] = [
    PaletteColor(name: "red", hueDegrees: 0, textHex: "#ffa6a6"),
    PaletteColor(name: "orange", hueDegrees: 28, textHex: "#ffcfa6"),
    PaletteColor(name: "yellow", hueDegrees: 52, textHex: "#fff3a6"),
    PaletteColor(name: "lime", hueDegrees: 90, textHex: "#d2ffa6"),
    PaletteColor(name: "green", hueDegrees: 130, textHex: "#a6ffb5"),
    PaletteColor(name: "teal", hueDegrees: 165, textHex: "#a6ffe9"),
    PaletteColor(name: "cyan", hueDegrees: 190, textHex: "#a6f0ff"),
    PaletteColor(name: "blue", hueDegrees: 215, textHex: "#a6cbff"),
    PaletteColor(name: "indigo", hueDegrees: 250, textHex: "#b5a6ff"),
    PaletteColor(name: "purple", hueDegrees: 275, textHex: "#daa6ff"),
    PaletteColor(name: "magenta", hueDegrees: 310, textHex: "#ffa6f0"),
    PaletteColor(name: "pink", hueDegrees: 340, textHex: "#ffa6c4"),
]

/// Turns a persisted colour name (or PLANTER_COLOR) into a slot.
let paletteSlots: [String: Int] = Dictionary(
    uniqueKeysWithValues: palette.enumerated().map { ($1.name, $0) }
)

private extension NSColor {
    /// Parses a "#rrggbb" literal. Only ever fed this file's own hard-coded
    /// hexes, so a malformed one is a bug here, not bad input to guard against.
    convenience init(paletteHex hex: String) {
        let value = UInt32(hex.dropFirst(), radix: 16) ?? 0
        self.init(
            srgbRed: CGFloat((value >> 16) & 0xff) / 255,
            green: CGFloat((value >> 8) & 0xff) / 255,
            blue: CGFloat(value & 0xff) / 255,
            alpha: 1
        )
    }
}
