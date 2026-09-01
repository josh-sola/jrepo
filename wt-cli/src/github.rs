//! GitHub lookups that go through `gh` rather than a local Graphite
//! sidecar — the fallback `wt go '#<PR>'` uses when `.graphite_pr_info`
//! doesn't know about a PR yet (opened by someone else, or before this
//! clone's last `gt submit`).

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// A pull request's head branch and state, as `gh` reports them. `state` is
/// case-inconsistent across `gh` versions (`OPEN`/`open`), so callers
/// compare it case-insensitively.
pub struct PrHead {
    pub branch: String,
    pub state: String,
}

#[derive(Deserialize)]
struct PrView {
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    state: String,
}

/// Runs `gh pr view <number>` with `cwd` set to the repo base so `gh`
/// infers the GitHub repository from its remotes.
pub fn pr_head(base: &Path, number: u64) -> Result<PrHead> {
    let out = Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--json",
            "headRefName,state",
        ])
        .current_dir(base)
        .output()
        .with_context(|| {
            format!("running `gh pr view {number}`; is `gh` installed? check `gh auth status`")
        })?;
    if !out.status.success() {
        bail!(
            "looking up PR #{number} with `gh pr view` failed: {}; check `gh auth status`",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let view: PrView = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing `gh pr view {number}` output"))?;
    Ok(PrHead {
        branch: view.head_ref_name,
        state: view.state,
    })
}
