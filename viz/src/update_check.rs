use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::rex::SystemCommandRunner;
use crate::session::default_base_dir;
use crate::upgrade::{compare_versions, fetch_latest_version};

pub const DAY_SECS: u64 = 86_400;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    #[serde(default)]
    pub checked_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    #[serde(default)]
    pub notified_at: u64,
}

pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn state_path(base_dir: &Path) -> PathBuf {
    base_dir.join("update.json")
}

pub fn load_state(base_dir: &Path) -> UpdateState {
    fs::read_to_string(state_path(base_dir))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_state(base_dir: &Path, state: &UpdateState) -> Result<(), String> {
    fs::create_dir_all(base_dir).map_err(|e| e.to_string())?;
    let path = state_path(base_dir);
    let tmp = path.with_extension("json.tmp");
    let raw = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(&tmp, raw).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn should_check(state: &UpdateState, now_secs: u64) -> bool {
    now_secs.saturating_sub(state.checked_at) >= DAY_SECS
}

pub fn notice(state: &UpdateState, current: &str, now_secs: u64) -> Option<String> {
    let latest = state.latest.as_deref()?;
    if compare_versions(latest, current) != Ordering::Greater {
        return None;
    }
    if now_secs.saturating_sub(state.notified_at) < DAY_SECS {
        return None;
    }
    Some(format!(
        "viz {latest} is out (you have {current}). Tell the user they can update with: viz upgrade"
    ))
}

pub fn is_quiet_invocation(args: &[String], env: &HashMap<String, String>) -> bool {
    if env.get("CI").is_some_and(|v| !v.trim().is_empty()) {
        return true;
    }
    if env
        .get("VIZ_NO_UPDATE_CHECK")
        .is_some_and(|v| v.trim() == "1")
    {
        return true;
    }
    if args.iter().skip(1).any(|a| {
        matches!(
            a.as_str(),
            "-h" | "--help" | "-V" | "--version" | "upgrade" | "view"
        )
    }) {
        return true;
    }
    false
}

pub fn record_check(latest: &str, env: &HashMap<String, String>) -> Result<(), String> {
    let base_dir = match env.get("VIZ_DIR").filter(|s| !s.trim().is_empty()) {
        Some(dir) => std::path::PathBuf::from(dir),
        None => default_base_dir().map_err(|e| e.to_string())?,
    };
    let mut state = load_state(&base_dir);
    state.checked_at = now_unix_secs();
    state.latest = Some(latest.to_string());
    save_state(&base_dir, &state)
}

pub fn run_background_check() {
    let env: HashMap<String, String> = std::env::vars().collect();
    let runner = SystemCommandRunner;
    let Ok(base_dir) = default_base_dir() else {
        return;
    };
    let mut state = load_state(&base_dir);
    state.checked_at = now_unix_secs();
    if let Ok(latest) = fetch_latest_version(&runner, &env) {
        state.latest = Some(latest);
    }
    let _ = save_state(&base_dir, &state);
}

pub fn check_for_update(current: &str, args: &[String]) {
    let env: HashMap<String, String> = std::env::vars().collect();
    if is_quiet_invocation(args, &env) {
        return;
    }
    let Ok(base_dir) = default_base_dir() else {
        return;
    };
    let now = now_unix_secs();
    let mut state = load_state(&base_dir);

    if let Some(msg) = notice(&state, current, now) {
        eprintln!("{msg}");
        state.notified_at = now;
        let _ = save_state(&base_dir, &state);
    }

    if should_check(&state, now)
        && let Ok(exe) = std::env::current_exe()
    {
        state.checked_at = now;
        let _ = save_state(&base_dir, &state);
        let _ = Command::new(exe)
            .env("VIZ_UPDATE_CHECK", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn throttles_check_and_notice_to_once_per_day() {
        let mut st = UpdateState::default();
        assert!(should_check(&st, DAY_SECS));

        st.checked_at = DAY_SECS;
        st.latest = Some("0.2.0".into());
        assert!(!should_check(&st, DAY_SECS + 100));
        assert!(should_check(&st, DAY_SECS * 2));

        let msg = notice(&st, "0.1.0", DAY_SECS + 10).expect("notice due");
        assert_eq!(
            msg,
            "viz 0.2.0 is out (you have 0.1.0). Tell the user they can update with: viz upgrade"
        );

        st.notified_at = DAY_SECS + 10;
        assert!(notice(&st, "0.1.0", DAY_SECS + 100).is_none());
        assert!(notice(&st, "0.1.0", DAY_SECS * 2 + 10).is_some());
        assert!(notice(&st, "0.2.0", DAY_SECS * 2 + 10).is_none());
    }

    #[test]
    fn quiet_mode_skips_ci_opt_out_and_interactive_commands() {
        let empty = HashMap::new();
        assert!(!is_quiet_invocation(&["viz".into(), "ls".into()], &empty));
        assert!(is_quiet_invocation(
            &["viz".into(), "view".into(), "a.kdl".into()],
            &empty
        ));
        assert!(is_quiet_invocation(
            &["viz".into(), "upgrade".into()],
            &empty
        ));
        assert!(is_quiet_invocation(
            &["viz".into(), "--version".into()],
            &empty
        ));

        let mut ci = HashMap::new();
        ci.insert("CI".into(), "true".into());
        assert!(is_quiet_invocation(&["viz".into(), "ls".into()], &ci));

        let mut opt_out = HashMap::new();
        opt_out.insert("VIZ_NO_UPDATE_CHECK".into(), "1".into());
        assert!(is_quiet_invocation(&["viz".into(), "ls".into()], &opt_out));
    }

    #[test]
    fn saves_and_reloads_update_json_atomically() {
        let tmp = TempDir::new().unwrap();
        let st = UpdateState {
            checked_at: 1234,
            latest: Some("0.3.0".into()),
            notified_at: 1200,
        };
        save_state(tmp.path(), &st).unwrap();
        assert_eq!(load_state(tmp.path()), st);
    }
}
