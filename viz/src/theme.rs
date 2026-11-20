use std::io::{IsTerminal, Read, Write};
use std::process::Command;

pub use viz_core::theme::*;

pub fn resolve_theme_mode(cli_mode: ThemeMode, env_var: Option<&str>) -> ThemeMode {
    if cli_mode != ThemeMode::Auto {
        return cli_mode;
    }
    if let Some(raw) = env_var
        && let Some(parsed) = ThemeMode::parse(raw)
        && parsed != ThemeMode::Auto
    {
        return parsed;
    }
    detect_terminal_theme()
}

pub fn detect_terminal_theme() -> ThemeMode {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return ThemeMode::Dark;
    }
    if let Some(mode) = query_osc11_theme() {
        return mode;
    }
    if let Ok(colorfgbg) = std::env::var("COLORFGBG")
        && let Some(mode) = parse_colorfgbg(&colorfgbg)
    {
        return mode;
    }
    ThemeMode::Dark
}

fn query_osc11_theme() -> Option<ThemeMode> {
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;

        let mut tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()?;

        // Use POSIX VMIN=0 VTIME=1 via stty so the read times out in <=100ms without
        // needing a libc dependency or leaving a blocked reader thread on /dev/tty.
        let saved = Command::new("stty")
            .arg("-g")
            .stdin(tty.try_clone().ok()?)
            .output()
            .ok()?;
        if !saved.status.success() {
            return None;
        }
        let saved_state = String::from_utf8(saved.stdout).ok()?;
        let saved_state = saved_state.trim().to_string();

        let configured = Command::new("stty")
            .args(["raw", "-echo", "min", "0", "time", "1"])
            .stdin(tty.try_clone().ok()?)
            .status()
            .ok()?;
        if !configured.success() {
            return None;
        }

        let response = (|| {
            tty.write_all(b"\x1b]11;?\x07").ok()?;
            tty.flush().ok()?;
            let mut buf = [0u8; 64];
            let mut acc = Vec::with_capacity(64);
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
            while std::time::Instant::now() < deadline && acc.len() < 64 {
                let n = tty.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                acc.extend_from_slice(&buf[..n]);
                if acc.contains(&0x07) || acc.windows(2).any(|w| w == b"\x1b\\") {
                    break;
                }
            }
            String::from_utf8(acc).ok()
        })();

        if let Ok(restore_tty) = tty.try_clone() {
            let _ = Command::new("stty")
                .arg(&saved_state)
                .stdin(restore_tty)
                .status();
        }

        response.as_deref().and_then(parse_osc11_response)
    }

    #[cfg(not(unix))]
    {
        None
    }
}

pub(crate) fn parse_osc11_response(resp: &str) -> Option<ThemeMode> {
    let idx = resp.find("rgb:")?;
    let payload = &resp[idx + 4..];
    let end = payload.find(['\x07', '\x1b']).unwrap_or(payload.len());
    let parts: Vec<&str> = payload[..end].split('/').collect();
    if parts.len() != 3 {
        return None;
    }

    let parse_chan = |hex: &str| -> Option<f64> {
        if hex.is_empty() || hex.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (hex.len() * 4)) - 1;
        Some(f64::from(v) / f64::from(max))
    };

    let r = parse_chan(parts[0])?;
    let g = parse_chan(parts[1])?;
    let b = parse_chan(parts[2])?;
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    Some(if luma > 0.5 {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    })
}

fn parse_colorfgbg(value: &str) -> Option<ThemeMode> {
    let bg: u8 = value.rsplit(';').next()?.trim().parse().ok()?;
    Some(match bg {
        7 | 15 => ThemeMode::Light,
        _ => ThemeMode::Dark,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc11_parser_classifies_light_and_dark() {
        assert_eq!(
            parse_osc11_response("\x1b]11;rgb:ffff/ffff/ffff\x07"),
            Some(ThemeMode::Light)
        );
        assert_eq!(
            parse_osc11_response("\x1b]11;rgb:0f0f/1111/1717\x1b\\"),
            Some(ThemeMode::Dark)
        );
        assert_eq!(parse_osc11_response("garbage"), None);
    }

    #[test]
    fn resolve_theme_mode_precedence() {
        assert_eq!(
            resolve_theme_mode(ThemeMode::Light, Some("dark")),
            ThemeMode::Light
        );
        assert_eq!(
            resolve_theme_mode(ThemeMode::Auto, Some("light")),
            ThemeMode::Light
        );
        assert_eq!(
            resolve_theme_mode(ThemeMode::Auto, Some("dark")),
            ThemeMode::Dark
        );
    }
}
