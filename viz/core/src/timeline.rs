use std::fmt::Write as _;

use crate::model::{Event, Frame, StateValue, VizShare};
use crate::store::Store;

/// Positions between stored snapshots, so reaching any position replays at
/// most this many frames.
const CHECKPOINT_EVERY: usize = 64;
const MAX_SUMMARY_CHARS: usize = 96;

/// Replays a `VizShare`: position 0 is the initial document and position `i`
/// is the store after applying frames `1..=i`.
pub struct Timeline {
    frames: Vec<Frame>,
    checkpoints: Vec<Store>,
}

impl Timeline {
    pub fn new(share: VizShare) -> Self {
        let mut store = Store::new(share.doc);
        let mut checkpoints = vec![store.clone()];
        for (i, frame) in share.frames.iter().enumerate() {
            apply_frame(&mut store, frame);
            if (i + 1) % CHECKPOINT_EVERY == 0 {
                checkpoints.push(store.clone());
            }
        }
        Self {
            frames: share.frames,
            checkpoints,
        }
    }

    pub fn positions(&self) -> usize {
        self.frames.len() + 1
    }

    pub fn at(&self, position: usize) -> Option<Store> {
        if position >= self.positions() {
            return None;
        }
        let base = position / CHECKPOINT_EVERY;
        let mut store = self.checkpoints[base].clone();
        for frame in &self.frames[base * CHECKPOINT_EVERY..position] {
            apply_frame(&mut store, frame);
        }
        Some(store)
    }

    /// The frame's timestamp and a short summary of its events; position 0 is
    /// `"initial"`.
    pub fn label(&self, position: usize) -> Option<(Option<u64>, String)> {
        match position {
            0 => Some((None, "initial".to_string())),
            _ => {
                let frame = self.frames.get(position - 1)?;
                Some((frame.ts, summarize(&frame.events)))
            }
        }
    }
}

fn apply_frame(store: &mut Store, frame: &Frame) {
    for event in &frame.events {
        store.apply_event(event.clone());
    }
}

/// One event's summary, or a count when a frame mixes several.
pub fn summarize(events: &[Event]) -> String {
    let summary = match events {
        [] => "no ops".to_string(),
        [one] => summarize_event(one),
        [first, ..] => {
            if let Event::LogAppend { target, .. } = first
                && events
                    .iter()
                    .all(|e| matches!(e, Event::LogAppend { target: t, .. } if t == target))
            {
                let count = events
                    .iter()
                    .map(|e| match e {
                        Event::LogAppend { entries, .. } => entries.len(),
                        _ => 0,
                    })
                    .sum();
                log_append_summary(target, count)
            } else if events.iter().all(|e| op_name(e) == op_name(first)) {
                format!("{} ×{}", op_name(first), events.len())
            } else {
                format!("{} ops", events.len())
            }
        }
    };
    truncate(summary)
}

fn summarize_event(event: &Event) -> String {
    let mut out = op_name(event).to_string();
    match event {
        Event::Init(doc) => {
            let _ = write!(out, " \"{}\"", doc.title);
        }
        Event::SetState(updates) => {
            for (k, v) in updates {
                let _ = write!(out, " {k}={}", state_text(v));
            }
        }
        Event::SetNode {
            graph_id,
            node_id,
            label,
            sublabel,
            status,
            tooltip,
        } => {
            let _ = write!(out, " {graph_id}/{node_id}");
            if let Some(s) = status {
                let _ = write!(out, " status={}", s.as_str());
            }
            push_field(&mut out, "label", label.as_deref());
            push_field(&mut out, "sublabel", sublabel.as_deref());
            push_field(&mut out, "tooltip", tooltip.as_deref());
        }
        Event::SetEdge {
            graph_id,
            from,
            to,
            label,
            active,
            tooltip,
        } => {
            let _ = write!(out, " {graph_id}/{from}->{to}");
            if let Some(a) = active {
                let _ = write!(out, " active={a}");
            }
            push_field(&mut out, "label", label.as_deref());
            push_field(&mut out, "tooltip", tooltip.as_deref());
        }
        Event::SetStat {
            stat_id,
            item_id,
            value,
            delta,
            status,
            ..
        } => {
            let _ = write!(out, " {stat_id}/{item_id}");
            push_field(&mut out, "value", value.as_deref());
            push_field(&mut out, "delta", delta.as_deref());
            if let Some(s) = status {
                let _ = write!(out, " status={}", s.as_str());
            }
        }
        Event::LogAppend { target, entries } => return log_append_summary(target, entries.len()),
        Event::ChartAppend {
            target,
            series,
            points,
            ..
        } => {
            let _ = write!(out, " {target}/{series}");
            if points.len() > 1 {
                let _ = write!(out, " ×{}", points.len());
            }
        }
        Event::Replace { target, .. } => {
            let _ = write!(out, " {target}");
        }
        Event::Close => {}
    }
    out
}

fn op_name(event: &Event) -> &'static str {
    match event {
        Event::Init(_) => "init",
        Event::SetState(_) => "set-state",
        Event::SetNode { .. } => "set-node",
        Event::SetEdge { .. } => "set-edge",
        Event::SetStat { .. } => "set-stat",
        Event::LogAppend { .. } => "log-append",
        Event::ChartAppend { .. } => "chart-append",
        Event::Replace { .. } => "replace",
        Event::Close => "close",
    }
}

fn log_append_summary(target: &str, count: usize) -> String {
    if count == 1 {
        format!("log-append {target}")
    } else {
        format!("log-append {target} ×{count}")
    }
}

fn push_field(out: &mut String, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        let _ = write!(out, " {key}={v}");
    }
}

fn state_text(value: &StateValue) -> String {
    match value {
        StateValue::Range { start, end } => format!("{start}..{end}"),
        StateValue::Bool(b) => b.to_string(),
        StateValue::Int(i) => i.to_string(),
        StateValue::Float(f) => f.to_string(),
        StateValue::List(items) => items.join(","),
        StateValue::Str(s) => s.clone(),
    }
}

fn truncate(summary: String) -> String {
    let single_line = summary.replace('\n', " ");
    if single_line.chars().count() <= MAX_SUMMARY_CHARS {
        return single_line;
    }
    let mut cut: String = single_line.chars().take(MAX_SUMMARY_CHARS - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Document, LogEntry, LogLevel, Node, Status};

    fn doc() -> Document {
        Document {
            v: 1,
            title: "T".into(),
            subtitle: None,
            width: None,
            height: None,
            state: Default::default(),
            steps: vec![],
            root: Node::Log {
                id: "events".into(),
                title: None,
                height: None,
                entries: vec![],
            },
        }
    }

    fn log(n: usize) -> Event {
        Event::LogAppend {
            target: "events".into(),
            entries: (0..n)
                .map(|i| LogEntry {
                    ts: None,
                    level: LogLevel::Info,
                    source: None,
                    message: format!("m{i}"),
                })
                .collect(),
        }
    }

    fn entry_count(store: &Store) -> usize {
        match &store.doc.root {
            Node::Log { entries, .. } => entries.len(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn at_replays_across_checkpoints() {
        let frames = (0..150)
            .map(|i| Frame {
                ts: Some(i),
                events: vec![log(1)],
            })
            .collect();
        let tl = Timeline::new(VizShare {
            v: 1,
            doc: doc(),
            frames,
        });
        assert_eq!(tl.positions(), 151);
        for pos in [0, 1, 63, 64, 65, 128, 150] {
            assert_eq!(entry_count(&tl.at(pos).unwrap()), pos);
        }
        assert!(tl.at(151).is_none());
        assert_eq!(tl.label(0), Some((None, "initial".into())));
        assert_eq!(tl.label(5), Some((Some(4), "log-append events".into())));
        assert_eq!(tl.label(151), None);
    }

    #[test]
    fn summaries_name_the_change() {
        let set_node = Event::SetNode {
            graph_id: "arch".into(),
            node_id: "auth".into(),
            label: None,
            sublabel: None,
            status: Some(Status::Warn),
            tooltip: None,
        };
        assert_eq!(
            summarize(std::slice::from_ref(&set_node)),
            "set-node arch/auth status=warn"
        );
        assert_eq!(summarize(&[log(2), log(1)]), "log-append events ×3");
        let mut state = crate::model::StateMap::new();
        state.insert("selected".into(), StateValue::Str("auth".into()));
        assert_eq!(
            summarize(&[Event::SetState(state)]),
            "set-state selected=auth"
        );
        assert_eq!(
            summarize(&[set_node.clone(), set_node.clone()]),
            "set-node ×2"
        );
        assert_eq!(summarize(&[set_node, log(1), Event::Close]), "3 ops");
    }
}
