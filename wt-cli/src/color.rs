use std::io::IsTerminal;
use std::process::Command;

/// Claude Code's `/color` palette, each paired with a near-black background
/// hex tuned to match. They are all one uniform step off a lighter set, which
/// is what keeps them even: no colour reads brighter than its neighbours, so
/// a tab is tinted rather than coloured. Darken or lighten them together.
/// Order is load-bearing: `pick` indexes into it by hash.
pub const PALETTE: [(&str, &str); 8] = [
    ("red", "#170b0c"),
    ("blue", "#090f19"),
    ("green", "#0a140e"),
    ("yellow", "#151209"),
    ("purple", "#120c1a"),
    ("orange", "#180e08"),
    ("pink", "#180b12"),
    ("cyan", "#081415"),
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

pub fn pick(repo: &str, name: &str) -> (&'static str, &'static str) {
    let key = format!("{repo}/{}", crate::tree::slugify(name));
    let hash = fnv1a64(&key);
    PALETTE[(hash % PALETTE.len() as u64) as usize]
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
        let colors: HashSet<&str> = (0..200)
            .map(|i| pick("monorepo", &format!("t{i}")).0)
            .collect();
        assert_eq!(colors.len(), 8, "expected all 8 colors, got {colors:?}");
    }

    #[test]
    fn osc11_sets_the_background() {
        assert_eq!(osc11("#120c1a"), "\x1b]11;#120c1a\x1b\\");
    }

    #[test]
    fn shader_keys_every_palette_tint() {
        let glsl = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ghostty/shaders/balatro_bg.glsl"
        ))
        .expect("balatro_bg.glsl should sit next to wt-cli in the repo");
        for (name, hex) in PALETTE {
            assert!(
                glsl.contains(hex),
                "{name}'s tint {hex} is missing from balatro_bg.glsl's \
                 WT_TINTS; the shader keys every palette tint per pixel"
            );
        }
    }

    #[test]
    fn every_palette_hex_is_six_hex_digits() {
        for (name, hex) in PALETTE {
            let digits = hex.strip_prefix('#').unwrap_or_else(|| {
                panic!("{name}'s hex '{hex}' is missing a leading '#'");
            });
            assert_eq!(digits.len(), 6, "{name}'s hex '{hex}' is not 6 digits");
            assert!(
                digits.chars().all(|c| c.is_ascii_hexdigit()),
                "{name}'s hex '{hex}' has a non-hex digit"
            );
        }
    }
}
