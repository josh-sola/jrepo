#!/bin/bash
# Builds the overlay, installs the hook, and wires it into ~/.claude/settings.json.
# Safe to re-run: it replaces its own hook entries rather than stacking them up.
#
#   ./install.sh              install or update
#   ./install.sh --uninstall  remove the hooks, the hook script, and the binary
set -euo pipefail

src="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="$HOME/.local/bin"
binary="$bin_dir/planter"
hook="$HOME/.claude/hooks/planter-state"
settings="$HOME/.claude/settings.json"

# Matches this project's own hook entries, including those written when it was
# called claude-pets, so an upgrade replaces them instead of running both.
own_hooks='(planter-state|pet-state)'

backup_settings() {
    local backup="$settings.bak.$(date +%Y%m%d%H%M%S)"
    cp "$settings" "$backup"
    echo "backed up settings to $backup"
}

if [[ "${1:-}" == "--uninstall" ]]; then
    if [[ -f "$settings" ]]; then
        backup_settings
        tmp="$(mktemp)"
        jq --arg own "$own_hooks" '
          def strip($event):
            .hooks[$event] = ((.hooks[$event] // [])
              | map(select(((.hooks // []) | map(.command // "") | join(" ")
                            | test($own)) | not)));
          reduce (.hooks // {} | keys[]) as $e (.; strip($e))
        ' "$settings" >"$tmp"
        jq -e . "$tmp" >/dev/null
        mv "$tmp" "$settings"
        echo "removed hooks from $settings"
    fi
    "$src/login-item.sh" uninstall >/dev/null 2>&1 || true
    rm -f "$hook" "$binary"
    echo "removed $hook and $binary"
    echo "state left in ~/.claude/planter — delete that directory if you want it gone"
    exit 0
fi

# --- prerequisites -----------------------------------------------------------

fail() {
    echo "error: $1" >&2
    exit 1
}

[[ "$(uname -s)" == "Darwin" ]] || fail "planter is macOS only: it draws with AppKit"
command -v swiftc >/dev/null ||
    fail "swiftc not found — install the Xcode command line tools: xcode-select --install"
command -v jq >/dev/null || fail "jq not found — the hook needs it: brew install jq"

# --- build and install -------------------------------------------------------

echo "building planter"
mkdir -p "$bin_dir"
swiftc -O -o "$binary" "$src/main.swift" "$src/Planter.swift" "$src/Sprites.swift"

echo "installing hook to $hook"
mkdir -p "$(dirname "$hook")"
install -m 755 "$src/planter-state" "$hook"

[[ -f "$settings" ]] || echo '{}' >"$settings"
backup_settings

tmp="$(mktemp)"
jq --arg hook "$hook" --arg own "$own_hooks" '
  def strip($event):
    .hooks[$event] = ((.hooks[$event] // [])
      | map(select(((.hooks // []) | map(.command // "") | join(" ")
                    | test($own)) | not)));

  def wire($event; $arg):
    strip($event)
    | .hooks[$event] += [{hooks: [{type: "command", command: ($hook + " " + $arg)}]}];

  .hooks //= {}
  | wire("SessionStart";    "start")
  | wire("CwdChanged";      "cwd")
  | wire("UserPromptSubmit";"prompt")
  | wire("PostToolUse";     "tool")
  | wire("PostToolUseFailure";"tool")
  | wire("Stop";            "stop")
  | wire("PermissionRequest";"permission")
  | wire("Notification";    "notify")
  | wire("SubagentStart";   "agent-start")
  | wire("SubagentStop";    "agent-stop")
  | wire("SessionEnd";      "end")
' "$settings" >"$tmp"

# Never leave a half-written settings file behind.
jq -e . "$tmp" >/dev/null
mv "$tmp" "$settings"

# Report what is actually in the file, not what this script meant to put there.
# A hardcoded count once claimed nine hooks while writing eight.
wired="$(jq -r --arg own "$own_hooks" '
  [.hooks // {} | to_entries[]
   | .key as $event
   | .value[] | (.hooks // [])[] | select((.command // "") | test($own)) | $event]
  | sort | join(" ")' "$settings")"
echo "wired into $settings:"
echo "  ${wired:-nothing — something went wrong}"

# Leftovers from when this was called claude-pets.
if [[ -e "$HOME/.local/bin/pets" || -e "$HOME/.claude/hooks/pet-state" ]]; then
    rm -f "$HOME/.local/bin/pets" "$HOME/.claude/hooks/pet-state"
    echo "removed the older claude-pets install"
fi

case ":$PATH:" in
*":$bin_dir:"*) ;;
*) echo "note: $bin_dir is not on your PATH — run it as $binary" ;;
esac

echo
echo "done. start the overlay with:  planter"
echo "to start it at every login:    ./login-item.sh install"
echo "already-running sessions pick this up on their next turn; restart any"
echo "session whose plant never appears."
