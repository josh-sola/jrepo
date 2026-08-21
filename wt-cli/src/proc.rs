use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::store;

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

/// A `provisioning` row whose recorded pid is no longer alive is wedged, not
/// progressing — a killed child leaves it in `provisioning` forever
/// otherwise. `None` (not yet recorded, or an older registry entry) is not
/// flagged: there is nothing to compare against. Shared by `wt tree status`/
/// `wt tree ls` and the hot-spare reaper, which both need to tell a wedged row from
/// one that's genuinely still working.
pub fn provisioning_is_stale(t: &store::Tree) -> bool {
    t.state == store::TreeState::Provisioning && t.provision_pid.is_some_and(|pid| !pid_alive(pid))
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

/// Re-execs the current binary as `wt <args...>`, detached from the caller:
/// its own process group so a Ctrl-C at the spawning terminal doesn't also
/// signal it, and null stdio so it can outlive the caller with nothing to
/// write into or read from. `WT_ROOT` and `WT_CONFIG` are carried across so
/// the child reads the same registry and config file.
pub fn spawn_detached(root: &Path, config_path: &Path, args: &[&str]) -> Result<u32> {
    let exe = std::env::current_exe().context("resolving current executable")?;
    // Under a unit-test harness `current_exe` is the test binary, not the
    // CLI, and it reads these arguments as test-name filters — so a
    // subcommand like `__spare new <repo>` re-runs every test matching
    // "new", each of which spawns again. Only ever re-exec `wt` itself.
    if exe.file_name().and_then(|n| n.to_str()) != Some("wt") {
        bail!(
            "refusing to re-exec {} as wt: background work is only spawned from the real binary",
            exe.display()
        );
    }
    let child = Command::new(exe)
        .args(args)
        .env("WT_ROOT", root)
        .env("WT_CONFIG", config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .with_context(|| format!("spawning detached `wt {}`", args.join(" ")))?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `current_exe` inside a unit test is the test harness, which is
    /// precisely the case the guard in `spawn_detached` exists to catch, so
    /// this asserts the refusal without having to fake anything.
    ///
    /// The arguments deliberately match no test name. Should the guard ever
    /// be removed, this spawns a harness that runs nothing and exits —
    /// rather than one that re-runs a matching test and spawns again.
    #[test]
    fn spawn_detached_refuses_anything_but_the_real_binary() {
        let err = spawn_detached(
            Path::new("/tmp"),
            Path::new("/tmp/config.kdl"),
            &["__provision", "00000000-0000-0000-0000-000000000000"],
        )
        .expect_err("spawning from a test harness must fail");
        assert!(
            err.to_string().contains("refusing to re-exec"),
            "unexpected error: {err:#}"
        );
    }
}
