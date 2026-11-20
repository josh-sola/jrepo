use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use miette::{Context, IntoDiagnostic, miette};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::kdl_codec::{format_event_kdl, parse_events};
use crate::model::{Document, Event, Frame};
use crate::store::Store;

pub const FRAME_DELIMITER: &str = "\n// ---viz-frame---\n";

/// Starts the first chunk of each `append_events` call, so readers can group a
/// call's events into one timestamped frame. KDL parsers skip it as a comment.
const TIMESTAMP_PREFIX: &str = "// viz-ts=";

const ADJECTIVES: &[&str] = &[
    "amber", "brisk", "calm", "cobalt", "crisp", "dusk", "ember", "fern", "flint", "frost",
    "gilded", "hazel", "indigo", "ivory", "jade", "keen", "lumen", "maple", "misty", "noble",
    "ochre", "opal", "pine", "plume", "quartz", "rapid", "sable", "sage", "silver", "slate",
    "solar", "swift", "tidal", "umbra", "velvet", "vivid",
];

const NOUNS: &[&str] = &[
    "alder", "anvil", "arch", "atlas", "beacon", "brook", "cairn", "cedar", "cliff", "comet",
    "coral", "crane", "delta", "drift", "falcon", "fjord", "forge", "glade", "harbor", "heron",
    "horizon", "island", "kestrel", "lantern", "larch", "meadow", "mesa", "nebula", "osprey",
    "pebble", "prism", "quasar", "ridge", "river", "spruce", "summit",
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RexMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rex_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rex_layer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rex_block_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created_at_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rex_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rex_layer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rex_block_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dir: Option<String>,
}

pub fn default_base_dir() -> miette::Result<PathBuf> {
    if let Ok(dir) = std::env::var("VIZ_DIR")
        && !dir.trim().is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var("HOME")
        .into_diagnostic()
        .wrap_err("HOME is not set and VIZ_DIR is unset")?;
    Ok(PathBuf::from(home).join(".local").join("share").join("viz"))
}

pub fn session_dir(base_dir: &Path, id: &str) -> PathBuf {
    base_dir.join(id)
}

pub fn events_path(base_dir: &Path, id: &str) -> PathBuf {
    session_dir(base_dir, id).join("events.kdl")
}

pub fn meta_path(base_dir: &Path, id: &str) -> PathBuf {
    session_dir(base_dir, id).join("meta.json")
}

pub fn generate_slug(base_dir: &Path) -> String {
    let mut rng = rand::rng();
    for _ in 0..64 {
        let adj = ADJECTIVES[rng.random_range(0..ADJECTIVES.len())];
        let noun = NOUNS[rng.random_range(0..NOUNS.len())];
        let candidate = format!("{adj}-{noun}");
        if !session_dir(base_dir, &candidate).exists() {
            return candidate;
        }
    }
    let adj = ADJECTIVES[rng.random_range(0..ADJECTIVES.len())];
    let noun = NOUNS[rng.random_range(0..NOUNS.len())];
    let suffix: u16 = rng.random_range(100..9999);
    format!("{adj}-{noun}-{suffix}")
}

pub fn create_session(
    base_dir: &Path,
    doc: Document,
    meta_rex: RexMeta,
) -> miette::Result<SessionMeta> {
    fs::create_dir_all(base_dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create session root {}", base_dir.display()))?;

    let id = generate_slug(base_dir);
    let dir = session_dir(base_dir, &id);
    fs::create_dir_all(&dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to create session dir {}", dir.display()))?;

    let created_at_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let meta = SessionMeta {
        id: id.clone(),
        title: doc.title.clone(),
        created_at_secs,
        rex_session: meta_rex.rex_session,
        rex_layer_id: meta_rex.rex_layer_id,
        rex_block_id: meta_rex.rex_block_id,
        source_dir: meta_rex.source_dir,
    };

    update_meta(base_dir, &meta)?;
    append_events(base_dir, &id, &[Event::Init(Box::new(doc))])?;
    Ok(meta)
}

pub fn append_events(base_dir: &Path, id: &str, events: &[Event]) -> miette::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut payload = format!("{TIMESTAMP_PREFIX}{now_ms}\n");
    for ev in events {
        payload.push_str(&format_event_kdl(ev));
        payload.push_str(FRAME_DELIMITER);
    }

    let path = events_path(base_dir, id);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to open {}", path.display()))?;

    file.write_all(payload.as_bytes())
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to append frame to {}", path.display()))?;
    file.flush().into_diagnostic()?;
    Ok(())
}

pub fn read_frames_from(path: &Path, byte_offset: &mut u64) -> miette::Result<Vec<Event>> {
    let mut file = File::open(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to open {}", path.display()))?;
    file.seek(SeekFrom::Start(*byte_offset)).into_diagnostic()?;

    let mut raw = Vec::new();
    file.read_to_end(&mut raw).into_diagnostic()?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }

    let delim = FRAME_DELIMITER.as_bytes();
    let Some(last_pos) = find_last_subslice(&raw, delim) else {
        return Ok(Vec::new());
    };
    let consumed_len = last_pos + delim.len();
    let complete = std::str::from_utf8(&raw[..consumed_len]).into_diagnostic()?;

    let mut events = Vec::new();
    let source_name = path.display().to_string();
    for frame in complete.split(FRAME_DELIMITER) {
        if frame.trim().is_empty() {
            continue;
        }
        let mut parsed = parse_events(frame, &source_name)?;
        events.append(&mut parsed);
    }

    *byte_offset += consumed_len as u64;
    Ok(events)
}

/// Reads every complete frame in the file, one per `append_events` call.
/// Chunks written before timestamps existed each become their own frame with
/// `ts: None`.
pub fn read_timed_frames(path: &Path) -> miette::Result<Vec<Frame>> {
    let raw = fs::read(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read {}", path.display()))?;
    let delim = FRAME_DELIMITER.as_bytes();
    let Some(last_pos) = find_last_subslice(&raw, delim) else {
        return Ok(Vec::new());
    };
    let complete = std::str::from_utf8(&raw[..last_pos + delim.len()]).into_diagnostic()?;

    let source_name = path.display().to_string();
    let mut frames: Vec<Frame> = Vec::new();
    for chunk in complete.split(FRAME_DELIMITER) {
        if chunk.trim().is_empty() {
            continue;
        }
        let events = parse_events(chunk, &source_name)?;
        let ts = chunk
            .strip_prefix(TIMESTAMP_PREFIX)
            .and_then(|rest| rest.lines().next())
            .and_then(|ms| ms.trim().parse::<u64>().ok());
        match frames.last_mut() {
            Some(open) if ts.is_none() && open.ts.is_some() => open.events.extend(events),
            _ => frames.push(Frame { ts, events }),
        }
    }
    Ok(frames)
}

fn find_last_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

pub fn load_store(base_dir: &Path, id: &str) -> miette::Result<Store> {
    let (store, _) = load_store_with_offset(base_dir, id)?;
    Ok(store)
}

pub fn load_store_with_offset(base_dir: &Path, id: &str) -> miette::Result<(Store, u64)> {
    let path = events_path(base_dir, id);
    let mut offset = 0u64;
    let events = read_frames_from(&path, &mut offset)?;
    let mut iter = events.into_iter();
    let first = iter
        .next()
        .ok_or_else(|| miette!("session `{id}` has no complete frames"))?;
    let Event::Init(doc) = first else {
        return Err(miette!(
            "session `{id}` must begin with an initial `viz` document"
        ));
    };
    let source_dir = load_meta(base_dir, id)
        .ok()
        .and_then(|m| m.source_dir)
        .map(PathBuf::from);
    let mut store = Store::with_base_dir(*doc, source_dir);
    for ev in iter {
        store.apply_event(ev);
    }
    Ok((store, offset))
}

pub fn list_sessions(base_dir: &Path) -> miette::Result<Vec<SessionMeta>> {
    if !base_dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(base_dir).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        if !entry.file_type().into_diagnostic()?.is_dir() {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(ToString::to_string) else {
            continue;
        };
        if let Ok(meta) = load_meta(base_dir, &id) {
            sessions.push(meta);
        }
    }
    sessions.sort_by(|a, b| {
        b.created_at_secs
            .cmp(&a.created_at_secs)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(sessions)
}

pub fn load_meta(base_dir: &Path, id: &str) -> miette::Result<SessionMeta> {
    let path = meta_path(base_dir, id);
    let raw = fs::read_to_string(&path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to parse {}", path.display()))
}

pub fn update_meta(base_dir: &Path, meta: &SessionMeta) -> miette::Result<()> {
    let path = meta_path(base_dir, &meta.id);
    let tmp = path.with_extension("json.tmp");
    let raw = serde_json::to_string_pretty(meta).into_diagnostic()?;
    fs::write(&tmp, raw).into_diagnostic()?;
    fs::rename(&tmp, &path).into_diagnostic()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LogEntry, LogLevel, Node};
    use tempfile::TempDir;

    fn minimal_doc(title: &str) -> Document {
        Document {
            v: 1,
            title: title.into(),
            subtitle: None,
            width: None,
            height: None,
            state: Default::default(),
            steps: vec![],
            root: Node::Log {
                id: "logs".into(),
                title: None,
                height: None,
                entries: vec![],
            },
        }
    }

    #[test]
    fn frames_without_timestamps_stand_alone() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.kdl");
        let old = format!("close{FRAME_DELIMITER}close{FRAME_DELIMITER}");
        fs::write(&path, old).unwrap();
        let frames = read_timed_frames(&path).unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.ts.is_none() && f.events.len() == 1));
    }

    #[test]
    fn create_append_read_frames_and_list_sessions() {
        let tmp = TempDir::new().unwrap();
        let meta = create_session(
            tmp.path(),
            minimal_doc("Live Trace"),
            RexMeta {
                rex_session: Some("sess-1".into()),
                rex_layer_id: None,
                rex_block_id: Some("blk-1".into()),
                source_dir: None,
            },
        )
        .unwrap();

        assert!(meta.id.contains('-'));
        assert_eq!(meta.title, "Live Trace");
        assert_eq!(load_meta(tmp.path(), &meta.id).unwrap(), meta);

        let path = events_path(tmp.path(), &meta.id);
        let mut offset = 0u64;
        let initial = read_frames_from(&path, &mut offset).unwrap();
        assert_eq!(initial.len(), 1);
        assert!(offset > 0);

        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"log-append target=\"logs\" {\n    entry \"partial")
                .unwrap();
        }
        let prev_offset = offset;
        let partial_read = read_frames_from(&path, &mut offset).unwrap();
        assert!(partial_read.is_empty());
        assert_eq!(offset, prev_offset);

        {
            let f = OpenOptions::new().write(true).open(&path).unwrap();
            f.set_len(prev_offset).unwrap();
        }
        append_events(
            tmp.path(),
            &meta.id,
            &[
                Event::LogAppend {
                    target: "logs".into(),
                    entries: vec![LogEntry {
                        ts: Some("10:00".into()),
                        level: LogLevel::Info,
                        source: None,
                        message: "complete".into(),
                    }],
                },
                Event::Close,
            ],
        )
        .unwrap();

        let next = read_frames_from(&path, &mut offset).unwrap();
        assert_eq!(next.len(), 2);

        let frames = read_timed_frames(&path).unwrap();
        assert_eq!(frames.len(), 2);
        assert!(matches!(frames[0].events[..], [Event::Init(_)]));
        assert!(matches!(
            frames[1].events[..],
            [Event::LogAppend { .. }, Event::Close]
        ));
        assert!(frames.iter().all(|f| f.ts.is_some()));

        let store = load_store(tmp.path(), &meta.id).unwrap();
        assert!(store.closed);
        match store.doc.root {
            Node::Log { entries, .. } => assert_eq!(entries.len(), 1),
            other => panic!("unexpected root: {other:?}"),
        }

        let listed = list_sessions(tmp.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, meta.id);
    }
}
