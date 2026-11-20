use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

fn viz_bin() -> &'static str {
    env!("CARGO_BIN_EXE_viz")
}

#[test]
fn cli_full_session_lifecycle_and_check_and_guide() {
    let tmp = TempDir::new().unwrap();
    let viz_dir = tmp.path();

    let guide = Command::new(viz_bin())
        .env("VIZ_NO_UPDATE_CHECK", "1")
        .arg("guide")
        .output()
        .unwrap();
    assert!(guide.status.success());
    let guide_str = String::from_utf8(guide.stdout).unwrap();
    assert!(guide_str.contains("# viz"));
    assert!(guide_str.contains("var(--bg)"));

    let check_ok = Command::new(viz_bin())
        .env("VIZ_NO_UPDATE_CHECK", "1")
        .args(["check", "examples/walkthrough.viz.kdl"])
        .output()
        .unwrap();
    assert!(check_ok.status.success());
    assert!(String::from_utf8(check_ok.stdout).unwrap().contains("OK:"));

    let bad_path = viz_dir.join("bad.viz.kdl");
    std::fs::write(&bad_path, "viz v=2 title=\"Bad\" { markdown \"x\" }").unwrap();
    let check_bad = Command::new(viz_bin())
        .env("VIZ_NO_UPDATE_CHECK", "1")
        .arg("check")
        .arg(&bad_path)
        .output()
        .unwrap();
    assert!(!check_bad.status.success());

    let open = Command::new(viz_bin())
        .env("VIZ_DIR", viz_dir)
        .env("VIZ_NO_UPDATE_CHECK", "1")
        .args(["open", "--no-rex", "examples/live-stack.viz.kdl"])
        .output()
        .unwrap();
    assert!(
        open.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&open.stderr)
    );
    let session_id = String::from_utf8(open.stdout).unwrap().trim().to_string();
    assert!(session_id.contains('-'));

    let patch_flags = Command::new(viz_bin())
        .env("VIZ_DIR", viz_dir)
        .args([
            "patch",
            &session_id,
            "--node-status",
            "mesh:api=error",
            "--stat",
            "kpis:rps=9.9k/s",
            "--set-state",
            "selected=stable",
        ])
        .output()
        .unwrap();
    assert!(
        patch_flags.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&patch_flags.stderr)
    );

    let mut patch_stdin = Command::new(viz_bin())
        .env("VIZ_DIR", viz_dir)
        .args(["patch", &session_id])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    patch_stdin
        .stdin
        .as_mut()
        .unwrap()
        .write_all(
            b"chart-append target=\"latency\" series=\"p99\" x-label=\"12:05\" max-points=5 { point 44.0 }\n",
        )
        .unwrap();
    let patch_stdin_out = patch_stdin.wait_with_output().unwrap();
    assert!(
        patch_stdin_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&patch_stdin_out.stderr)
    );

    let log_cmd = Command::new(viz_bin())
        .env("VIZ_DIR", viz_dir)
        .args([
            "log",
            &session_id,
            "events",
            "--level",
            "error",
            "--source",
            "canary",
            "health",
            "check",
            "failed",
        ])
        .output()
        .unwrap();
    assert!(
        log_cmd.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&log_cmd.stderr)
    );

    let export_kdl = Command::new(viz_bin())
        .env("VIZ_DIR", viz_dir)
        .args(["export", &session_id])
        .output()
        .unwrap();
    assert!(export_kdl.status.success());
    let kdl_out = String::from_utf8(export_kdl.stdout).unwrap();
    assert!(kdl_out.contains("9.9k/s"));
    assert!(kdl_out.contains("health check failed"));

    let export_json = Command::new(viz_bin())
        .env("VIZ_DIR", viz_dir)
        .args(["export", &session_id, "--json"])
        .output()
        .unwrap();
    assert!(export_json.status.success());
    let json_doc: Value = serde_json::from_slice(&export_json.stdout).unwrap();
    assert_eq!(json_doc["state"]["selected"], "stable");

    let ls_json = Command::new(viz_bin())
        .env("VIZ_DIR", viz_dir)
        .args(["ls", "--json"])
        .output()
        .unwrap();
    assert!(ls_json.status.success());
    let sessions: Vec<Value> = serde_json::from_slice(&ls_json.stdout).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], session_id);

    let close = Command::new(viz_bin())
        .env("VIZ_DIR", viz_dir)
        .args(["close", &session_id])
        .output()
        .unwrap();
    assert!(close.status.success());
}

#[test]
fn cli_render_png_chart_from_and_version() {
    let tmp = TempDir::new().unwrap();

    let version = Command::new(viz_bin())
        .env("VIZ_NO_UPDATE_CHECK", "1")
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert!(
        String::from_utf8(version.stdout)
            .unwrap()
            .starts_with("viz ")
    );

    let png_path = tmp.path().join("walkthrough.png");
    let render = Command::new(viz_bin())
        .env("VIZ_NO_UPDATE_CHECK", "1")
        .args([
            "render",
            "examples/walkthrough.viz.kdl",
            "--png",
            png_path.to_str().unwrap(),
            "--width",
            "800",
            "--height",
            "480",
            "--theme",
            "light",
            "--step",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        render.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&render.stderr)
    );
    let png_bytes = std::fs::read(&png_path).unwrap();
    assert!(png_bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]));
    assert!(png_bytes.len() > 1000);

    let csv_path = tmp.path().join("bench.csv");
    std::fs::write(
        &csv_path,
        "case,before,after\ncold,40.0,12.5\nwarm,10.0,2.5\n",
    )
    .unwrap();
    let chart_from = Command::new(viz_bin())
        .env("VIZ_NO_UPDATE_CHECK", "1")
        .args([
            "chart-from",
            csv_path.to_str().unwrap(),
            "--type",
            "hbar",
            "--id",
            "imported",
            "--unit",
            "seconds",
            "--scale",
            "bench",
            "--value-labels",
        ])
        .output()
        .unwrap();
    assert!(
        chart_from.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&chart_from.stderr)
    );
    let kdl = String::from_utf8(chart_from.stdout).unwrap();
    assert!(kdl.contains("chart id=\"imported\" type=\"hbar\""));
    assert!(kdl.contains("2 rows from bench.csv"));
}
