use std::process::Command;

use serde_json::{Value, json};

use crate::theme::ThemeMode;

pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String>;
}

pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| format!("failed to run `{program}`: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "`{program}` exited with {}: {}",
                output.status,
                stderr.trim()
            ));
        }
        String::from_utf8(output.stdout)
            .map_err(|e| format!("`{program}` produced non-UTF-8 output: {e}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    Right,
    Left,
    Above,
    Below,
}

impl SplitDir {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Left => "left",
            Self::Above => "above",
            Self::Below => "below",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "right" => Some(Self::Right),
            "left" => Some(Self::Left),
            "above" => Some(Self::Above),
            "below" => Some(Self::Below),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RexLayerResult {
    pub layer_id: Option<String>,
    pub block_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FloatingLayerRequest {
    pub rex_session: String,
    pub viz_id: String,
    pub exe: String,
    pub bounds: [f64; 4],
    pub focus: bool,
    pub theme: ThemeMode,
    pub window_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SplitRequest {
    pub rex_session: String,
    pub viz_id: String,
    pub exe: String,
    pub direction: SplitDir,
    pub title: String,
    pub focus: bool,
    pub theme: ThemeMode,
    pub window_id: Option<String>,
}

pub fn open_floating_layer(
    runner: &dyn CommandRunner,
    req: &FloatingLayerRequest,
) -> Result<RexLayerResult, String> {
    let mut block = json!({
        "flavor": "com.superlogical.terminal.shell",
        "options": {
            "shell": "none",
            "command": [
                req.exe,
                "view",
                "--follow",
                req.viz_id,
                "--theme",
                theme_mode_arg(req.theme),
            ],
        },
    });
    if let (Some(obj), Some(title)) = (block.as_object_mut(), &req.title) {
        obj.insert("label".into(), json!(title));
    }
    let mut payload = json!({
        "focus": req.focus,
        "bounds": req.bounds,
        "layout": {
            "block": block,
        },
    });
    if let (Some(obj), Some(window_id)) = (payload.as_object_mut(), &req.window_id) {
        obj.insert("window_id".into(), json!(window_id));
    }
    let payload_str = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let stdout = runner.run(
        "rex",
        &[
            "api",
            "call",
            "-s",
            &req.rex_session,
            "session.new_layer",
            &payload_str,
        ],
    )?;
    Ok(parse_layer_result(&stdout))
}

pub fn open_split(
    runner: &dyn CommandRunner,
    req: &SplitRequest,
) -> Result<RexLayerResult, String> {
    let split_arg = format!("--split={}", req.direction.as_str());
    let theme_arg = theme_mode_arg(req.theme);
    let mut args: Vec<&str> = vec![
        "run",
        "-s",
        &req.rex_session,
        &split_arg,
        "--label",
        &req.title,
        "--shell=none",
        "--json",
    ];
    if !req.focus {
        args.push("--focus=false");
    }
    if let Some(window_id) = req.window_id.as_deref() {
        args.push("--window");
        args.push(window_id);
    }
    args.extend([
        "--",
        &req.exe,
        "view",
        "--follow",
        &req.viz_id,
        "--theme",
        theme_arg,
    ]);

    let stdout = runner.run("rex", &args)?;
    Ok(parse_layer_result(&stdout))
}

pub fn close_layer(
    runner: &dyn CommandRunner,
    rex_session: &str,
    layer_id: &str,
) -> Result<(), String> {
    let payload =
        serde_json::to_string(&json!({ "layer_id": layer_id })).map_err(|e| e.to_string())?;
    runner.run(
        "rex",
        &[
            "api",
            "call",
            "-s",
            rex_session,
            "session.close_layer",
            &payload,
        ],
    )?;
    Ok(())
}

fn theme_mode_arg(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::Auto => "auto",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

fn parse_layer_result(stdout: &str) -> RexLayerResult {
    let Ok(val) = serde_json::from_str::<Value>(stdout) else {
        return RexLayerResult {
            layer_id: None,
            block_id: None,
        };
    };
    let root = val.get("result").unwrap_or(&val);
    let layer_id = root
        .get("layer_id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let block_id = root
        .get("block_ids")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(Value::as_str)
        .or_else(|| root.get("block_id").and_then(Value::as_str))
        .map(ToString::to_string);
    RexLayerResult { layer_id, block_id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockRunner {
        calls: RefCell<Vec<(String, Vec<String>)>>,
        response: String,
    }

    impl MockRunner {
        fn new(response: impl Into<String>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                response: response.into(),
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
    fn open_floating_layer_sends_expected_json_and_parses_ids() {
        let mock = MockRunner::new(r#"{"layer_id":"layer:1","block_ids":["block:9"]}"#);
        let res = open_floating_layer(
            &mock,
            &FloatingLayerRequest {
                rex_session: "session:abc".into(),
                viz_id: "amber-falcon".into(),
                exe: "/usr/local/bin/viz".into(),
                bounds: [0.48, 0.06, 0.48, 0.84],
                focus: false,
                theme: ThemeMode::Dark,
                window_id: None,
                title: Some("Trace".into()),
            },
        )
        .unwrap();

        assert_eq!(res.layer_id.as_deref(), Some("layer:1"));
        assert_eq!(res.block_id.as_deref(), Some("block:9"));

        let calls = mock.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "rex");
        assert_eq!(
            calls[0].1[..5],
            ["api", "call", "-s", "session:abc", "session.new_layer"]
        );
        let payload: Value = serde_json::from_str(&calls[0].1[5]).unwrap();
        assert_eq!(payload["focus"], json!(false));
        assert_eq!(payload["bounds"], json!([0.48, 0.06, 0.48, 0.84]));
        assert_eq!(
            payload["layout"]["block"]["flavor"],
            json!("com.superlogical.terminal.shell")
        );
        assert_eq!(payload["layout"]["block"]["label"], json!("Trace"));
        assert_eq!(
            payload["layout"]["block"]["options"]["shell"],
            json!("none")
        );
        assert_eq!(
            payload["layout"]["block"]["options"]["command"],
            json!([
                "/usr/local/bin/viz",
                "view",
                "--follow",
                "amber-falcon",
                "--theme",
                "dark"
            ])
        );
    }

    #[test]
    fn open_floating_layer_honors_focus_and_custom_bounds() {
        let mock = MockRunner::new(r#"{"result":{"layer_id":"layer:2","block_ids":["block:10"]}}"#);
        let res = open_floating_layer(
            &mock,
            &FloatingLayerRequest {
                rex_session: "session:xyz".into(),
                viz_id: "cobalt-prism".into(),
                exe: "viz".into(),
                bounds: [0.1, 0.2, 0.7, 0.6],
                focus: true,
                theme: ThemeMode::Auto,
                window_id: Some("win:1".into()),
                title: None,
            },
        )
        .unwrap();

        assert_eq!(res.layer_id.as_deref(), Some("layer:2"));
        assert_eq!(res.block_id.as_deref(), Some("block:10"));
        let calls = mock.calls.borrow();
        let payload: Value = serde_json::from_str(&calls[0].1[5]).unwrap();
        assert_eq!(payload["focus"], json!(true));
        assert_eq!(payload["bounds"], json!([0.1, 0.2, 0.7, 0.6]));
        assert_eq!(payload["window_id"], json!("win:1"));
    }

    #[test]
    fn open_split_and_close_layer_build_expected_commands() {
        let mock = MockRunner::new(r#"{"block_id":"block:42"}"#);
        let res = open_split(
            &mock,
            &SplitRequest {
                rex_session: "session:abc".into(),
                viz_id: "jade-ridge".into(),
                exe: "/bin/viz".into(),
                direction: SplitDir::Right,
                title: "Live".into(),
                focus: false,
                theme: ThemeMode::Light,
                window_id: None,
            },
        )
        .unwrap();
        assert_eq!(res.block_id.as_deref(), Some("block:42"));

        close_layer(&mock, "session:abc", "layer:1").unwrap();

        let calls = mock.calls.borrow();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].1,
            vec![
                "run",
                "-s",
                "session:abc",
                "--split=right",
                "--label",
                "Live",
                "--shell=none",
                "--json",
                "--focus=false",
                "--",
                "/bin/viz",
                "view",
                "--follow",
                "jade-ridge",
                "--theme",
                "light",
            ]
        );
        assert_eq!(
            calls[1].1[..5],
            ["api", "call", "-s", "session:abc", "session.close_layer"]
        );
        let close_payload: Value = serde_json::from_str(&calls[1].1[5]).unwrap();
        assert_eq!(close_payload, json!({"layer_id": "layer:1"}));
    }
}
