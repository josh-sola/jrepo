use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use miette::{IntoDiagnostic, miette};
use serde::Deserialize;

use crate::rex::CommandRunner;
use crate::update_check;

pub const VERSION: &str = match option_env!("VIZ_BUILD_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

pub const REPO: &str = "josh-sola/jrepo";
pub const TAG_PREFIX: &str = "viz-v";
const DEFAULT_GITHUB_API: &str = "https://api.github.com";
const DEFAULT_DOWNLOAD_BASE: &str = "https://github.com/josh-sola/jrepo/releases/download";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

pub fn parse_version(raw: &str) -> Option<ParsedVersion> {
    let trimmed = raw
        .trim()
        .strip_prefix(TAG_PREFIX)
        .or_else(|| raw.trim().strip_prefix('v'))
        .unwrap_or(raw.trim());
    let (core, pre) = match trimmed.split_once('-') {
        Some((c, p)) => (c, Some(p.to_string())),
        None => (trimmed, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ParsedVersion {
        major,
        minor,
        patch,
        pre,
    })
}

pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (Some(pa), Some(pb)) = (parse_version(a), parse_version(b)) else {
        return a.cmp(b);
    };
    pa.major
        .cmp(&pb.major)
        .then_with(|| pa.minor.cmp(&pb.minor))
        .then_with(|| pa.patch.cmp(&pb.patch))
        .then_with(|| match (&pa.pre, &pb.pre) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(la), Some(lb)) => la.cmp(lb),
        })
}

#[derive(Debug, Deserialize)]
struct ReleaseEntry {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub fn latest_version(releases_json: &str) -> Result<Option<String>, String> {
    let releases: Vec<ReleaseEntry> = serde_json::from_str(releases_json)
        .map_err(|e| format!("failed to parse GitHub releases JSON: {e}"))?;
    let mut best: Option<String> = None;
    for rel in releases {
        if rel.draft || rel.prerelease {
            continue;
        }
        let Some(ver) = rel.tag_name.strip_prefix(TAG_PREFIX) else {
            continue;
        };
        let Some(parsed) = parse_version(ver) else {
            continue;
        };
        if parsed.pre.is_some() {
            continue;
        }
        if best
            .as_deref()
            .is_none_or(|cur| compare_versions(ver, cur) == Ordering::Greater)
        {
            best = Some(ver.to_string());
        }
    }
    Ok(best)
}

pub fn checksum_for(sums: &str, filename: &str) -> Option<String> {
    for line in sums.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        let clean = name.strip_prefix('*').unwrap_or(name);
        if clean == filename {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

pub fn fetch_latest_version(
    runner: &dyn CommandRunner,
    env: &HashMap<String, String>,
) -> Result<String, String> {
    let api = env
        .get("VIZ_GITHUB_API")
        .filter(|s| !s.trim().is_empty())
        .map(String::as_str)
        .unwrap_or(DEFAULT_GITHUB_API);
    let url = format!(
        "{}/repos/{REPO}/releases?per_page=100",
        api.trim_end_matches('/')
    );

    let auth_header = env
        .get("GITHUB_TOKEN")
        .filter(|t| !t.trim().is_empty())
        .map(|t| format!("Authorization: Bearer {}", t.trim()));

    let mut args = vec!["-fsSL", "-H", "Accept: application/vnd.github+json"];
    if let Some(ref h) = auth_header {
        args.push("-H");
        args.push(h);
    }
    args.push(&url);

    let json = runner.run("curl", &args)?;
    latest_version(&json)?.ok_or_else(|| "no viz release is published yet".to_string())
}

pub fn run_upgrade(
    runner: &dyn CommandRunner,
    current: &str,
    check: bool,
    env: &HashMap<String, String>,
) -> miette::Result<String> {
    let latest = fetch_latest_version(runner, env).map_err(|e| miette!("{e}"))?;
    let _ = update_check::record_check(&latest, env);

    if check {
        return Ok(format!("Installed: {current}\nLatest: {latest}"));
    }
    if compare_versions(&latest, current) != Ordering::Greater {
        return Ok(format!("viz {current} is up to date."));
    }

    let base = env
        .get("VIZ_DOWNLOAD_BASE")
        .filter(|s| !s.trim().is_empty())
        .map(String::as_str)
        .unwrap_or(DEFAULT_DOWNLOAD_BASE);
    let release_url = format!("{}/{TAG_PREFIX}{latest}", base.trim_end_matches('/'));

    let tmp_dir = std::env::temp_dir().join(format!(
        "viz-upgrade-{}-{}",
        std::process::id(),
        update_check::now_unix_secs()
    ));
    std::fs::create_dir_all(&tmp_dir).into_diagnostic()?;
    let result = download_verify_and_install(runner, &tmp_dir, &release_url, &latest);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    result?;

    Ok(format!("Upgraded viz from {current} to {latest}."))
}

fn download_verify_and_install(
    runner: &dyn CommandRunner,
    tmp_dir: &Path,
    release_url: &str,
    latest: &str,
) -> miette::Result<()> {
    let script_path = tmp_dir.join("install.sh");
    let sums_path = tmp_dir.join("SHA256SUMS");
    let script_str = script_path.to_string_lossy().into_owned();
    let sums_str = sums_path.to_string_lossy().into_owned();

    let script_url = format!("{release_url}/install.sh");
    let sums_url = format!("{release_url}/SHA256SUMS");
    runner
        .run("curl", &["-fsSL", "-o", &script_str, &script_url])
        .map_err(|e| miette!("failed to download install.sh: {e}"))?;
    runner
        .run("curl", &["-fsSL", "-o", &sums_str, &sums_url])
        .map_err(|e| miette!("failed to download SHA256SUMS: {e}"))?;

    let sums_text = std::fs::read_to_string(&sums_path).into_diagnostic()?;
    let expected = checksum_for(&sums_text, "install.sh")
        .ok_or_else(|| miette!("SHA256SUMS has no entry for install.sh"))?;
    let actual = sha256_file(runner, &script_str).map_err(|e| miette!("{e}"))?;
    if actual != expected {
        return Err(miette!(
            "install.sh checksum mismatch: expected {expected}, got {actual}"
        ));
    }

    match runner.run(
        "gh",
        &["attestation", "verify", &script_str, "--repo", REPO],
    ) {
        Ok(_) => {}
        Err(err) if err.contains("No such file") || err.contains("not found") => {
            eprintln!("viz: gh not found; skipped build attestation check for install.sh.");
        }
        Err(err) => {
            return Err(miette!(
                "build attestation verification failed for install.sh: {err}"
            ));
        }
    }

    let status = Command::new("sh")
        .arg(&script_path)
        .env("VIZ_VERSION", latest)
        .status()
        .into_diagnostic()?;
    if !status.success() {
        return Err(miette!("install.sh exited with {status}"));
    }
    Ok(())
}

fn sha256_file(runner: &dyn CommandRunner, path: &str) -> Result<String, String> {
    let out = runner
        .run("sha256sum", &[path])
        .or_else(|_| runner.run("shasum", &["-a", "256", path]))?;
    out.split_whitespace()
        .next()
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| "failed to parse sha256 output".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockRunner {
        response: String,
        calls: RefCell<Vec<(String, Vec<String>)>>,
    }

    impl MockRunner {
        fn new(response: impl Into<String>) -> Self {
            Self {
                response: response.into(),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for MockRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
            self.calls.borrow_mut().push((
                program.to_string(),
                args.iter().map(|s| (*s).to_string()).collect(),
            ));
            Ok(self.response.clone())
        }
    }

    #[test]
    fn semver_parsing_and_comparison_orders_prereleases_before_releases() {
        assert_eq!(compare_versions("0.2.0", "0.1.9"), Ordering::Greater);
        assert_eq!(compare_versions("0.2.0-rc1", "0.2.0"), Ordering::Less);
        assert_eq!(compare_versions("viz-v1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.3", "1.2.4"), Ordering::Less);
        assert!(parse_version("not-a-version").is_none());
    }

    #[test]
    fn latest_version_filters_drafts_prereleases_and_unrelated_tags() {
        let json = r#"[
            {"tag_name": "cm-v9.9.9", "draft": false, "prerelease": false},
            {"tag_name": "viz-v0.3.0", "draft": true, "prerelease": false},
            {"tag_name": "viz-v0.4.0-rc1", "draft": false, "prerelease": true},
            {"tag_name": "viz-v0.1.2", "draft": false, "prerelease": false},
            {"tag_name": "viz-v0.2.0", "draft": false, "prerelease": false}
        ]"#;
        assert_eq!(latest_version(json).unwrap().as_deref(), Some("0.2.0"));
    }

    #[test]
    fn checksum_for_handles_sha256sum_and_shasum_formats() {
        let sums = "deadbeef  viz-darwin-arm64.gz\n0123456789ABCDEF *install.sh\n";
        assert_eq!(
            checksum_for(sums, "install.sh").as_deref(),
            Some("0123456789abcdef")
        );
        assert_eq!(
            checksum_for(sums, "viz-darwin-arm64.gz").as_deref(),
            Some("deadbeef")
        );
        assert_eq!(checksum_for(sums, "missing"), None);
    }

    #[test]
    fn run_upgrade_check_and_up_to_date_paths() {
        let releases = r#"[{"tag_name":"viz-v0.2.0","draft":false,"prerelease":false}]"#;
        let mock = MockRunner::new(releases);
        let dir = tempfile::tempdir().unwrap();
        let mut env = HashMap::new();
        env.insert("VIZ_DIR".into(), dir.path().display().to_string());

        let checked = run_upgrade(&mock, "0.1.0", true, &env).unwrap();
        assert_eq!(checked, "Installed: 0.1.0\nLatest: 0.2.0");

        let up_to_date = run_upgrade(&mock, "0.2.0", false, &env).unwrap();
        assert_eq!(up_to_date, "viz 0.2.0 is up to date.");
    }
}
