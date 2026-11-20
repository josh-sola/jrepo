use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{
    ChartSeries, Document, Event, Node, StateMap, StateValue, infer_lang_from_path,
};

#[derive(Debug, Clone)]
pub struct Store {
    pub doc: Document,
    pub closed: bool,
    pub step_index: usize,
    pub user_selected: Option<String>,
    pub active_tabs: BTreeMap<String, usize>,
    pub base_dir: Option<PathBuf>,
}

impl Store {
    pub fn new(doc: Document) -> Self {
        Self::with_base_dir(doc, None)
    }

    pub fn with_base_dir(doc: Document, base_dir: Option<PathBuf>) -> Self {
        Self {
            doc,
            closed: false,
            step_index: 0,
            user_selected: None,
            active_tabs: BTreeMap::new(),
            base_dir,
        }
    }

    pub fn apply_event(&mut self, event: Event) {
        match event {
            Event::Init(doc) => {
                self.doc = *doc;
                self.closed = false;
                if self.step_index >= self.doc.steps.len() {
                    self.step_index = 0;
                }
            }
            Event::SetState(updates) => {
                for (k, v) in updates {
                    if k == "_step"
                        && let StateValue::Int(idx) = v
                        && let Ok(step) = usize::try_from(idx)
                        && step < self.doc.steps.len()
                    {
                        self.step_index = step;
                        self.user_selected = None;
                        continue;
                    }
                    self.doc.state.insert(k, v);
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
                update_node_by_id(&mut self.doc.root, &graph_id, &mut |node| {
                    if let Node::Graph { nodes, .. } = node
                        && let Some(target) = nodes.iter_mut().find(|n| n.id == node_id)
                    {
                        if let Some(l) = label.clone() {
                            target.label = l;
                        }
                        if let Some(sub) = sublabel.clone() {
                            target.sublabel = Some(sub);
                        }
                        if let Some(st) = status {
                            target.status = Some(st);
                        }
                        if let Some(tt) = tooltip.clone() {
                            target.tooltip = Some(tt);
                        }
                    }
                });
            }
            Event::SetEdge {
                graph_id,
                from,
                to,
                label,
                active,
                tooltip,
            } => {
                update_node_by_id(&mut self.doc.root, &graph_id, &mut |node| {
                    if let Node::Graph { edges, .. } = node
                        && let Some(edge) = edges.iter_mut().find(|e| e.from == from && e.to == to)
                    {
                        if let Some(l) = label.clone() {
                            edge.label = Some(l);
                        }
                        if let Some(a) = active {
                            edge.active = a;
                        }
                        if let Some(tt) = tooltip.clone() {
                            edge.tooltip = Some(tt);
                        }
                    }
                });
            }
            Event::SetStat {
                stat_id,
                item_id,
                value,
                delta,
                status,
                sparkline_push,
            } => {
                update_node_by_id(&mut self.doc.root, &stat_id, &mut |node| {
                    if let Node::Stat { items, .. } = node
                        && let Some(item) = items.iter_mut().find(|i| i.id == item_id)
                    {
                        if let Some(v) = value.clone() {
                            item.value = v;
                        }
                        if let Some(d) = delta.clone() {
                            item.delta = Some(d);
                        }
                        if let Some(st) = status {
                            item.status = Some(st);
                        }
                        if let Some(pt) = sparkline_push {
                            item.sparkline.push(pt);
                        }
                    }
                });
            }
            Event::LogAppend { target, entries } => {
                let mut pending = Some(entries);
                update_node_by_id(&mut self.doc.root, &target, &mut |node| {
                    if let Node::Log { entries: log, .. } = node
                        && let Some(mut incoming) = pending.take()
                    {
                        log.append(&mut incoming);
                    }
                });
            }
            Event::ChartAppend {
                target,
                series,
                x_label,
                points,
                max_points,
            } => {
                let mut pending_pts = Some(points);
                let mut pending_label = x_label;
                update_node_by_id(&mut self.doc.root, &target, &mut |node| {
                    if let Node::Chart {
                        x_labels,
                        series: all_series,
                        ..
                    } = node
                    {
                        if let Some(lbl) = pending_label.take() {
                            x_labels.push(lbl);
                            if let Some(max) = max_points
                                && x_labels.len() > max
                            {
                                let excess = x_labels.len() - max;
                                x_labels.drain(0..excess);
                            }
                        }
                        if let Some(mut incoming) = pending_pts.take() {
                            let target_series =
                                match all_series.iter_mut().find(|s| s.name == series) {
                                    Some(existing) => existing,
                                    None => {
                                        all_series.push(ChartSeries {
                                            name: series.clone(),
                                            color: None,
                                            data: Vec::new(),
                                        });
                                        all_series.last_mut().unwrap()
                                    }
                                };
                            target_series.data.append(&mut incoming);
                            if let Some(max) = max_points
                                && target_series.data.len() > max
                            {
                                let excess = target_series.data.len() - max;
                                target_series.data.drain(0..excess);
                            }
                        }
                    }
                });
            }
            Event::Replace { target, node } => {
                let mut replacement = Some(*node);
                replace_node_by_id(&mut self.doc.root, &target, &mut replacement);
            }
            Event::Close => {
                self.closed = true;
            }
        }
    }

    pub fn effective_state(&self) -> StateMap {
        let mut merged = self.doc.state.clone();
        if let Some(step) = self.doc.steps.get(self.step_index) {
            for (k, v) in &step.state {
                merged.insert(k.clone(), v.clone());
            }
        }
        if let Some(selected) = &self.user_selected {
            merged.insert("selected".into(), StateValue::Str(selected.clone()));
        }
        merged
    }

    pub fn next_step(&mut self) {
        if self.doc.steps.is_empty() {
            return;
        }
        if self.step_index + 1 < self.doc.steps.len() {
            self.step_index += 1;
            self.user_selected = None;
        }
    }

    pub fn prev_step(&mut self) {
        if self.step_index > 0 {
            self.step_index -= 1;
            self.user_selected = None;
        }
    }

    pub fn set_step(&mut self, idx: usize) {
        if idx < self.doc.steps.len() {
            self.step_index = idx;
            self.user_selected = None;
        }
    }

    pub fn select(&mut self, id: Option<String>) {
        self.user_selected = id;
    }

    pub fn export_document(&self) -> Document {
        let mut snapshot = self.doc.clone();
        snapshot.state = self.effective_state();
        inline_external_code(&mut snapshot, self.base_dir.as_deref());
        snapshot
    }
}

pub fn resolve_code_file(
    path: &str,
    lines: Option<(usize, usize)>,
    base_dir: Option<&Path>,
) -> Result<(String, usize), String> {
    let raw_path = Path::new(path);
    let resolved = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else if let Some(dir) = base_dir {
        dir.join(raw_path)
    } else {
        raw_path.to_path_buf()
    };

    let content =
        fs::read_to_string(&resolved).map_err(|e| format!("{}: {e}", resolved.display()))?;

    if let Some((lo, hi)) = lines {
        let start = lo.max(1);
        let end = hi.max(start);
        let sliced: Vec<&str> = content
            .lines()
            .skip(start - 1)
            .take(end - start + 1)
            .collect();
        Ok((sliced.join("\n"), start))
    } else {
        Ok((content, 1))
    }
}

pub fn inline_external_code(doc: &mut Document, base_dir: Option<&Path>) {
    inline_external_code_node(&mut doc.root, base_dir);
}

fn inline_external_code_node(node: &mut Node, base_dir: Option<&Path>) {
    match node {
        Node::Split { children, .. } | Node::Scroll { children, .. } => {
            for child in children {
                inline_external_code_node(child, base_dir);
            }
        }
        Node::Tabs { items, .. } => {
            for item in items {
                inline_external_code_node(&mut item.node, base_dir);
            }
        }
        Node::Code {
            lang,
            path,
            lines,
            source,
            start_line,
            by_selected,
            ..
        } => {
            if let Some(p) = path.as_deref()
                && let Ok((loaded, start)) = resolve_code_file(p, *lines, base_dir)
            {
                *source = loaded;
                *start_line = start;
                if lang.is_none() {
                    *lang = infer_lang_from_path(p).map(ToString::to_string);
                }
                *path = None;
                *lines = None;
            }
            for variant in by_selected.values_mut() {
                if let Some(p) = variant.path.as_deref()
                    && let Ok((loaded, start)) = resolve_code_file(p, variant.lines, base_dir)
                {
                    variant.source = loaded;
                    variant.start_line = start;
                    if variant.lang.is_none() {
                        variant.lang = infer_lang_from_path(p).map(ToString::to_string);
                    }
                    variant.path = None;
                    variant.lines = None;
                }
            }
        }
        Node::Markdown { .. }
        | Node::Stat { .. }
        | Node::Table { .. }
        | Node::Log { .. }
        | Node::Graph { .. }
        | Node::Chart { .. }
        | Node::Svg { .. } => {}
    }
}

pub fn compute_chart_scales(root: &Node) -> BTreeMap<String, f64> {
    let mut scales = BTreeMap::new();
    collect_chart_scales(root, &mut scales);
    scales
}

fn collect_chart_scales(node: &Node, out: &mut BTreeMap<String, f64>) {
    match node {
        Node::Split { children, .. } | Node::Scroll { children, .. } => {
            for child in children {
                collect_chart_scales(child, out);
            }
        }
        Node::Tabs { items, .. } => {
            for item in items {
                collect_chart_scales(&item.node, out);
            }
        }
        Node::Chart {
            scale: Some(name),
            series,
            ..
        } => {
            let max_val = series
                .iter()
                .flat_map(|s| s.data.iter())
                .map(|p| p.y)
                .filter(|v| v.is_finite())
                .fold(f64::NEG_INFINITY, f64::max);
            if max_val.is_finite() {
                out.entry(name.clone())
                    .and_modify(|cur| *cur = cur.max(max_val))
                    .or_insert(max_val);
            }
        }
        Node::Chart { scale: None, .. }
        | Node::Markdown { .. }
        | Node::Code { .. }
        | Node::Stat { .. }
        | Node::Table { .. }
        | Node::Log { .. }
        | Node::Graph { .. }
        | Node::Svg { .. } => {}
    }
}

fn update_node_by_id(node: &mut Node, target_id: &str, f: &mut impl FnMut(&mut Node)) {
    if node.id() == Some(target_id) {
        f(node);
    }
    match node {
        Node::Split { children, .. } | Node::Scroll { children, .. } => {
            for child in children {
                update_node_by_id(child, target_id, f);
            }
        }
        Node::Tabs { items, .. } => {
            for item in items {
                update_node_by_id(&mut item.node, target_id, f);
            }
        }
        Node::Markdown { .. }
        | Node::Code { .. }
        | Node::Stat { .. }
        | Node::Table { .. }
        | Node::Log { .. }
        | Node::Graph { .. }
        | Node::Chart { .. }
        | Node::Svg { .. } => {}
    }
}

fn replace_node_by_id(node: &mut Node, target_id: &str, replacement: &mut Option<Node>) -> bool {
    if node.id() == Some(target_id) {
        if let Some(new_node) = replacement.take() {
            *node = new_node;
        }
        return true;
    }
    match node {
        Node::Split { children, .. } | Node::Scroll { children, .. } => {
            for child in children {
                if replace_node_by_id(child, target_id, replacement) {
                    return true;
                }
            }
            false
        }
        Node::Tabs { items, .. } => {
            for item in items {
                if replace_node_by_id(&mut item.node, target_id, replacement) {
                    return true;
                }
            }
            false
        }
        Node::Markdown { .. }
        | Node::Code { .. }
        | Node::Stat { .. }
        | Node::Table { .. }
        | Node::Log { .. }
        | Node::Graph { .. }
        | Node::Chart { .. }
        | Node::Svg { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ChartPoint, ChartType, EdgeStyle, GraphDirection, GraphEdge, GraphNode, LogEntry, LogLevel,
        NodeShape, SplitDirection, StatItem, Status, Step, TabItem,
    };

    fn sample_store() -> Store {
        let mut base_state = StateMap::new();
        base_state.insert("selected".into(), StateValue::Str("a".into()));
        base_state.insert("theme-hint".into(), StateValue::Str("base".into()));

        let mut step1_state = StateMap::new();
        step1_state.insert("selected".into(), StateValue::Str("b".into()));

        let doc = Document {
            v: 1,
            title: "Store Test".into(),
            subtitle: None,
            width: None,
            height: None,
            state: base_state,
            steps: vec![
                Step {
                    id: Some("s0".into()),
                    title: "Step 0".into(),
                    body: String::new(),
                    state: StateMap::new(),
                },
                Step {
                    id: Some("s1".into()),
                    title: "Step 1".into(),
                    body: String::new(),
                    state: step1_state,
                },
            ],
            root: Node::Split {
                direction: SplitDirection::Horizontal,
                ratio: None,
                weights: vec![1, 1],
                children: vec![
                    Node::Tabs {
                        id: "tabs".into(),
                        items: vec![
                            TabItem {
                                id: None,
                                title: "Graph".into(),
                                node: Node::Graph {
                                    id: "g1".into(),
                                    title: None,
                                    caption: None,
                                    height: None,
                                    direction: GraphDirection::Lr,
                                    nodes: vec![
                                        GraphNode {
                                            id: "a".into(),
                                            label: "A".into(),
                                            sublabel: None,
                                            status: Some(Status::Idle),
                                            group: None,
                                            shape: NodeShape::Rounded,
                                            tooltip: None,
                                        },
                                        GraphNode {
                                            id: "b".into(),
                                            label: "B".into(),
                                            sublabel: None,
                                            status: None,
                                            group: None,
                                            shape: NodeShape::Box,
                                            tooltip: None,
                                        },
                                    ],
                                    edges: vec![GraphEdge {
                                        from: "a".into(),
                                        to: "b".into(),
                                        label: None,
                                        style: EdgeStyle::Solid,
                                        active: false,
                                        tooltip: None,
                                    }],
                                    highlight_key: None,
                                    select_key: None,
                                },
                            },
                            TabItem {
                                id: None,
                                title: "Chart".into(),
                                node: Node::Chart {
                                    id: "c1".into(),
                                    title: None,
                                    caption: None,
                                    height: None,
                                    chart_type: ChartType::Line,
                                    x_labels: vec!["t0".into(), "t1".into()],
                                    x_unit: None,
                                    y_unit: None,
                                    value_labels: false,
                                    scale: None,
                                    series: vec![ChartSeries {
                                        name: "s".into(),
                                        color: None,
                                        data: vec![
                                            ChartPoint { x: None, y: 1.0 },
                                            ChartPoint { x: None, y: 2.0 },
                                        ],
                                    }],
                                },
                            },
                        ],
                    },
                    Node::Split {
                        direction: SplitDirection::Vertical,
                        ratio: None,
                        weights: vec![1, 1, 1],
                        children: vec![
                            Node::Stat {
                                id: Some("st1".into()),
                                title: None,
                                height: None,
                                items: vec![StatItem {
                                    id: "qps".into(),
                                    label: "QPS".into(),
                                    value: "100".into(),
                                    delta: None,
                                    status: None,
                                    sparkline: vec![1.0],
                                }],
                            },
                            Node::Log {
                                id: "log1".into(),
                                title: None,
                                height: None,
                                entries: vec![],
                            },
                            Node::Markdown {
                                id: Some("md1".into()),
                                title: None,
                                tone: None,
                                height: None,
                                content: "old".into(),
                                by_selected: BTreeMap::new(),
                            },
                        ],
                    },
                ],
            },
        };
        Store::new(doc)
    }

    #[test]
    fn applies_all_semantic_ops_and_max_points() {
        let mut store = sample_store();

        store.apply_event(Event::SetNode {
            graph_id: "g1".into(),
            node_id: "a".into(),
            label: Some("Alpha".into()),
            sublabel: Some("v2".into()),
            status: Some(Status::Ok),
            tooltip: Some("tip".into()),
        });
        store.apply_event(Event::SetEdge {
            graph_id: "g1".into(),
            from: "a".into(),
            to: "b".into(),
            label: Some("rpc".into()),
            active: Some(true),
            tooltip: Some("fast".into()),
        });
        store.apply_event(Event::SetStat {
            stat_id: "st1".into(),
            item_id: "qps".into(),
            value: Some("250".into()),
            delta: Some("+150%".into()),
            status: Some(Status::Active),
            sparkline_push: Some(2.5),
        });
        store.apply_event(Event::LogAppend {
            target: "log1".into(),
            entries: vec![LogEntry {
                ts: Some("00:01".into()),
                level: LogLevel::Info,
                source: None,
                message: "hello".into(),
            }],
        });
        store.apply_event(Event::ChartAppend {
            target: "c1".into(),
            series: "s".into(),
            x_label: Some("t2".into()),
            points: vec![ChartPoint { x: None, y: 3.0 }],
            max_points: Some(2),
        });
        store.apply_event(Event::Replace {
            target: "md1".into(),
            node: Box::new(Node::Markdown {
                id: Some("md1".into()),
                title: Some("New".into()),
                tone: None,
                height: None,
                content: "replaced".into(),
                by_selected: BTreeMap::new(),
            }),
        });
        store.apply_event(Event::Close);
        assert!(store.closed);

        let Node::Split { children, .. } = &store.doc.root else {
            panic!("expected root split");
        };
        let Node::Tabs { items, .. } = &children[0] else {
            panic!("expected tabs");
        };
        let Node::Graph { nodes, edges, .. } = &items[0].node else {
            panic!("expected graph");
        };
        assert_eq!(nodes[0].label, "Alpha");
        assert_eq!(nodes[0].status, Some(Status::Ok));
        assert!(edges[0].active);
        assert_eq!(edges[0].label.as_deref(), Some("rpc"));

        let Node::Chart {
            x_labels, series, ..
        } = &items[1].node
        else {
            panic!("expected chart");
        };
        assert_eq!(x_labels, &["t1", "t2"]);
        assert_eq!(series[0].data.len(), 2);
        assert_eq!(series[0].data[1].y, 3.0);

        let Node::Split {
            children: right, ..
        } = &children[1]
        else {
            panic!("expected right split");
        };
        let Node::Stat { items: stats, .. } = &right[0] else {
            panic!("expected stat");
        };
        assert_eq!(stats[0].value, "250");
        assert_eq!(stats[0].sparkline, vec![1.0, 2.5]);

        let Node::Log { entries, .. } = &right[1] else {
            panic!("expected log");
        };
        assert_eq!(entries.len(), 1);

        let Node::Markdown { content, .. } = &right[2] else {
            panic!("expected markdown");
        };
        assert_eq!(content, "replaced");
    }

    #[test]
    fn effective_state_precedence_and_export() {
        let mut store = sample_store();
        assert_eq!(
            store.effective_state().get("selected"),
            Some(&StateValue::Str("a".into()))
        );

        store.next_step();
        assert_eq!(store.step_index, 1);
        assert_eq!(
            store.effective_state().get("selected"),
            Some(&StateValue::Str("b".into()))
        );

        store.select(Some("user-pick".into()));
        assert_eq!(
            store.effective_state().get("selected"),
            Some(&StateValue::Str("user-pick".into()))
        );

        let exported = store.export_document();
        assert_eq!(
            exported.state.get("selected"),
            Some(&StateValue::Str("user-pick".into()))
        );
        assert_eq!(
            exported.state.get("theme-hint"),
            Some(&StateValue::Str("base".into()))
        );

        store.prev_step();
        assert_eq!(store.step_index, 0);
        assert_eq!(store.user_selected, None);
    }

    #[test]
    fn applies_events_recursively_inside_scroll() {
        let doc = Document {
            v: 1,
            title: "Scroll Ops".into(),
            subtitle: None,
            width: None,
            height: None,
            state: StateMap::new(),
            steps: vec![],
            root: Node::Scroll {
                id: "page".into(),
                children: vec![
                    Node::Stat {
                        id: Some("kpis".into()),
                        title: None,
                        height: Some(3),
                        items: vec![StatItem {
                            id: "p50".into(),
                            label: "P50".into(),
                            value: "10s".into(),
                            delta: None,
                            status: None,
                            sparkline: vec![],
                        }],
                    },
                    Node::Chart {
                        id: "bench".into(),
                        title: Some("Bench".into()),
                        caption: None,
                        height: Some(12),
                        chart_type: ChartType::HBar,
                        x_labels: vec!["cold".into()],
                        x_unit: Some("seconds".into()),
                        y_unit: None,
                        value_labels: true,
                        scale: None,
                        series: vec![ChartSeries {
                            name: "v1".into(),
                            color: None,
                            data: vec![ChartPoint { x: None, y: 1.5 }],
                        }],
                    },
                    Node::Markdown {
                        id: Some("summary".into()),
                        title: None,
                        tone: Some(Status::Warn),
                        height: None,
                        content: "before".into(),
                        by_selected: BTreeMap::new(),
                    },
                ],
            },
        };
        let mut store = Store::new(doc);
        store.apply_event(Event::SetStat {
            stat_id: "kpis".into(),
            item_id: "p50".into(),
            value: Some("8s".into()),
            delta: Some("-20%".into()),
            status: Some(Status::Ok),
            sparkline_push: None,
        });
        store.apply_event(Event::ChartAppend {
            target: "bench".into(),
            series: "v1".into(),
            x_label: Some("warm".into()),
            points: vec![ChartPoint { x: None, y: 0.4 }],
            max_points: None,
        });
        store.apply_event(Event::Replace {
            target: "summary".into(),
            node: Box::new(Node::Markdown {
                id: Some("summary".into()),
                title: Some("Finding".into()),
                tone: Some(Status::Ok),
                height: None,
                content: "after".into(),
                by_selected: BTreeMap::new(),
            }),
        });

        let Node::Scroll { children, .. } = &store.doc.root else {
            panic!("expected scroll root");
        };
        let Node::Stat { items, .. } = &children[0] else {
            panic!("expected stat");
        };
        assert_eq!(items[0].value, "8s");
        let Node::Chart {
            x_labels, series, ..
        } = &children[1]
        else {
            panic!("expected chart");
        };
        assert_eq!(x_labels, &["cold", "warm"]);
        assert_eq!(series[0].data.len(), 2);
        let Node::Markdown { content, tone, .. } = &children[2] else {
            panic!("expected markdown");
        };
        assert_eq!(content, "after");
        assert_eq!(*tone, Some(Status::Ok));
    }

    #[test]
    fn set_state_step_event_updates_step_index_and_effective_selection() {
        let mut store = sample_store();
        let mut updates = StateMap::new();
        updates.insert("_step".into(), StateValue::Int(1));
        store.apply_event(Event::SetState(updates));

        assert_eq!(store.step_index, 1);
        assert_eq!(
            store.effective_state().get("selected"),
            Some(&StateValue::Str("b".into()))
        );
    }

    #[test]
    fn resolves_external_code_file_and_inlines_on_export() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("demo.rs");
        std::fs::write(&file, "line1\nline2\nline3\nline4\n").unwrap();

        let doc = Document {
            v: 1,
            title: "Code Ptr".into(),
            subtitle: None,
            width: None,
            height: None,
            state: StateMap::new(),
            steps: vec![],
            root: Node::Code {
                id: Some("src".into()),
                title: Some("demo.rs".into()),
                lang: None,
                path: Some("demo.rs".into()),
                lines: Some((2, 3)),
                height: None,
                source: String::new(),
                start_line: 1,
                highlight_lines: vec![],
                focus_key: None,
                by_selected: BTreeMap::new(),
            },
        };
        let store = Store::with_base_dir(doc, Some(tmp.path().to_path_buf()));
        let exported = store.export_document();
        let Node::Code {
            lang,
            path,
            lines,
            source,
            start_line,
            ..
        } = exported.root
        else {
            panic!("expected code root");
        };
        assert_eq!(lang.as_deref(), Some("rust"));
        assert_eq!(path, None);
        assert_eq!(lines, None);
        assert_eq!(start_line, 2);
        assert_eq!(source, "line2\nline3");
    }

    #[test]
    fn compute_chart_scales_aggregates_named_groups() {
        let doc = Document {
            v: 1,
            title: "Scales".into(),
            subtitle: None,
            width: None,
            height: None,
            state: StateMap::new(),
            steps: vec![],
            root: Node::Scroll {
                id: "s".into(),
                children: vec![
                    Node::Chart {
                        id: "c1".into(),
                        title: None,
                        caption: None,
                        height: None,
                        chart_type: ChartType::HBar,
                        x_labels: vec!["a".into()],
                        x_unit: None,
                        y_unit: None,
                        value_labels: false,
                        scale: Some("sec".into()),
                        series: vec![ChartSeries {
                            name: "s1".into(),
                            color: None,
                            data: vec![ChartPoint { x: None, y: 12.0 }],
                        }],
                    },
                    Node::Chart {
                        id: "c2".into(),
                        title: None,
                        caption: None,
                        height: None,
                        chart_type: ChartType::HBar,
                        x_labels: vec!["b".into()],
                        x_unit: None,
                        y_unit: None,
                        value_labels: false,
                        scale: Some("sec".into()),
                        series: vec![ChartSeries {
                            name: "s2".into(),
                            color: None,
                            data: vec![ChartPoint { x: None, y: 48.5 }],
                        }],
                    },
                ],
            },
        };
        let scales = compute_chart_scales(&doc.root);
        assert_eq!(scales.get("sec"), Some(&48.5));
    }
}
