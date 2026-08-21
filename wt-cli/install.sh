#!/usr/bin/env bash
# Idempotent installer for wt. Safe to re-run: every step no-ops if already applied.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ZSHRC="${ZSHRC:-${HOME}/.zshrc}"
source "${REPO_ROOT}/shell-integration.sh"

CLAUDE_DIR="${WT_CLAUDE_DIR:-${HOME}/.claude}"
SETTINGS_FILE="${CLAUDE_DIR}/settings.json"
SKILL_LINK="${CLAUDE_DIR}/skills/wt"
HOOK_COMMAND="${REPO_ROOT}/hooks/session-context.sh"

LAUNCHAGENTS_DIR="${WT_LAUNCHAGENTS_DIR:-${HOME}/Library/LaunchAgents}"
SYNC_LOG_DIR="${WT_SYNC_LOG_DIR:-${HOME}/repos/wt}"
LAUNCHAGENT_LABEL="com.joshbassin.wt.sync"
LAUNCHAGENT_PLIST="${LAUNCHAGENTS_DIR}/${LAUNCHAGENT_LABEL}.plist"

chmod +x "${REPO_ROOT}/install.sh" "${HOOK_COMMAND}"

echo "==> Installing wt (cargo install --path .)"
if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo not found on PATH. Install Rust (https://rustup.rs) and re-run this script." >&2
  exit 1
fi
cargo install --path "${REPO_ROOT}"
WT_BIN="$(command -v wt || true)"
if [[ -z "${WT_BIN}" ]]; then
  echo "ERROR: wt was installed but is not on PATH; add cargo's bin dir to PATH and re-run." >&2
  exit 1
fi

echo "==> Wiring the wt() shell function into ${ZSHRC}"
wt_replace_shell_integration "${ZSHRC}"
echo "    ${WT_SHELL_INTEGRATION_ACTION}"

echo "==> Symlinking ${SKILL_LINK} -> ${REPO_ROOT}/plugin"
mkdir -p "${CLAUDE_DIR}/skills"
if [[ -L "${SKILL_LINK}" ]]; then
  current_target="$(readlink "${SKILL_LINK}")"
  if [[ "${current_target}" == "${REPO_ROOT}/plugin" ]]; then
    echo "    already correct, no-op"
  elif [[ "${current_target}" == "wt-cli/plugin" || "${current_target}" == */wt-cli/plugin ]]; then
    ln -sfn "${REPO_ROOT}/plugin" "${SKILL_LINK}"
    echo "    replaced prior wt-cli symlink"
  else
    echo "    WARNING: ${SKILL_LINK} already exists and points elsewhere (${current_target}) — skipping, not clobbering"
  fi
elif [[ -e "${SKILL_LINK}" ]]; then
  echo "    WARNING: ${SKILL_LINK} exists and is not a symlink — skipping, not clobbering"
else
  ln -s "${REPO_ROOT}/plugin" "${SKILL_LINK}"
fi

echo "==> Wiring SessionStart/CwdChanged hooks into ${SETTINGS_FILE}"
mkdir -p "${CLAUDE_DIR}"
if [[ ! -f "${SETTINGS_FILE}" ]]; then
  echo '{"hooks":{}}' > "${SETTINGS_FILE}"
fi
cp "${SETTINGS_FILE}" "${SETTINGS_FILE}.bak"

already_installed() {
  jq --arg event "$1" --arg cmd "${HOOK_COMMAND}" '
    [.hooks[$event][]?.hooks[]?.command? // "" | select(. == $cmd)] | length == 1
  ' "${SETTINGS_FILE}"
}

tmp_file="$(mktemp)"
jq --arg cmd "${HOOK_COMMAND}" '
  def is_own:
    (.command // "") | endswith("/wt-cli/hooks/session-context.sh");

  def strip_own:
    map(.hooks = ((.hooks // []) | map(select(is_own | not))))
    | map(select((.hooks // []) | length > 0));

  def wire($event; $entry):
    .hooks[$event] = ((.hooks[$event] // []) | strip_own) + [$entry];

  .hooks //= {}
  | wire("SessionStart"; {
      "matcher": "startup|resume|clear|compact",
      "hooks": [{"type": "command", "command": $cmd}]
    })
  | wire("CwdChanged"; {"hooks": [{"type": "command", "command": $cmd}]})
' "${SETTINGS_FILE}" > "${tmp_file}"
mv "${tmp_file}" "${SETTINGS_FILE}"
echo "    SessionStart hook installed: $(already_installed SessionStart)"
echo "    CwdChanged hook installed: $(already_installed CwdChanged)"

echo "==> Writing the LaunchAgent plist (not loading it)"
mkdir -p "${LAUNCHAGENTS_DIR}" "${SYNC_LOG_DIR}"
plist_tmp="$(mktemp)"
cat > "${plist_tmp}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>${LAUNCHAGENT_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>${WT_BIN}</string>
		<string>repo</string>
		<string>sync</string>
	</array>
	<key>StartInterval</key>
	<integer>300</integer>
	<key>StandardOutPath</key>
	<string>${SYNC_LOG_DIR}/wt-sync.log</string>
	<key>StandardErrorPath</key>
	<string>${SYNC_LOG_DIR}/wt-sync.err.log</string>
	<key>RunAtLoad</key>
	<false/>
	<!-- sync spawns hot spare builds that outlive it by design; without
	     this launchd kills them the moment sync exits, so a build long
	     enough to matter never finishes and is retried every interval. -->
	<key>AbandonProcessGroup</key>
	<true/>
	<!-- A fetch plus a dependency install competes with whatever the user
	     is doing; this keeps an unattended sync off the foreground's back. -->
	<key>ProcessType</key>
	<string>Background</string>
	<key>LowPriorityIO</key>
	<true/>
</dict>
</plist>
EOF
if command -v plutil >/dev/null 2>&1; then
  plutil -lint "${plist_tmp}"
fi
if [[ -f "${LAUNCHAGENT_PLIST}" ]] && diff -q "${plist_tmp}" "${LAUNCHAGENT_PLIST}" >/dev/null 2>&1; then
  echo "    ${LAUNCHAGENT_PLIST} already up to date, no-op"
  rm -f "${plist_tmp}"
else
  mv "${plist_tmp}" "${LAUNCHAGENT_PLIST}"
  echo "    written ${LAUNCHAGENT_PLIST}"
fi
echo "    NOT loaded. To activate: launchctl bootstrap gui/$(id -u) ${LAUNCHAGENT_PLIST}"

echo
echo "==> Verification"
echo "    wt on PATH: ${WT_BIN}"
if grep -qF "${WT_SHELL_MARKER_BEGIN}" "${ZSHRC}" 2>/dev/null; then
  echo "    ${ZSHRC}: shell function present"
else
  echo "    ${ZSHRC}: shell function MISSING"
fi
echo "    skill symlink target: $(readlink "${SKILL_LINK}" 2>/dev/null || echo 'NOT A SYMLINK')"
if jq . "${SETTINGS_FILE}" >/dev/null 2>&1; then
  echo "    ${SETTINGS_FILE}: valid JSON"
else
  echo "    ${SETTINGS_FILE}: INVALID JSON"
fi
echo "    SessionStart hook installed: $(already_installed SessionStart)"
echo "    CwdChanged hook installed: $(already_installed CwdChanged)"
echo "    LaunchAgent plist: ${LAUNCHAGENT_PLIST} ($(command -v plutil >/dev/null 2>&1 && plutil -lint "${LAUNCHAGENT_PLIST}" || echo 'plutil not available'))"

echo
echo "Next: open a new shell, or run 'source ${ZSHRC}', so 'wt go' reaches the binary and 'wt cd' changes this shell."
echo "The LaunchAgent is written but not active; run the bootstrap command above when ready."
