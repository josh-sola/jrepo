# List the available recipes.
default:
    @just --list

# Run every repository check.
check: fmt clippy test bash swift

# Check Rust formatting.
fmt:
    cargo fmt --check --all

# Lint the Rust workspace.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the Rust test suite.
test:
    RUST_TEST_THREADS=4 cargo test --workspace

# Check shell-script syntax.
bash:
    bash -n wt-cli/install.sh wt-cli/shell-integration.sh wt-cli/tests/shell-integration.sh wt-cli/hooks/session-context.sh claude-planter/install.sh claude-planter/login-item.sh claude-planter/planter-state ghostty/install.sh
    bash wt-cli/tests/shell-integration.sh

# Typecheck and self-test the Swift components.
swift:
    #!/usr/bin/env zsh
    set -euo pipefail
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT
    swiftc -typecheck claude-planter/main.swift claude-planter/Planter.swift claude-planter/Sprites.swift
    swiftc -typecheck claude-planter/PlanterCodexBridge.swift
    swiftc -O -o "$tmpdir/planter" claude-planter/main.swift claude-planter/Planter.swift claude-planter/Sprites.swift
    swiftc -O -o "$tmpdir/planter-codex-bridge" claude-planter/PlanterCodexBridge.swift
    "$tmpdir/planter-codex-bridge" --self-test

install:
    ./ghostty/install.sh
    ./wt-cli/install.sh
    ./claude-planter/install.sh
    ./claude-planter/login-item.sh install
