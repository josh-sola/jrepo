use std::path::{Path, PathBuf};

use miette::{Context, IntoDiagnostic, miette};
use serde_json::{Value, json};

use crate::kdl_codec::parse_document;
use crate::model::{Event, Frame, VizShare};
use crate::rex::CommandRunner;
use crate::session::{events_path, load_meta, load_store, read_timed_frames};
use crate::store::{inline_event_code, inline_external_code};

pub const DEFAULT_PLANHUB_URL: &str = "https://planhub-sola-internal-toy.tail67f40b.ts.net";

/// Planhub rejects bodies over 5 MB, so `viz share` checks before uploading.
pub const MAX_SHARE_BYTES: usize = 5_000_000;

pub struct ShareRequest<'a> {
    pub title: Option<&'a str>,
    pub private: bool,
    pub host: Option<&'a str>,
}

/// A file share: the parsed document with code inlined from beside the file,
/// and no history.
pub fn share_from_file(path: &Path) -> miette::Result<VizShare> {
    let src = std::fs::read_to_string(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read {}", path.display()))?;
    let mut doc = parse_document(&src, &path.display().to_string())?;
    let abs = std::path::absolute(path).into_diagnostic()?;
    inline_external_code(&mut doc, abs.parent());
    Ok(VizShare {
        v: 1,
        doc,
        frames: Vec::new(),
    })
}

/// A session share: the initial document plus every later frame, or with
/// `no_history` just the current document.
pub fn share_from_session(base_dir: &Path, id: &str, no_history: bool) -> miette::Result<VizShare> {
    if no_history {
        return Ok(VizShare {
            v: 1,
            doc: load_store(base_dir, id)?.export_document(),
            frames: Vec::new(),
        });
    }

    let source_dir = load_meta(base_dir, id)
        .ok()
        .and_then(|m| m.source_dir)
        .map(PathBuf::from);
    let mut frames = read_timed_frames(&events_path(base_dir, id))?.into_iter();
    let first = frames
        .next()
        .ok_or_else(|| miette!("session `{id}` has no complete frames"))?;
    let mut first_events = first.events.into_iter();
    let Some(Event::Init(mut doc)) = first_events.next() else {
        return Err(miette!(
            "session `{id}` must begin with an initial `viz` document"
        ));
    };
    inline_external_code(&mut doc, source_dir.as_deref());

    let rest: Vec<Event> = first_events.collect();
    let leading = (!rest.is_empty()).then_some(Frame {
        ts: first.ts,
        events: rest,
    });
    let mut frames: Vec<Frame> = leading.into_iter().chain(frames).collect();
    for event in frames.iter_mut().flat_map(|f| f.events.iter_mut()) {
        inline_event_code(event, source_dir.as_deref());
    }
    Ok(VizShare {
        v: 1,
        doc: *doc,
        frames,
    })
}

/// The `POST /api/viz` body, refused before upload when it is over the limit.
pub fn request_body(share: &VizShare, req: &ShareRequest<'_>) -> miette::Result<String> {
    let mut body = json!({ "share": share, "public": !req.private });
    if let Some(title) = req.title {
        body["title"] = json!(title);
    }
    if let Some(host) = req.host {
        body["host"] = json!(host);
    }
    let encoded = serde_json::to_string(&body).into_diagnostic()?;
    if encoded.len() > MAX_SHARE_BYTES {
        let hint = if share.frames.is_empty() {
            ""
        } else {
            "; rerun with --no-history to share only the current state"
        };
        return Err(miette!(
            "share is {:.1} MB, over planhub's 5 MB limit{hint}",
            encoded.len() as f64 / 1_000_000.0
        ));
    }
    Ok(encoded)
}

/// Fails unless planhub answers 2xx with a `url`; the error carries planhub's
/// `detail` when it sends one.
pub fn post_share(
    runner: &dyn CommandRunner,
    planhub_url: &str,
    body: &str,
) -> Result<Value, String> {
    let url = format!("{}/api/viz", planhub_url.trim_end_matches('/'));
    let out = runner.run_with_stdin(
        "curl",
        &[
            "-sS",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            "-w",
            "\n%{http_code}",
            &url,
        ],
        body,
    )?;
    let (payload, status) = out.trim_end().rsplit_once('\n').unwrap_or(("", out.trim()));
    let status: u16 = status
        .trim()
        .parse()
        .map_err(|_| format!("unexpected curl output from {url}: {out}"))?;
    let parsed: Option<Value> = serde_json::from_str(payload).ok();

    if (200..300).contains(&status)
        && let Some(resp) = &parsed
        && resp.get("url").and_then(Value::as_str).is_some()
    {
        return Ok(resp.clone());
    }
    let detail = match parsed.as_ref().and_then(|v| v.get("detail")) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None if payload.trim().is_empty() => "empty response".to_string(),
        None => payload.trim().to_string(),
    };
    Err(format!("planhub returned HTTP {status}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use crate::model::{Document, LogEntry, LogLevel, Node};
    use crate::session::{RexMeta, append_events, create_session};

    struct FakeCurl {
        response: String,
        stdin: RefCell<Vec<(Vec<String>, String)>>,
    }

    impl FakeCurl {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                stdin: RefCell::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for FakeCurl {
        fn run(&self, _program: &str, _args: &[&str]) -> Result<String, String> {
            unreachable!("share pipes its body on stdin")
        }

        fn run_with_stdin(
            &self,
            program: &str,
            args: &[&str],
            stdin: &str,
        ) -> Result<String, String> {
            assert_eq!(program, "curl");
            self.stdin.borrow_mut().push((
                args.iter().map(ToString::to_string).collect(),
                stdin.to_string(),
            ));
            Ok(self.response.clone())
        }
    }

    fn code_doc() -> Document {
        let src = r#"viz v=1 title="Shared" {
            split direction="horizontal" {
                log id="events"
                code id="snippet" path="lib.rs"
            }
        }"#;
        parse_document(src, "test").unwrap()
    }

    fn code_node(id: &str) -> Node {
        let src = format!(r#"viz v=1 title="x" {{ code id="{id}" path="lib.rs"; }}"#);
        parse_document(&src, "test").unwrap().root
    }

    fn code_source(node: &Node) -> Option<&str> {
        match node {
            Node::Code { source, path, .. } if path.is_none() => Some(source),
            _ => None,
        }
    }

    fn session_with_history(dir: &Path) -> String {
        std::fs::write(dir.join("lib.rs"), "fn main() {}\n").unwrap();
        let meta = create_session(
            dir,
            code_doc(),
            RexMeta {
                source_dir: Some(dir.display().to_string()),
                ..RexMeta::default()
            },
        )
        .unwrap();
        let entry = |m: &str| LogEntry {
            ts: None,
            level: LogLevel::Info,
            source: None,
            message: m.into(),
        };
        append_events(
            dir,
            &meta.id,
            &[Event::LogAppend {
                target: "events".into(),
                entries: vec![entry("one"), entry("two")],
            }],
        )
        .unwrap();
        append_events(
            dir,
            &meta.id,
            &[
                Event::Replace {
                    target: "snippet".into(),
                    node: Box::new(code_node("snippet")),
                },
                Event::Close,
            ],
        )
        .unwrap();
        meta.id
    }

    #[test]
    fn session_share_carries_initial_doc_and_inlined_frames() {
        let tmp = tempfile::tempdir().unwrap();
        let id = session_with_history(tmp.path());
        let share = share_from_session(tmp.path(), &id, false).unwrap();

        assert_eq!(share.doc.title, "Shared");
        let Node::Split { children, .. } = &share.doc.root else {
            panic!("root changed");
        };
        assert_eq!(code_source(&children[1]), Some("fn main() {}\n"));

        assert_eq!(share.frames.len(), 2);
        assert!(share.frames.iter().all(|f| f.ts.is_some()));
        assert!(matches!(
            share.frames[0].events[..],
            [Event::LogAppend { .. }]
        ));
        let [Event::Replace { node, .. }, Event::Close] = &share.frames[1].events[..] else {
            panic!("unexpected frame {:?}", share.frames[1]);
        };
        assert_eq!(code_source(node), Some("fn main() {}\n"));
    }

    #[test]
    fn no_history_sends_the_current_doc() {
        let tmp = tempfile::tempdir().unwrap();
        let id = session_with_history(tmp.path());
        let share = share_from_session(tmp.path(), &id, true).unwrap();
        assert!(share.frames.is_empty());
        let Node::Split { children, .. } = &share.doc.root else {
            panic!("root changed");
        };
        let Node::Log { entries, .. } = &children[0] else {
            panic!("log moved");
        };
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn request_body_defaults_to_public_and_names_the_overage() {
        let share = VizShare {
            v: 1,
            doc: code_doc(),
            frames: vec![],
        };
        let req = ShareRequest {
            title: Some("Override"),
            private: false,
            host: Some("box"),
        };
        let body: Value = serde_json::from_str(&request_body(&share, &req).unwrap()).unwrap();
        assert_eq!(body["public"], json!(true));
        assert_eq!(body["title"], json!("Override"));
        assert_eq!(body["host"], json!("box"));
        assert_eq!(body["share"]["doc"]["title"], json!("Shared"));
        assert_eq!(body["share"]["frames"], json!([]));

        let mut big = share.clone();
        big.frames = vec![Frame {
            ts: None,
            events: vec![Event::LogAppend {
                target: "events".into(),
                entries: vec![LogEntry {
                    ts: None,
                    level: LogLevel::Info,
                    source: None,
                    message: "x".repeat(MAX_SHARE_BYTES),
                }],
            }],
        }];
        let err = request_body(&big, &req).unwrap_err().to_string();
        assert!(err.contains("5.0 MB"), "{err}");
        assert!(err.contains("--no-history"), "{err}");
    }

    #[test]
    fn post_share_pipes_the_body_and_returns_the_url() {
        let curl = FakeCurl::new(
            "{\"ok\":true,\"slug\":\"abc\",\"url\":\"https://planhub.test/p/abc\"}\n200",
        );
        let resp = post_share(&curl, "https://planhub.test/", "{\"share\":1}").unwrap();
        assert_eq!(resp["url"], json!("https://planhub.test/p/abc"));
        let calls = curl.stdin.borrow();
        let (args, stdin) = &calls[0];
        assert_eq!(stdin, "{\"share\":1}");
        assert_eq!(args.last().unwrap(), "https://planhub.test/api/viz");
        assert!(args.windows(2).any(|w| w == ["--data-binary", "@-"]));
    }

    #[test]
    fn post_share_surfaces_planhub_detail() {
        let curl = FakeCurl::new("{\"detail\":\"share.doc.v must be 1\"}\n422");
        let err = post_share(&curl, DEFAULT_PLANHUB_URL, "{}").unwrap_err();
        assert_eq!(err, "planhub returned HTTP 422: share.doc.v must be 1");

        let curl = FakeCurl::new("<html>bad gateway</html>\n502");
        let err = post_share(&curl, DEFAULT_PLANHUB_URL, "{}").unwrap_err();
        assert_eq!(err, "planhub returned HTTP 502: <html>bad gateway</html>");
    }
}
