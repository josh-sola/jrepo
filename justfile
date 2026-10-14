# List the available recipes.
default:
    @just --list

# Run every repository check.
check: bash gt-api panopticon

# Check shell-script syntax.
bash:
    bash -n ghostty/install.sh

# Install dependencies and run gt-api's checks.
gt-api:
    cd gt-api && bun install --frozen-lockfile && bun run check
    helm lint gt-api/deploy/chart --set image.tag=lint

# Install dependencies and run panopticon's checks.
panopticon:
    cd panopticon && bun install --frozen-lockfile && bun run check && bun run gen:gtapi:check
    helm lint panopticon/deploy/chart --set image.tag=lint

install:
    ./ghostty/install.sh

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

# Image tag for the current commit; refuses to build from uncommitted gt-api changes.
[private]
gt-api-tag:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "$(git status --porcelain -- gt-api)" ]]; then
        echo "error: commit or stash gt-api changes before building an image" >&2
        exit 1
    fi
    git rev-parse --short HEAD

# Build the gt-api image for the local cluster, tagged with the git short sha.
gt-api-image:
    docker build -t "gt-api:$(just gt-api-tag)" gt-api/

# Create or update the GitHub token Secret that gt-api reads.
gt-api-secret:
    kubectl --context orbstack create namespace gt-api --dry-run=client -o yaml | kubectl --context orbstack apply -f -
    gh auth token | kubectl --context orbstack -n gt-api create secret generic gt-api-github --from-file=token=/dev/stdin --dry-run=client -o yaml | kubectl --context orbstack apply -f -

# Build the gt-api and panopticon images and apply every release to the local cluster.
k8s-deploy: gt-api-image panopticon-image
    GT_API_IMAGE_TAG="$(just gt-api-tag)" PANOPTICON_IMAGE_TAG="$(just panopticon-tag)" helmfile apply
