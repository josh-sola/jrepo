use std::io::{IsTerminal, Write};
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
/// escape bytes. Inside tmux the background belongs to the outer terminal and
/// is shared by every window, so a plain write from the last-launched window
/// would win over all the others; instead the hex is stored on the current
/// window and hooks re-apply the active window's tint on every switch.
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

/// tmux window option carrying a window's tint. Windows wt never launched in
/// don't have it, which is how `retint_clients` knows to reset instead.
const TMUX_BG_OPTION: &str = "@wt_bg";

/// tmux hooks are arrays; wt claims one fixed high index so a user's own
/// hooks at other indexes survive, and re-setting is idempotent.
const TMUX_HOOK_INDEX: u32 = 97;

/// Every event after which the outer terminal may be showing a different
/// window than the one that last set the background.
const TMUX_HOOKS: [&str; 3] = [
    "session-window-changed",
    "client-session-changed",
    "client-attached",
];

fn set_tmux_window_background(hex: &str) {
    let _ = Command::new("tmux")
        .args(["set-option", "-w", TMUX_BG_OPTION, hex])
        .status();
    if let Ok(exe) = std::env::current_exe() {
        let cmd = format!("run-shell -b '{} __retint'", exe.display());
        for hook in TMUX_HOOKS {
            let _ = Command::new("tmux")
                .args([
                    "set-hook",
                    "-g",
                    &format!("{hook}[{TMUX_HOOK_INDEX}]"),
                    &cmd,
                ])
                .status();
        }
    }
    retint_clients();
}

/// Re-applies each tmux client's active-window tint by writing directly to
/// the client tty. The direct write bypasses tmux entirely, so it needs no
/// passthrough envelope (or `allow-passthrough on`), and the terminal keeps
/// treating the color as its *default* background — tmux consuming OSC 11
/// would instead repaint pane cells with an explicit background, which
/// terminals draw opaque, hiding shaders and transparency. Best-effort
/// throughout: a dead client or missing tmux is ignored.
pub fn retint_clients() {
    let Ok(out) = Command::new("tmux")
        .args(["list-clients", "-F", "#{client_tty}\t#{@wt_bg}"])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((tty, hex)) = line.split_once('\t') else {
            continue;
        };
        let Some(payload) = retint_payload(hex) else {
            continue;
        };
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(tty) {
            let _ = f.write_all(payload.as_bytes());
        }
    }
}

/// OSC 111 resets the background to the terminal's own default, so switching
/// to an untinted window clears the previous window's tint. The hex came back
/// through tmux, so anything not shaped like a color is dropped rather than
/// written to a tty.
fn retint_payload(hex: &str) -> Option<String> {
    if hex.is_empty() {
        Some("\x1b]111\x1b\\".to_string())
    } else if is_hex_color(hex) {
        Some(osc11(hex))
    } else {
        None
    }
}

fn is_hex_color(s: &str) -> bool {
    s.strip_prefix('#')
        .is_some_and(|d| d.len() == 6 && d.chars().all(|c| c.is_ascii_hexdigit()))
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
    fn retint_payload_sets_a_valid_hex() {
        assert_eq!(
            retint_payload("#170a0a").as_deref(),
            Some("\x1b]11;#170a0a\x1b\\")
        );
    }

    #[test]
    fn retint_payload_resets_an_untinted_window() {
        assert_eq!(retint_payload("").as_deref(), Some("\x1b]111\x1b\\"));
    }

    #[test]
    fn retint_payload_drops_a_malformed_value() {
        assert_eq!(retint_payload("#120c1"), None);
        assert_eq!(retint_payload("120c1a"), None);
        assert_eq!(retint_payload("#120c1g"), None);
        assert_eq!(retint_payload("evil\x07"), None);
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
