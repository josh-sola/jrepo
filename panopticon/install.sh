#!/usr/bin/env bash
# Installs panopticon as a login LaunchAgent, following claude-planter's
# login-item.sh. Safe to re-run: every step replaces what it wrote before.
#
#   ./install.sh install [--dry-run]   build and load the agent
#   ./install.sh uninstall             stop it and remove the plist
#   ./install.sh status                what launchd thinks
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LABEL="local.panopticon"
PLIST_TEMPLATE="${REPO_ROOT}/launchd/local.panopticon.plist.tmpl"
PLIST_PATH="${HOME}/Library/LaunchAgents/${LABEL}.plist"
OUT_LOG="${HOME}/Library/Logs/panopticon.out.log"
ERR_LOG="${HOME}/Library/Logs/panopticon.err.log"
DOMAIN="gui/$(id -u)"

# Overridable so a dry run can point at a throwaway file instead of the real
# one; the server itself honors the same variable.
CONFIG_PATH="${PANOPTICON_CONFIG:-${HOME}/.config/panopticon/config.json}"

fail() {
  echo "error: $1" >&2
  exit 1
}

render_plist() {
  local bun_path="$1"
  local content
  content="$(cat "${PLIST_TEMPLATE}")"
  content="${content//__BUN_PATH__/${bun_path}}"
  content="${content//__WORKDIR__/${REPO_ROOT}}"
  content="${content//__OUT_LOG__/${OUT_LOG}}"
  content="${content//__ERR_LOG__/${ERR_LOG}}"
  # launchd starts agents with only /usr/bin:/bin:/usr/sbin:/sbin, which has
  # none of gh, difft, wt, or the hover language servers. Freeze the
  # installer's PATH into the plist instead.
  content="${content//__PATH__/${PATH}}"
  printf '%s' "${content}"
}

cmd_install() {
  local dry_run="${1:-}"

  if [[ ! -f "${CONFIG_PATH}" ]]; then
    fail "panopticon config not found at ${CONFIG_PATH}. Copy ${REPO_ROOT}/config.example.json there and fill it in first."
  fi

  local bun_path
  bun_path="$(command -v bun || true)"
  [[ -n "${bun_path}" ]] || fail "bun not found on PATH"

  local plist
  plist="$(render_plist "${bun_path}")"

  if [[ "${dry_run}" == "--dry-run" ]]; then
    echo "==> Would write ${PLIST_PATH}:"
    echo "${plist}"
    echo
    echo "==> Would run:"
    echo "    cd ${REPO_ROOT} && bun install --frozen-lockfile"
    echo "    cd ${REPO_ROOT} && bun run build"
    echo "    mkdir -p $(dirname "${PLIST_PATH}") $(dirname "${OUT_LOG}")"
    echo "    launchctl bootout ${DOMAIN}/${LABEL}"
    echo "    launchctl bootstrap ${DOMAIN} ${PLIST_PATH}"
    return 0
  fi

  echo "==> Installing dependencies"
  (cd "${REPO_ROOT}" && bun install --frozen-lockfile)

  echo "==> Building"
  (cd "${REPO_ROOT}" && bun run build)

  echo "==> Writing ${PLIST_PATH}"
  mkdir -p "$(dirname "${PLIST_PATH}")" "$(dirname "${OUT_LOG}")"
  printf '%s' "${plist}" >"${PLIST_PATH}"
  if command -v plutil >/dev/null 2>&1; then
    plutil -lint "${PLIST_PATH}"
  fi

  echo "==> Loading the LaunchAgent"
  launchctl bootout "${DOMAIN}/${LABEL}" 2>/dev/null || true
  launchctl bootstrap "${DOMAIN}" "${PLIST_PATH}"
  echo "installed ${PLIST_PATH}"
  echo "logs: ${OUT_LOG} and ${ERR_LOG}"
}

cmd_uninstall() {
  launchctl bootout "${DOMAIN}/${LABEL}" 2>/dev/null || true
  rm -f "${PLIST_PATH}"
  echo "removed ${PLIST_PATH}"
}

cmd_status() {
  launchctl print "${DOMAIN}/${LABEL}" 2>/dev/null | grep -E '^\s*(state|pid|program|last exit) ' ||
    echo "not loaded"
}

case "${1:-install}" in
install)
  cmd_install "${2:-}"
  ;;
uninstall)
  cmd_uninstall
  ;;
status)
  cmd_status
  ;;
*)
  echo "usage: $0 [install [--dry-run]|uninstall|status]" >&2
  exit 1
  ;;
esac
