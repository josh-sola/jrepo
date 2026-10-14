# List the available recipes.
default:
    @just --list

# Run every repository check.
check: fmt clippy test bash swift check-palette panopticon

# Check Rust formatting.
fmt:
    cargo fmt --check --all

# Lint the Rust workspace.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Run the Rust test suite.
test:
    RUST_TEST_THREADS=4 cargo test --workspace

# Verify every palette copy matches palette.json.
check-palette:
    ./tools/check-palette

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
    swiftc -typecheck claude-planter/main.swift claude-planter/Planter.swift claude-planter/Palette.swift claude-planter/Sprites.swift
    swiftc -typecheck claude-planter/Planter.swift claude-planter/Palette.swift claude-planter/PlanterTests.swift
    swiftc -typecheck claude-planter/PlanterCodexBridge.swift
    swiftc -O -o "$tmpdir/planter" claude-planter/main.swift claude-planter/Planter.swift claude-planter/Palette.swift claude-planter/Sprites.swift
    swiftc -O -o "$tmpdir/planter-tests" claude-planter/Planter.swift claude-planter/Palette.swift claude-planter/PlanterTests.swift
    swiftc -O -o "$tmpdir/planter-codex-bridge" claude-planter/PlanterCodexBridge.swift
    "$tmpdir/planter-tests" "$tmpdir/planter"
    "$tmpdir/planter-codex-bridge" --self-test

# Install dependencies and run panopticon's checks.
panopticon:
    cd panopticon && bun install --frozen-lockfile && bun run check
    helm lint panopticon/deploy/chart --set image.tag=lint

install:
    ./ghostty/install.sh
    ./wt-cli/install.sh
    ./claude-planter/install.sh
    ./claude-planter/login-item.sh install
    ./panopticon/install.sh install

# Image tag for the current commit; refuses to build from uncommitted panopticon changes.
[private]
panopticon-tag:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "$(git status --porcelain -- panopticon)" ]]; then
        echo "error: commit or stash panopticon changes before building an image" >&2
        exit 1
    fi
    git rev-parse --short HEAD

# Build the panopticon image for the local cluster, tagged with the git short sha.
panopticon-image:
    docker build -t "panopticon:$(just panopticon-tag)" panopticon/

# Create or update the GitHub token Secret that panopticon reads.
panopticon-secret:
    kubectl --context orbstack create namespace panopticon --dry-run=client -o yaml | kubectl --context orbstack apply -f -
    gh auth token | kubectl --context orbstack -n panopticon create secret generic panopticon-github --from-file=token=/dev/stdin --dry-run=client -o yaml | kubectl --context orbstack apply -f -

# Build the panopticon image and apply every release to the local cluster.
k8s-deploy: panopticon-image
    PANOPTICON_IMAGE_TAG="$(just panopticon-tag)" helmfile apply
