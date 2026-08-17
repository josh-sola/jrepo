# jrepo

Personal projects that share one checkout and one Git history.

| Project | What it does |
| --- | --- |
| [wt-cli](wt-cli/) | Git worktree tooling for creating and managing prepared worktrees. |
| [claude-planter](claude-planter/) | A macOS overlay that shows the state of coding sessions as plants. |

## Validate

Run these from the repository root:

```sh
cargo fmt --check --all
cargo clippy --workspace --all-targets -- -D warnings
RUST_TEST_THREADS=4 cargo test --workspace
bash -n wt-cli/install.sh wt-cli/hooks/session-context.sh \
  claude-planter/install.sh claude-planter/login-item.sh claude-planter/planter-state
swiftc -typecheck claude-planter/main.swift claude-planter/Planter.swift claude-planter/Sprites.swift
swiftc -typecheck claude-planter/PlanterCodexBridge.swift
tmpdir="$(mktemp -d)"
swiftc -O -o "$tmpdir/planter" claude-planter/main.swift claude-planter/Planter.swift claude-planter/Sprites.swift
swiftc -O -o "$tmpdir/planter-codex-bridge" claude-planter/PlanterCodexBridge.swift
"$tmpdir/planter-codex-bridge" --self-test
rm -rf "$tmpdir"
```

## Install

Install `wt-cli` from a long-lived checkout. Its installer records absolute
paths for its Claude hook and plugin symlink, so moving or deleting that
checkout later breaks those integrations.

```sh
git clone git@github.com:josh-sola/jrepo.git
cd jrepo
./wt-cli/install.sh
./claude-planter/install.sh
./claude-planter/login-item.sh install
```
