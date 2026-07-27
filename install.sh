#!/usr/bin/env bash
# Idempotent installer for wt. Safe to re-run: every step no-ops if already applied.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ZSHRC="${ZSHRC:-${HOME}/.zshrc}"
MARKER_BEGIN="# >>> wt-cli shell integration >>>"
MARKER_END="# <<< wt-cli shell integration <<<"

echo "==> Installing wt (cargo install --path .)"
if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo not found on PATH. Install Rust (https://rustup.rs) and re-run this script." >&2
  exit 1
fi
cargo install --path "${REPO_ROOT}"

echo "==> Wiring the wt() shell function into ${ZSHRC}"
if [[ -f "${ZSHRC}" ]] && grep -qF "${MARKER_BEGIN}" "${ZSHRC}"; then
  echo "    already installed, no-op"
else
  cat >> "${ZSHRC}" <<EOF

${MARKER_BEGIN}
# No child process can change its parent shell's directory, so \`wt go\`/
# \`wt cd\` are resolved here and everything else forwards to the real binary.
wt() {
  if [[ "\$1" == "go" || "\$1" == "cd" ]]; then
    shift
    local target
    target="\$(command wt path "\$@")" || return \$?
    cd "\${target}"
  else
    command wt "\$@"
  fi
}
${MARKER_END}
EOF
  echo "    installed"
fi

echo
echo "==> Verification"
echo "    wt on PATH: $(command -v wt || echo 'NOT FOUND (open a new shell or: hash -r)')"
if grep -qF "${MARKER_BEGIN}" "${ZSHRC}" 2>/dev/null; then
  echo "    ${ZSHRC}: shell function present"
else
  echo "    ${ZSHRC}: shell function MISSING"
fi

echo
echo "Next: open a new shell, or run 'source ${ZSHRC}', to pick up 'wt go'/'wt cd'."
