#!/usr/bin/env bash

WT_SHELL_MARKER_BEGIN="# >>> wt-cli shell integration >>>"
WT_SHELL_MARKER_END="# <<< wt-cli shell integration <<<"

wt_shell_integration_block() {
  cat <<'EOF'
# >>> wt-cli shell integration >>>
wt() {
  if [[ "${1:-}" == "cd" ]]; then
    local arg
    for arg in "$@"; do
      case "$arg" in
        -h|--help)
          command wt "$@"
          return $?
          ;;
      esac
    done
    shift
    local target
    target="$(command wt tree path "$@")" || return $?
    builtin cd -- "$target"
  else
    command wt "$@"
  fi
}
# <<< wt-cli shell integration <<<
EOF
}

wt_replace_shell_integration() {
  local zshrc="$1"
  local block tmp marker_count
  mkdir -p "$(dirname "$zshrc")"
  touch "$zshrc"
  block="$(mktemp)"
  tmp="$(mktemp "${zshrc}.tmp.XXXXXX")"
  cp -p "$zshrc" "$tmp"
  wt_shell_integration_block > "$block"

  marker_count="$(awk -v begin="$WT_SHELL_MARKER_BEGIN" '$0 == begin { count++ } END { print count + 0 }' "$zshrc")"
  if ! awk -v begin="$WT_SHELL_MARKER_BEGIN" -v end="$WT_SHELL_MARKER_END" -v block="$block" '
    $0 == begin {
      if (inside) {
        print "nested wt shell integration marker" > "/dev/stderr"
        exit 1
      }
      inside = 1
      count++
      while ((getline line < block) > 0) print line
      close(block)
      next
    }
    $0 == end {
      if (!inside) {
        print "unmatched wt shell integration marker" > "/dev/stderr"
        exit 1
      }
      inside = 0
      next
    }
    !inside { print }
    END {
      if (inside) {
        print "unterminated wt shell integration marker" > "/dev/stderr"
        exit 1
      }
    }
  ' "$zshrc" > "$tmp"; then
    rm -f "$block" "$tmp"
    return 1
  fi

  if [[ "$marker_count" == "0" ]]; then
    printf '\n' >> "$tmp"
    cat "$block" >> "$tmp"
    WT_SHELL_INTEGRATION_ACTION="installed"
  else
    WT_SHELL_INTEGRATION_ACTION="updated"
  fi
  rm -f "$block"
  mv "$tmp" "$zshrc"
}
