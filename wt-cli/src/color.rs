use std::io::{IsTerminal, Write};
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
    fn retint_payload_sets_a_valid_hex() {
        assert_eq!(
            retint_payload("#120c1a").as_deref(),
            Some("\x1b]11;#120c1a\x1b\\")
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
