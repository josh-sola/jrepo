#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "${1:-}" == "--uninstall" ]]; then
  if [[ $# -ne 1 ]]; then
    echo "Usage: ./claudex/install.sh [--uninstall]" >&2
    exit 2
  fi
  if ! command -v uv >/dev/null 2>&1; then
    echo "claudex needs uv. Install uv, then run this command again." >&2
    exit 1
  fi
  if uv tool list | grep -q '^jrepo-claudex '; then
    uv tool uninstall jrepo-claudex
  fi
  echo "Removed the claudex tool. Your ChatGPT login and configuration remain in ~/.config/claudex and ~/.local/state/claudex."
  exit 0
fi

if [[ $# -ne 0 ]]; then
  echo "Usage: ./claudex/install.sh [--uninstall]" >&2
  exit 2
fi

case "$(uname -s)" in
  Darwin|Linux) ;;
  *)
    echo "claudex supports macOS and Linux." >&2
    exit 1
    ;;
esac

if ! command -v uv >/dev/null 2>&1; then
  echo "claudex needs uv. Install uv, then run this command again." >&2
  exit 1
fi

uv tool install --reinstall "$project_dir"

if ! command -v claudex >/dev/null 2>&1; then
  echo "claudex installed, but it is not on PATH. Run 'uv tool update-shell', open a new shell, then run 'claudex doctor'." >&2
  exit 1
fi

echo "claudex is installed. Run 'claudex login', then 'claudex doctor'."
