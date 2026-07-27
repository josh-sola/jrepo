use std::process::{Command, Stdio};
use std::time::Duration;

use uuid::Uuid;

/// Best-effort identity check for a recorded provisioning pid: reads the
/// process's command line and looks for both the subcommand name and the
/// tree id, so a reused pid resolving to an unrelated process is never
/// mistaken for ours. No match — process gone, or a genuinely different
/// command — is treated the same as "already gone".
pub fn is_our_provision_child(pid: u32, tree_id: Uuid) -> bool {
    let Ok(out) = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let cmdline = String::from_utf8_lossy(&out.stdout);
    cmdline.contains("__provision") && cmdline.contains(&tree_id.to_string())
}

pub fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Spawned with `process_group(0)`, so the provisioning child is its own
/// group leader; signalling the negative pid reaches everything it spawned
/// too (e.g. a still-running `pnpm install`), not just that one process.
fn signal_group(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([signal, &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Stops a tree's background provisioning before its directory is deleted
/// out from under it. A pid that no longer identifies our child — already
/// exited, or reused by something unrelated — is left alone; there is
/// nothing of ours left to signal.
pub fn stop_provisioning_child(pid: Option<u32>, tree_id: Uuid) {
    let Some(pid) = pid else { return };
    if !is_our_provision_child(pid, tree_id) {
        return;
    }
    signal_group(pid, "-TERM");
    for _ in 0..20 {
        if !pid_alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    signal_group(pid, "-KILL");
}
