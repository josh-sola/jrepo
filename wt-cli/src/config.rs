use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use kdl::{FormatConfig, KdlDocument, KdlEntry, KdlEntryFormat, KdlError, KdlNode, KdlValue};
use miette::{LabeledSpan, Report, SourceSpan};

/// The config file's own schema version, unrelated to `store::Store::version`.
pub const CONFIG_VERSION: i128 = 1;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    pub env: BTreeMap<String, String>,
    pub repos: BTreeMap<String, RepoConfig>,
    pub features: Features,
}

/// A hook holds exactly one of a name into wt's own implementations, or an
/// argv to run instead.
#[derive(Debug, Clone, PartialEq)]
pub enum Hook {
    Builtin(String),
    Cmd(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Features {
    pub planter: Option<Planter>,
    pub terminal: Option<Terminal>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Planter {
    pub get_position: Option<Hook>,
    pub renumber_peers: Option<Hook>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Terminal {
    pub set_background: Option<Hook>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoConfig {
    pub trunk: String,
    pub branch_prefix: String,
    pub spares: u8,
    pub env: BTreeMap<String, String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub label: String,
    pub profile: String,
    pub cwd: String,
    pub cmd: Vec<String>,
}

pub fn default_spares() -> u8 {
    1
}

/// `$WT_CONFIG`, else `$XDG_CONFIG_HOME/wt/config.kdl`, else
/// `~/.config/wt/config.kdl`.
pub fn config_path() -> PathBuf {
    if let Ok(p) = env::var("WT_CONFIG") {
        return PathBuf::from(p);
    }
    if let Ok(p) = env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(p).join("wt").join("config.kdl");
    }
    let home = env::var("HOME").expect("HOME must be set");
    PathBuf::from(home)
        .join(".config")
        .join("wt")
        .join("config.kdl")
}

/// A missing config file is an empty config, not an error — a fresh machine
/// with no `config.kdl` yet is the normal starting state.
pub fn load(path: &Path) -> Result<Config> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    parse_config(&text).with_context(|| format!("parsing {}", path.display()))
}

/// The repo's config block, or an error naming the file the user must edit.
pub fn repo<'a>(config: &'a Config, name: &str) -> Result<&'a RepoConfig> {
    config.repos.get(name).ok_or_else(|| {
        anyhow!(
            "repo '{name}' has no config block in {}; run `wt repo adopt <repo> <path>` for it",
            config_path().display()
        )
    })
}

/// Appends a `repo` block only when `name` has none. Returns whether it wrote.
pub fn append_repo(path: &Path, name: &str, repo: &RepoConfig) -> Result<bool> {
    let mut doc = match fs::read_to_string(path) {
        Ok(text) => parse_document(&text).with_context(|| format!("parsing {}", path.display()))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => fresh_document(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };

    if find_repo_node(&doc, name).is_some() {
        return Ok(false);
    }

    let mut node = build_repo_node(name, repo);
    node.autoformat();
    force_quoted_strings(&mut node);
    // A blank line ahead of the new block, matching how a hand-written file
    // separates top-level blocks from one another.
    if let Some(format) = node.format_mut() {
        format.leading = "\n".to_string();
    }
    doc.nodes_mut().push(node);

    write_atomic(path, doc.to_string().as_bytes())?;
    Ok(true)
}

/// Replaces just the `step` children of an existing `repo` block, leaving its
/// other settings and the rest of the file untouched.
pub fn replace_repo_steps(path: &Path, name: &str, steps: &[Step]) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc = parse_document(&text).with_context(|| format!("parsing {}", path.display()))?;

    let idx = doc
        .nodes()
        .iter()
        .position(|n| is_repo_named(n, name))
        .ok_or_else(|| anyhow!("no repo block named '{name}' in {}", path.display()))?;

    let children = doc.nodes_mut()[idx].ensure_children();
    children.nodes_mut().retain(|n| n.name().value() != "step");
    for step in steps {
        let mut node = build_step_node(step);
        node.autoformat_config(&FormatConfig::builder().indent_level(1).build());
        force_quoted_strings(&mut node);
        children.nodes_mut().push(node);
    }

    write_atomic(path, doc.to_string().as_bytes())?;
    Ok(())
}

/// Sets an existing repo block's `spares` value, leaving the rest of the
/// file untouched. Errors when `name` has no block.
pub fn set_repo_spares(path: &Path, name: &str, spares: u8) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc = parse_document(&text).with_context(|| format!("parsing {}", path.display()))?;

    let idx = doc
        .nodes()
        .iter()
        .position(|n| is_repo_named(n, name))
        .ok_or_else(|| anyhow!("no repo block named '{name}' in {}", path.display()))?;

    let children = doc.nodes_mut()[idx].ensure_children();
    match children
        .nodes_mut()
        .iter_mut()
        .find(|n| n.name().value() == "spares")
    {
        Some(node) => {
            let entry = node
                .entries_mut()
                .first_mut()
                .ok_or_else(|| anyhow!("'spares' in repo '{name}' has no value"))?;
            entry.set_value(spares as i128);
            let value_repr = spares.to_string();
            match entry.format_mut() {
                Some(format) => format.value_repr = value_repr,
                None => entry.set_format(KdlEntryFormat {
                    value_repr,
                    ..Default::default()
                }),
            }
        }
        None => {
            let mut node = KdlNode::new("spares");
            node.push(KdlEntry::new(spares as i128));
            node.autoformat_config(&FormatConfig::builder().indent_level(1).build());
            let insert_at = children
                .nodes()
                .iter()
                .position(|n| n.name().value() == "branch-prefix")
                .or_else(|| {
                    children
                        .nodes()
                        .iter()
                        .position(|n| n.name().value() == "trunk")
                })
                .map(|i| i + 1)
                .unwrap_or(0);
            children.nodes_mut().insert(insert_at, node);
        }
    }

    write_atomic(path, doc.to_string().as_bytes())?;
    Ok(())
}

/// KDL v2 lets a plain-looking string render unquoted as a bare identifier
/// string. Generated config always quotes them instead, matching how a
/// person would hand-write the file.
fn force_quoted_strings(node: &mut KdlNode) {
    for entry in node.entries_mut() {
        if let KdlValue::String(s) = entry.value().clone() {
            entry.set_format(KdlEntryFormat {
                value_repr: quote_kdl_string(&s),
                leading: " ".to_string(),
                ..Default::default()
            });
        }
    }
    if let Some(children) = node.children_mut() {
        for child in children.nodes_mut() {
            force_quoted_strings(child);
        }
    }
}

fn quote_kdl_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn find_repo_node<'a>(doc: &'a KdlDocument, name: &str) -> Option<&'a KdlNode> {
    doc.nodes().iter().find(|n| is_repo_named(n, name))
}

fn is_repo_named(node: &KdlNode, name: &str) -> bool {
    node.name().value() == "repo" && node.get(0).and_then(|v| v.as_string()) == Some(name)
}

const HEADER_COMMENT: &str = "\
// wt's user config. `wt repo adopt` appends a `repo` block here once per repo,\n\
// but never rewrites a block you have edited by hand.\n\
//\n\
// Uncomment to opt into hooks that reach outside wt. Absent means off.\n\
// features {\n\
//     planter {\n\
//         get-position { builtin \"tmux-window\" }\n\
//         renumber-peers { builtin \"planter-state\" }\n\
//     }\n\
//     terminal {\n\
//         set-background { builtin \"osc11\" }\n\
//     }\n\
// }\n\
\n";

fn fresh_document() -> KdlDocument {
    let mut doc = KdlDocument::new();
    let mut version = KdlNode::new("version");
    version.push(KdlEntry::new(CONFIG_VERSION));
    version.autoformat();
    if let Some(format) = version.format_mut() {
        format.leading = HEADER_COMMENT.to_string();
    }
    doc.nodes_mut().push(version);
    doc
}

fn build_repo_node(name: &str, repo: &RepoConfig) -> KdlNode {
    let mut node = KdlNode::new("repo");
    node.push(KdlEntry::new(name));

    let mut trunk = KdlNode::new("trunk");
    trunk.push(KdlEntry::new(repo.trunk.as_str()));

    let mut branch_prefix = KdlNode::new("branch-prefix");
    branch_prefix.push(KdlEntry::new(repo.branch_prefix.as_str()));

    let mut spares = KdlNode::new("spares");
    spares.push(KdlEntry::new(repo.spares as i128));

    let children = node.ensure_children();
    children.nodes_mut().push(trunk);
    children.nodes_mut().push(branch_prefix);
    children.nodes_mut().push(spares);
    if !repo.env.is_empty() {
        children.nodes_mut().push(build_env_node(&repo.env));
    }
    for step in &repo.steps {
        children.nodes_mut().push(build_step_node(step));
    }

    node
}

fn build_env_node(env: &BTreeMap<String, String>) -> KdlNode {
    let mut node = KdlNode::new("env");
    let children = node.ensure_children();
    for (key, value) in env {
        let mut child = KdlNode::new(key.as_str());
        child.push(KdlEntry::new(value.as_str()));
        children.nodes_mut().push(child);
    }
    node
}

fn build_step_node(step: &Step) -> KdlNode {
    let mut node = KdlNode::new("step");
    node.push(KdlEntry::new(step.label.as_str()));
    node.push(KdlEntry::new_prop("profile", step.profile.as_str()));
    node.push(KdlEntry::new_prop("cwd", step.cwd.as_str()));

    let mut cmd = KdlNode::new("cmd");
    for arg in &step.cmd {
        cmd.push(KdlEntry::new(arg.as_str()));
    }
    node.ensure_children().nodes_mut().push(cmd);

    node
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let tmp = parent.join(format!(
        "{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.kdl")
    ));
    let mut file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    // Without this, `rename` can land before the bytes do, so a crash
    // between the two can leave the config truncated.
    file.sync_all()
        .with_context(|| format!("syncing {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).with_context(|| format!("renaming {} into place", tmp.display()))?;
    Ok(())
}

// --- parsing ---

fn parse_document(text: &str) -> Result<KdlDocument> {
    text.parse::<KdlDocument>().map_err(render_kdl_error)
}

fn render_kdl_error(err: KdlError) -> anyhow::Error {
    anyhow!("{:?}", Report::new(err))
}

/// A span-anchored semantic error: the value parsed, but wt's own rules
/// reject it. Rendered through miette so the message carries a caret at the
/// offending line and column, same as a `kdl` syntax error.
#[derive(Debug)]
struct ConfigDiagnostic {
    src: String,
    span: SourceSpan,
    message: String,
}

impl std::fmt::Display for ConfigDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ConfigDiagnostic {}

impl miette::Diagnostic for ConfigDiagnostic {
    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.src)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::new_with_span(
            Some("here".to_string()),
            self.span,
        ))))
    }
}

fn config_error(text: &str, span: SourceSpan, message: impl Into<String>) -> anyhow::Error {
    let diagnostic = ConfigDiagnostic {
        src: text.to_string(),
        span,
        message: message.into(),
    };
    anyhow!("{:?}", Report::new(diagnostic))
}

fn positional_entries(node: &KdlNode) -> Vec<&KdlEntry> {
    node.entries()
        .iter()
        .filter(|e| e.name().is_none())
        .collect()
}

fn reject_properties(text: &str, node: &KdlNode) -> Result<()> {
    for entry in node.entries() {
        if let Some(key) = entry.name() {
            return Err(config_error(
                text,
                entry.span(),
                format!(
                    "unknown property '{}' on '{}'",
                    key.value(),
                    node.name().value()
                ),
            ));
        }
    }
    Ok(())
}

fn reject_children(text: &str, node: &KdlNode) -> Result<()> {
    if node.children().is_some() {
        return Err(config_error(
            text,
            node.span(),
            format!("'{}' does not take a block", node.name().value()),
        ));
    }
    Ok(())
}

fn single_string_arg(text: &str, node: &KdlNode) -> Result<String> {
    reject_properties(text, node)?;
    reject_children(text, node)?;
    let positional = positional_entries(node);
    if positional.len() != 1 {
        return Err(config_error(
            text,
            node.span(),
            format!(
                "'{}' expects exactly one string argument",
                node.name().value()
            ),
        ));
    }
    positional[0]
        .value()
        .as_string()
        .map(str::to_string)
        .ok_or_else(|| {
            config_error(
                text,
                positional[0].span(),
                format!(
                    "expected a string for '{}', found {}",
                    node.name().value(),
                    positional[0].value()
                ),
            )
        })
}

fn single_u8_arg(text: &str, node: &KdlNode) -> Result<u8> {
    reject_properties(text, node)?;
    reject_children(text, node)?;
    let positional = positional_entries(node);
    if positional.len() != 1 {
        return Err(config_error(
            text,
            node.span(),
            format!(
                "'{}' expects exactly one integer argument",
                node.name().value()
            ),
        ));
    }
    let entry = positional[0];
    let value = entry.value().as_integer().ok_or_else(|| {
        config_error(
            text,
            entry.span(),
            format!(
                "expected an integer for '{}', found {}",
                node.name().value(),
                entry.value()
            ),
        )
    })?;
    u8::try_from(value).map_err(|_| {
        config_error(
            text,
            entry.span(),
            format!(
                "'{}' must be between 0 and 255, found {value}",
                node.name().value()
            ),
        )
    })
}

fn parse_version_node(text: &str, node: &KdlNode) -> Result<i128> {
    reject_properties(text, node)?;
    reject_children(text, node)?;
    let positional = positional_entries(node);
    if positional.len() != 1 {
        return Err(config_error(
            text,
            node.span(),
            "'version' expects exactly one integer argument",
        ));
    }
    positional[0].value().as_integer().ok_or_else(|| {
        config_error(
            text,
            positional[0].span(),
            format!(
                "expected an integer for 'version', found {}",
                positional[0].value()
            ),
        )
    })
}

fn parse_env_block(text: &str, node: &KdlNode) -> Result<BTreeMap<String, String>> {
    reject_properties(text, node)?;
    if !positional_entries(node).is_empty() {
        return Err(config_error(
            text,
            node.span(),
            "'env' does not take arguments",
        ));
    }

    let mut map = BTreeMap::new();
    let Some(children) = node.children() else {
        return Ok(map);
    };
    for child in children.nodes() {
        reject_properties(text, child)?;
        reject_children(text, child)?;
        let positional = positional_entries(child);
        if positional.len() != 1 {
            return Err(config_error(
                text,
                child.span(),
                format!(
                    "env var '{}' expects exactly one string value",
                    child.name().value()
                ),
            ));
        }
        let value = positional[0].value().as_string().ok_or_else(|| {
            config_error(
                text,
                positional[0].span(),
                format!(
                    "expected a string value for env var '{}', found {}",
                    child.name().value(),
                    positional[0].value()
                ),
            )
        })?;
        let key = child.name().value().to_string();
        if map.contains_key(&key) {
            return Err(config_error(
                text,
                child.span(),
                format!("duplicate env key '{key}'"),
            ));
        }
        map.insert(key, value.to_string());
    }
    Ok(map)
}

fn parse_cmd_node(text: &str, node: &KdlNode) -> Result<Vec<String>> {
    reject_properties(text, node)?;
    reject_children(text, node)?;
    let entries = node.entries();
    if entries.is_empty() {
        return Err(config_error(
            text,
            node.span(),
            "'cmd' needs at least one argument",
        ));
    }
    let mut argv = Vec::with_capacity(entries.len());
    for entry in entries {
        let arg = entry.value().as_string().ok_or_else(|| {
            config_error(
                text,
                entry.span(),
                format!("expected a string in 'cmd', found {}", entry.value()),
            )
        })?;
        argv.push(arg.to_string());
    }
    Ok(argv)
}

/// Parses a hook block, e.g. `get-position { builtin "tmux-window" }`. Exactly
/// one of `builtin` or `cmd` is required; a `builtin` name must be one of
/// `valid_builtins`.
fn parse_hook_block(text: &str, node: &KdlNode, valid_builtins: &[&str]) -> Result<Hook> {
    reject_properties(text, node)?;
    if !positional_entries(node).is_empty() {
        return Err(config_error(
            text,
            node.span(),
            format!("'{}' does not take arguments", node.name().value()),
        ));
    }

    let mut builtin = None;
    let mut cmd = None;
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "builtin" => {
                    if builtin.is_some() || cmd.is_some() {
                        return Err(config_error(
                            text,
                            child.span(),
                            format!(
                                "'{}' may have only one of 'builtin' or 'cmd'",
                                node.name().value()
                            ),
                        ));
                    }
                    let name = single_string_arg(text, child)?;
                    if !valid_builtins.contains(&name.as_str()) {
                        return Err(config_error(
                            text,
                            child.span(),
                            format!(
                                "unknown builtin '{name}' for '{}'; valid builtins: {}",
                                node.name().value(),
                                valid_builtins.join(", ")
                            ),
                        ));
                    }
                    builtin = Some(name);
                }
                "cmd" => {
                    if builtin.is_some() || cmd.is_some() {
                        return Err(config_error(
                            text,
                            child.span(),
                            format!(
                                "'{}' may have only one of 'builtin' or 'cmd'",
                                node.name().value()
                            ),
                        ));
                    }
                    cmd = Some(parse_cmd_node(text, child)?);
                }
                other => {
                    return Err(config_error(
                        text,
                        child.span(),
                        format!("unknown node '{other}' inside '{}'", node.name().value()),
                    ));
                }
            }
        }
    }

    match (builtin, cmd) {
        (Some(b), None) => Ok(Hook::Builtin(b)),
        (None, Some(c)) => Ok(Hook::Cmd(c)),
        _ => Err(config_error(
            text,
            node.span(),
            format!(
                "'{}' needs exactly one of 'builtin' or 'cmd'",
                node.name().value()
            ),
        )),
    }
}

fn parse_planter_block(text: &str, node: &KdlNode) -> Result<Planter> {
    reject_properties(text, node)?;
    if !positional_entries(node).is_empty() {
        return Err(config_error(
            text,
            node.span(),
            "'planter' does not take arguments",
        ));
    }

    let mut planter = Planter::default();
    let Some(children) = node.children() else {
        return Ok(planter);
    };
    for child in children.nodes() {
        match child.name().value() {
            "get-position" => {
                planter.get_position = Some(parse_hook_block(text, child, &["tmux-window"])?);
            }
            "renumber-peers" => {
                planter.renumber_peers = Some(parse_hook_block(text, child, &["planter-state"])?);
            }
            other => {
                return Err(config_error(
                    text,
                    child.span(),
                    format!(
                        "unknown node '{other}' inside 'planter'; valid hooks: get-position, \
                         renumber-peers"
                    ),
                ));
            }
        }
    }
    Ok(planter)
}

fn parse_terminal_block(text: &str, node: &KdlNode) -> Result<Terminal> {
    reject_properties(text, node)?;
    if !positional_entries(node).is_empty() {
        return Err(config_error(
            text,
            node.span(),
            "'terminal' does not take arguments",
        ));
    }

    let mut terminal = Terminal::default();
    let Some(children) = node.children() else {
        return Ok(terminal);
    };
    for child in children.nodes() {
        match child.name().value() {
            "set-background" => {
                terminal.set_background = Some(parse_hook_block(text, child, &["osc11"])?);
            }
            other => {
                return Err(config_error(
                    text,
                    child.span(),
                    format!(
                        "unknown node '{other}' inside 'terminal'; valid hooks: set-background"
                    ),
                ));
            }
        }
    }
    Ok(terminal)
}

fn parse_features_block(text: &str, node: &KdlNode) -> Result<Features> {
    reject_properties(text, node)?;
    if !positional_entries(node).is_empty() {
        return Err(config_error(
            text,
            node.span(),
            "'features' does not take arguments",
        ));
    }

    let mut features = Features::default();
    let Some(children) = node.children() else {
        return Ok(features);
    };
    for child in children.nodes() {
        match child.name().value() {
            "planter" => features.planter = Some(parse_planter_block(text, child)?),
            "terminal" => features.terminal = Some(parse_terminal_block(text, child)?),
            other => {
                return Err(config_error(
                    text,
                    child.span(),
                    format!(
                        "unknown node '{other}' inside 'features'; valid features: planter, \
                         terminal"
                    ),
                ));
            }
        }
    }
    Ok(features)
}

fn parse_step_node(text: &str, node: &KdlNode) -> Result<Step> {
    let positional = positional_entries(node);
    if positional.len() != 1 {
        return Err(config_error(
            text,
            node.span(),
            "'step' expects exactly one label argument",
        ));
    }
    let label = positional[0]
        .value()
        .as_string()
        .ok_or_else(|| {
            config_error(
                text,
                positional[0].span(),
                format!(
                    "expected a string label for 'step', found {}",
                    positional[0].value()
                ),
            )
        })?
        .to_string();

    let mut profile = None;
    let mut cwd = None;
    for entry in node.entries() {
        let Some(key) = entry.name() else { continue };
        match key.value() {
            "profile" => {
                profile = Some(
                    entry
                        .value()
                        .as_string()
                        .ok_or_else(|| {
                            config_error(
                                text,
                                entry.span(),
                                format!("expected a string for 'profile', found {}", entry.value()),
                            )
                        })?
                        .to_string(),
                );
            }
            "cwd" => {
                cwd = Some(
                    entry
                        .value()
                        .as_string()
                        .ok_or_else(|| {
                            config_error(
                                text,
                                entry.span(),
                                format!("expected a string for 'cwd', found {}", entry.value()),
                            )
                        })?
                        .to_string(),
                );
            }
            other => {
                return Err(config_error(
                    text,
                    entry.span(),
                    format!("unknown property '{other}' on 'step'"),
                ));
            }
        }
    }
    let profile = profile.ok_or_else(|| {
        config_error(
            text,
            node.span(),
            format!("step '{label}' is missing required property 'profile'"),
        )
    })?;
    let cwd = cwd.ok_or_else(|| {
        config_error(
            text,
            node.span(),
            format!("step '{label}' is missing required property 'cwd'"),
        )
    })?;

    let mut cmd = None;
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "cmd" => {
                    if cmd.is_some() {
                        return Err(config_error(
                            text,
                            child.span(),
                            format!("step '{label}' has more than one 'cmd'"),
                        ));
                    }
                    cmd = Some(parse_cmd_node(text, child)?);
                }
                other => {
                    return Err(config_error(
                        text,
                        child.span(),
                        format!("unknown node '{other}' inside step '{label}'"),
                    ));
                }
            }
        }
    }
    let cmd = cmd.ok_or_else(|| {
        config_error(
            text,
            node.span(),
            format!("step '{label}' is missing a 'cmd' child"),
        )
    })?;

    Ok(Step {
        label,
        profile,
        cwd,
        cmd,
    })
}

fn parse_repo_node(text: &str, node: &KdlNode) -> Result<(String, RepoConfig)> {
    reject_properties(text, node)?;
    let positional = positional_entries(node);
    if positional.len() != 1 {
        return Err(config_error(
            text,
            node.span(),
            "'repo' expects exactly one string name argument",
        ));
    }
    let repo_name = positional[0]
        .value()
        .as_string()
        .ok_or_else(|| {
            config_error(
                text,
                positional[0].span(),
                format!(
                    "expected a string repo name, found {}",
                    positional[0].value()
                ),
            )
        })?
        .to_string();

    let mut trunk = None;
    let mut branch_prefix = String::new();
    let mut spares = default_spares();
    let mut env = BTreeMap::new();
    let mut steps = Vec::new();

    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "trunk" => trunk = Some(single_string_arg(text, child)?),
                "branch-prefix" => branch_prefix = single_string_arg(text, child)?,
                "spares" => spares = single_u8_arg(text, child)?,
                "env" => env = parse_env_block(text, child)?,
                "step" => steps.push(parse_step_node(text, child)?),
                other => {
                    return Err(config_error(
                        text,
                        child.span(),
                        format!("unknown node '{other}' inside repo '{repo_name}'"),
                    ));
                }
            }
        }
    }

    let trunk = trunk.ok_or_else(|| {
        config_error(
            text,
            node.span(),
            format!("repo '{repo_name}' is missing required 'trunk'"),
        )
    })?;

    Ok((
        repo_name,
        RepoConfig {
            trunk,
            branch_prefix,
            spares,
            env,
            steps,
        },
    ))
}

fn parse_config(text: &str) -> Result<Config> {
    let doc = parse_document(text)?;

    let mut version_seen = false;
    let mut config = Config::default();

    for node in doc.nodes() {
        match node.name().value() {
            "version" => {
                if version_seen {
                    return Err(config_error(text, node.span(), "duplicate 'version' node"));
                }
                version_seen = true;
                let found = parse_version_node(text, node)?;
                if found != CONFIG_VERSION {
                    return Err(config_error(
                        text,
                        node.span(),
                        format!(
                            "config version {found} is not supported; wt supports version {CONFIG_VERSION}"
                        ),
                    ));
                }
            }
            "env" => {
                config.env = parse_env_block(text, node)?;
            }
            "features" => {
                config.features = parse_features_block(text, node)?;
            }
            "repo" => {
                let (name, repo) = parse_repo_node(text, node)?;
                if config.repos.contains_key(&name) {
                    return Err(config_error(
                        text,
                        node.span(),
                        format!("duplicate repo block '{name}'"),
                    ));
                }
                config.repos.insert(name, repo);
            }
            other => {
                return Err(config_error(
                    text,
                    node.span(),
                    format!("unknown top-level node '{other}'"),
                ));
            }
        }
    }

    if !version_seen {
        let span = doc
            .nodes()
            .first()
            .map(|n| n.span())
            .unwrap_or_else(|| SourceSpan::new(0.into(), 0));
        return Err(config_error(
            text,
            span,
            format!("missing required 'version' node; wt supports version {CONFIG_VERSION}"),
        ));
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wt-config-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_repo() -> RepoConfig {
        let mut env = BTreeMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        RepoConfig {
            trunk: "master".to_string(),
            branch_prefix: "josh/".to_string(),
            spares: 1,
            env,
            steps: vec![
                Step {
                    label: "pnpm install".to_string(),
                    profile: "node".to_string(),
                    cwd: ".".to_string(),
                    cmd: vec![
                        "pnpm".to_string(),
                        "install".to_string(),
                        "--frozen-lockfile".to_string(),
                    ],
                },
                Step {
                    label: "uv sync".to_string(),
                    profile: "python".to_string(),
                    cwd: "python/datahub".to_string(),
                    cmd: vec!["uv".to_string(), "sync".to_string()],
                },
            ],
        }
    }

    #[test]
    fn round_trip_two_repos_with_env_and_steps() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");

        let mut global_env = BTreeMap::new();
        global_env.insert("RUSTC_WRAPPER".to_string(), "sccache".to_string());

        assert!(append_repo(&path, "monorepo", &sample_repo()).unwrap());
        let mut other = sample_repo();
        other.trunk = "main".to_string();
        other.branch_prefix = String::new();
        other.env.clear();
        other.steps.clear();
        assert!(append_repo(&path, "toy-apps", &other).unwrap());

        // Layer the global env block on by hand: append_repo only ever
        // writes `repo` blocks, so this exercises `load`'s own parsing of it.
        let text = fs::read_to_string(&path).unwrap();
        let text = format!("env {{\n    RUSTC_WRAPPER \"sccache\"\n}}\n\n{text}");
        fs::write(&path, text).unwrap();

        let config = load(&path).unwrap();
        assert_eq!(config.env, global_env);
        assert_eq!(config.repos.len(), 2);

        let monorepo = &config.repos["monorepo"];
        assert_eq!(monorepo.trunk, "master");
        assert_eq!(monorepo.branch_prefix, "josh/");
        assert_eq!(monorepo.spares, 1);
        assert_eq!(monorepo.env.get("FOO"), Some(&"bar".to_string()));
        assert_eq!(monorepo.steps.len(), 2);
        assert_eq!(monorepo.steps[0].label, "pnpm install");
        assert_eq!(monorepo.steps[0].profile, "node");
        assert_eq!(monorepo.steps[0].cwd, ".");
        assert_eq!(
            monorepo.steps[0].cmd,
            vec![
                "pnpm".to_string(),
                "install".to_string(),
                "--frozen-lockfile".to_string()
            ]
        );
        assert_eq!(monorepo.steps[1].label, "uv sync");

        let toy_apps = &config.repos["toy-apps"];
        assert_eq!(toy_apps.trunk, "main");
        assert_eq!(toy_apps.branch_prefix, "");
        assert!(toy_apps.env.is_empty());
        assert!(toy_apps.steps.is_empty());
    }

    #[test]
    fn missing_file_is_default_config() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let config = load(&path).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn repo_with_only_trunk_gets_defaults() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        fs::write(
            &path,
            "version 1\n\nrepo \"monorepo\" {\n    trunk \"master\"\n}\n",
        )
        .unwrap();

        let config = load(&path).unwrap();
        let repo = &config.repos["monorepo"];
        assert_eq!(repo.trunk, "master");
        assert_eq!(repo.branch_prefix, "");
        assert_eq!(repo.spares, 1);
        assert!(repo.env.is_empty());
        assert!(repo.steps.is_empty());
    }

    #[test]
    fn repo_lookup_on_an_unconfigured_repo_names_the_config_path_and_wt_init() {
        let config = Config::default();
        let err = repo(&config, "monorepo").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("monorepo"), "message was: {msg}");
        assert!(msg.contains("wt repo adopt"), "message was: {msg}");
        assert!(
            msg.contains(&config_path().display().to_string()),
            "message was: {msg}"
        );
    }

    #[test]
    fn append_repo_preserves_hand_written_comments_and_spacing() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let original = "// personal notes, don't touch\nversion 1\n\n\n// weird spacing on purpose\nrepo \"existing\" {\n    trunk    \"main\"\n    spares 0\n}\n";
        fs::write(&path, original).unwrap();

        assert!(append_repo(&path, "monorepo", &sample_repo()).unwrap());

        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.starts_with(original),
            "existing bytes were disturbed:\n{written}"
        );
        assert!(written.contains("repo \"monorepo\""));

        let config = load(&path).unwrap();
        assert_eq!(config.repos["existing"].spares, 0);
        assert_eq!(config.repos["monorepo"].trunk, "master");
    }

    #[test]
    fn append_repo_is_a_noop_when_the_repo_already_has_a_block() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        assert!(append_repo(&path, "monorepo", &sample_repo()).unwrap());
        let before = fs::read_to_string(&path).unwrap();

        let mut different = sample_repo();
        different.trunk = "should-not-appear".to_string();
        assert!(!append_repo(&path, "monorepo", &different).unwrap());

        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn append_repo_on_a_missing_file_creates_a_valid_file() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        assert!(append_repo(&path, "monorepo", &sample_repo()).unwrap());

        let config = load(&path).unwrap();
        assert_eq!(config.repos.len(), 1);
        assert_eq!(config.repos["monorepo"], sample_repo());

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("wt's user config"));
        assert!(text.contains("version 1"));
    }

    #[test]
    fn replace_repo_steps_swaps_steps_and_keeps_everything_else() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let original = "version 1\n\n// keep me\nrepo \"monorepo\" {\n    trunk \"master\"\n    spares 3\n    step \"old step\" profile=\"node\" cwd=\".\" {\n        cmd \"old\"\n    }\n}\n";
        fs::write(&path, original).unwrap();

        let new_steps = vec![Step {
            label: "new step".to_string(),
            profile: "python".to_string(),
            cwd: "python".to_string(),
            cmd: vec!["uv".to_string(), "sync".to_string()],
        }];
        replace_repo_steps(&path, "monorepo", &new_steps).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("// keep me"));
        assert!(!written.contains("old step"));
        assert!(written.contains("new step"));

        let config = load(&path).unwrap();
        let repo = &config.repos["monorepo"];
        assert_eq!(repo.trunk, "master");
        assert_eq!(repo.spares, 3);
        assert_eq!(repo.steps.len(), 1);
        assert_eq!(repo.steps[0].label, "new step");
        assert_eq!(
            repo.steps[0].cmd,
            vec!["uv".to_string(), "sync".to_string()]
        );
    }

    #[test]
    fn replace_repo_steps_errors_on_an_unknown_repo() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        fs::write(&path, "version 1\n").unwrap();

        let err = replace_repo_steps(&path, "nope", &[]).unwrap_err();
        assert!(err.to_string().contains("no repo block named 'nope'"));
    }

    #[test]
    fn set_repo_spares_updates_an_existing_value_and_keeps_everything_else() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let original = "// personal notes, don't touch\nversion 1\n\n\
                         // weird spacing on purpose\n\
                         repo \"monorepo\" {\n    trunk    \"main\"\n    spares 1\n}\n";
        fs::write(&path, original).unwrap();

        set_repo_spares(&path, "monorepo", 0).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("// personal notes, don't touch"));
        assert!(written.contains("// weird spacing on purpose"));
        assert!(written.contains("trunk    \"main\""));
        assert!(written.contains("spares 0"));
        assert!(!written.contains("spares 1"));

        let config = load(&path).unwrap();
        assert_eq!(config.repos["monorepo"].spares, 0);
        assert_eq!(config.repos["monorepo"].trunk, "main");
    }

    #[test]
    fn set_repo_spares_adds_a_child_when_the_block_has_none() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let original = "version 1\n\nrepo \"monorepo\" {\n    trunk \"master\"\n    branch-prefix \"josh/\"\n}\n";
        fs::write(&path, original).unwrap();

        set_repo_spares(&path, "monorepo", 0).unwrap();

        let config = load(&path).unwrap();
        let repo = &config.repos["monorepo"];
        assert_eq!(repo.spares, 0);
        assert_eq!(repo.trunk, "master");
        assert_eq!(repo.branch_prefix, "josh/");
    }

    #[test]
    fn set_repo_spares_errors_on_an_unknown_repo() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        fs::write(&path, "version 1\n").unwrap();

        let err = set_repo_spares(&path, "nope", 0).unwrap_err();
        assert!(err.to_string().contains("no repo block named 'nope'"));
    }

    fn line_of(text: &str, needle: &str) -> usize {
        for (i, line) in text.lines().enumerate() {
            if line.contains(needle) {
                return i + 1;
            }
        }
        panic!("'{needle}' not found in:\n{text}");
    }

    #[test]
    fn syntax_error_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nrepo \"monorepo\" {\n    trunk \"master\"\n    spares 1 {{{\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "spares 1");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
    }

    #[test]
    fn unknown_node_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nbogus 1\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "bogus 1");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("unknown top-level node"));
    }

    #[test]
    fn unknown_property_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nrepo \"monorepo\" {\n    trunk \"master\"\n    step \"s\" profile=\"node\" cwd=\".\" oops=\"x\" {\n        cmd \"a\"\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "oops=\"x\"");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("unknown property"));
    }

    #[test]
    fn missing_trunk_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nrepo \"monorepo\" {\n    spares 1\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "repo \"monorepo\"");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("missing required 'trunk'"));
    }

    #[test]
    fn missing_version_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "// no version here\nrepo \"monorepo\" {\n    trunk \"master\"\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "repo \"monorepo\"");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("missing required 'version'"));
    }

    #[test]
    fn wrong_version_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 2\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "version 2");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("config version 2 is not supported"));
    }

    #[test]
    fn parses_a_full_features_block() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nfeatures {\n    planter {\n        get-position { builtin \"tmux-window\" }\n        renumber-peers { cmd \"~/bin/iterm-tab-index\" }\n    }\n    terminal {\n        set-background { builtin \"osc11\" }\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let config = load(&path).unwrap();
        let planter = config.features.planter.unwrap();
        assert_eq!(
            planter.get_position,
            Some(Hook::Builtin("tmux-window".to_string()))
        );
        assert_eq!(
            planter.renumber_peers,
            Some(Hook::Cmd(vec!["~/bin/iterm-tab-index".to_string()]))
        );
        let terminal = config.features.terminal.unwrap();
        assert_eq!(
            terminal.set_background,
            Some(Hook::Builtin("osc11".to_string()))
        );
    }

    #[test]
    fn absent_features_block_is_default() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        fs::write(&path, "version 1\n").unwrap();

        let config = load(&path).unwrap();
        assert_eq!(config.features, Features::default());
    }

    #[test]
    fn planter_alone_leaves_terminal_absent() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nfeatures {\n    planter {\n        get-position { builtin \"tmux-window\" }\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let config = load(&path).unwrap();
        assert!(config.features.planter.is_some());
        assert!(config.features.terminal.is_none());
    }

    #[test]
    fn terminal_alone_leaves_planter_absent() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nfeatures {\n    terminal {\n        set-background { builtin \"osc11\" }\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let config = load(&path).unwrap();
        assert!(config.features.terminal.is_some());
        assert!(config.features.planter.is_none());
    }

    #[test]
    fn hook_with_both_builtin_and_cmd_errors() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nfeatures {\n    planter {\n        get-position {\n            builtin \"tmux-window\"\n            cmd \"x\"\n        }\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("only one of 'builtin' or 'cmd'"),
            "message was:\n{msg}"
        );
    }

    #[test]
    fn hook_with_neither_builtin_nor_cmd_errors() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text =
            "version 1\n\nfeatures {\n    planter {\n        get-position {\n        }\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("needs exactly one of 'builtin' or 'cmd'"),
            "message was:\n{msg}"
        );
    }

    #[test]
    fn unknown_feature_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nfeatures {\n    bogus {\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "bogus {");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("unknown node 'bogus'"));
    }

    #[test]
    fn unknown_hook_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text =
            "version 1\n\nfeatures {\n    planter {\n        bogus-hook {\n        }\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "bogus-hook {");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("unknown node 'bogus-hook'"));
    }

    #[test]
    fn unknown_builtin_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text = "version 1\n\nfeatures {\n    planter {\n        get-position { builtin \"ghostty-tab\" }\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "builtin \"ghostty-tab\"");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("unknown builtin 'ghostty-tab'"));
    }

    #[test]
    fn empty_cmd_names_the_line() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let text =
            "version 1\n\nfeatures {\n    planter {\n        get-position { cmd }\n    }\n}\n";
        fs::write(&path, text).unwrap();

        let err = load(&path).unwrap_err();
        let msg = format!("{err:#}");
        let line = line_of(text, "get-position { cmd }");
        assert!(msg.contains(&line.to_string()), "message was:\n{msg}");
        assert!(msg.contains("needs at least one argument"));
    }

    #[test]
    fn features_block_survives_append_repo_and_set_repo_spares() {
        let dir = temp_dir();
        let path = dir.join("config.kdl");
        let original = "version 1\n\nfeatures {\n    planter {\n        get-position { builtin \"tmux-window\" }\n    }\n}\n";
        fs::write(&path, original).unwrap();

        assert!(append_repo(&path, "monorepo", &sample_repo()).unwrap());
        set_repo_spares(&path, "monorepo", 0).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("features {"));
        assert!(written.contains("get-position"));

        let config = load(&path).unwrap();
        assert!(config.features.planter.is_some());
        assert_eq!(
            config.features.planter.unwrap().get_position,
            Some(Hook::Builtin("tmux-window".to_string()))
        );
        assert_eq!(config.repos["monorepo"].spares, 0);
    }
}
