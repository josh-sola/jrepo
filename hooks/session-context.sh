#!/usr/bin/env bash
# Wired to Claude Code's SessionStart and CwdChanged hooks (see
# ~/.claude/settings.json). Resolves the event's directory through `wt` and
# surfaces where plans live, or that the directory is a repo's base.
#
# CwdChanged cannot inject model context (no additionalContext support,
# unlike SessionStart) — it only supports a user-facing systemMessage — so
# the same text is delivered differently depending on which event fired.
#
# Must be fast and must never block a session: any failure anywhere in this
# script still results in an empty-but-valid exit 0.
set -euo pipefail

main() {
  local input event path block
  input="$(cat)"
  event="$(jq -r '.hook_event_name // "SessionStart"' <<<"${input}")"
  path="$(jq -r '.cwd // empty' <<<"${input}")"
  [[ -n "${path}" ]] || path="${PWD}"

  block="$(wt __session-context --path "${path}" 2>/dev/null)" || return 0
  [[ -n "${block}" ]] || return 0

  if [[ "${event}" == "CwdChanged" ]]; then
    jq -n --arg msg "${block}" '{"systemMessage":$msg}'
  else
    jq -n --arg block "${block}" \
      '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":$block}}'
  fi
}

main || true
exit 0
