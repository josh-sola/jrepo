use std::io::IsTerminal;
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    pub name: &'static str,
    pub primary: &'static str,
    pub text: &'static str,
    pub tint: &'static str,
}

/// Mirrors the canonical `/palette.json` at the repo root; `tools/check-palette`
/// checks this copy against it. `tint` is a near-black background tuned so no
/// color reads brighter than its neighbours, so a tab is tinted rather than
/// coloured. Order is load-bearing: `pick` indexes into it by hash.
///
/// `rustfmt::skip` keeps each entry on one line — `tools/check-palette`
/// regex-matches this table line by line against `palette.json`.
#[rustfmt::skip]
pub const PALETTE: [PaletteEntry; 12] = [
    PaletteEntry { name: "red", primary: "#eb5959", text: "#ffa6a6", tint: "#170a0a" },
    PaletteEntry { name: "orange", primary: "#eb9d59", text: "#ffcfa6", tint: "#17100a" },
    PaletteEntry { name: "yellow", primary: "#ebd759", text: "#fff3a6", tint: "#17150a" },
    PaletteEntry { name: "lime", primary: "#a2eb59", text: "#d2ffa6", tint: "#11170a" },
    PaletteEntry { name: "green", primary: "#59eb71", text: "#a6ffb5", tint: "#0a170c" },
    PaletteEntry { name: "teal", primary: "#59ebc6", text: "#a6ffe9", tint: "#0a1714" },
    PaletteEntry { name: "cyan", primary: "#59d2eb", text: "#a6f0ff", tint: "#0a1517" },
    PaletteEntry { name: "blue", primary: "#5996eb", text: "#a6cbff", tint: "#0a1017" },
    PaletteEntry { name: "indigo", primary: "#7159eb", text: "#b5a6ff", tint: "#0c0a17" },
    PaletteEntry { name: "purple", primary: "#ae59eb", text: "#daa6ff", tint: "#120a17" },
    PaletteEntry { name: "magenta", primary: "#eb59d2", text: "#ffa6f0", tint: "#170a15" },
    PaletteEntry { name: "pink", primary: "#eb598a", text: "#ffa6c4", tint: "#170a0f" },
];

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// `std`'s `DefaultHasher` is explicitly unstable across releases, which
/// would move a tree's color out from under the user on a toolchain bump.
fn fnv1a64(s: &str) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in s.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

pub fn pick(repo: &str, name: &str) -> &'static PaletteEntry {
    let key = format!("{repo}/{}", crate::tree::slugify(name));
    let hash = fnv1a64(&key);
    &PALETTE[(hash % PALETTE.len() as u64) as usize]
}

/// Outside tmux one OSC 11 write to stderr is enough: it sticks for the rest
/// of the terminal session. Guarded on a tty so a redirected run gets no
/// escape bytes. Inside tmux the tint goes on the window instead: OSC 11
/// would recolor the whole outer terminal for every window at once.
pub fn set_background(hex: &str) {
    if std::env::var_os("TMUX").is_some() {
        set_tmux_window_background(hex);
    } else if std::io::stderr().is_terminal() {
        eprint!("{}", osc11(hex));
    }
}

fn osc11(hex: &str) -> String {
    format!("\x1b]11;{hex}\x1b\\")
}

/// `window-style` is a window-scoped tmux option, so tmux itself keeps the
/// tint right per window and per client, across splits, session switches,
/// and reattach — nothing to hook and nothing to reset. tmux paints the
/// cells with an explicit background rather than the terminal's default,
/// which the Ghostty shader can't key off `iBackgroundColor`; the shader
/// keys each PALETTE tint per pixel instead (see balatro_bg.glsl, held in
/// sync by `shader_keys_every_palette_tint`).
fn set_tmux_window_background(hex: &str) {
    let _ = Command::new("tmux")
        .args(["set-option", "-w", "window-style", &format!("bg={hex}")])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn pick_is_deterministic() {
        assert_eq!(pick("monorepo", "fix login"), pick("monorepo", "fix login"));
    }

    #[test]
    fn pick_ignores_slug_equivalent_spelling() {
        assert_eq!(pick("monorepo", "fix login"), pick("monorepo", "fix-login"));
    }

    #[test]
    fn pick_reaches_every_palette_entry() {
        let colors: HashSet<&str> = (0..500)
            .map(|i| pick("monorepo", &format!("t{i}")).name)
            .collect();
        assert_eq!(colors.len(), 12, "expected all 12 colors, got {colors:?}");
    }

    #[test]
    fn osc11_sets_the_background() {
        assert_eq!(osc11("#170a0a"), "\x1b]11;#170a0a\x1b\\");
    }

    #[test]
    fn shader_keys_every_palette_tint() {
        let glsl = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ghostty/shaders/balatro_bg.glsl"
        ))
        .expect("balatro_bg.glsl should sit next to wt-cli in the repo");
        for entry in &PALETTE {
            assert!(
                glsl.contains(entry.tint),
                "{}'s tint {} is missing from balatro_bg.glsl's \
                 WT_TINTS; the shader keys every palette tint per pixel",
                entry.name,
                entry.tint
            );
        }
    }

    #[test]
    fn every_palette_hex_is_six_hex_digits() {
        for entry in &PALETTE {
            for hex in [entry.primary, entry.text, entry.tint] {
                let digits = hex.strip_prefix('#').unwrap_or_else(|| {
                    panic!("{}'s hex '{hex}' is missing a leading '#'", entry.name);
                });
                assert_eq!(
                    digits.len(),
                    6,
                    "{}'s hex '{hex}' is not 6 digits",
                    entry.name
                );
                assert!(
                    digits.chars().all(|c| c.is_ascii_hexdigit()),
                    "{}'s hex '{hex}' has a non-hex digit",
                    entry.name
                );
            }
        }
    }

    /// ghostty/shaders/balatro_bg.glsl stops tinting its swirl below this HSV
    /// saturation, so every tint must stay above it.
    #[test]
    fn every_tint_clears_the_balatro_shader_saturation_floor() {
        for entry in &PALETTE {
            let digits = entry.tint.strip_prefix('#').unwrap();
            let bytes: Vec<u8> = (0..3)
                .map(|i| u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).unwrap())
                .collect();
            let (max, min) = (
                *bytes.iter().max().unwrap() as f64,
                *bytes.iter().min().unwrap() as f64,
            );
            let saturation = if max == 0.0 { 0.0 } else { (max - min) / max };
            assert!(
                saturation >= 0.15,
                "{}'s tint '{}' has saturation {saturation:.2}, below the shader floor",
                entry.name,
                entry.tint
            );
        }
    }
}
