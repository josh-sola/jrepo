#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/../shell-integration.sh"

grep -qF 'grep -qF "${WT_SHELL_MARKER_BEGIN}"' "${SCRIPT_DIR}/../install.sh"
! grep -qF '${MARKER_BEGIN}' "${SCRIPT_DIR}/../install.sh"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
zshrc="${tmp_dir}/.zshrc"

cat > "$zshrc" <<'EOF'
before
# >>> wt-cli shell integration >>>
wt() { command wt path "$@"; }
# <<< wt-cli shell integration <<<
after
EOF
chmod 0644 "$zshrc"

wt_replace_shell_integration "$zshrc"
[[ "$WT_SHELL_INTEGRATION_ACTION" == "updated" ]]
[[ "$(stat -f '%Lp' "$zshrc")" == "644" ]]
grep -qx 'before' "$zshrc"
grep -qx 'after' "$zshrc"
grep -qx '    target="$(command wt tree path "$@")" || return $?' "$zshrc"
! grep -q '"\$1" == "go"' "$zshrc"
grep -qx '        -h|--help)' "$zshrc"

first="$(cksum "$zshrc")"
wt_replace_shell_integration "$zshrc"
[[ "$WT_SHELL_INTEGRATION_ACTION" == "updated" ]]
[[ "$first" == "$(cksum "$zshrc")" ]]

fresh="${tmp_dir}/fresh.zshrc"
printf 'export WT_TEST_KEEP=1\n' > "$fresh"
wt_replace_shell_integration "$fresh"
[[ "$WT_SHELL_INTEGRATION_ACTION" == "installed" ]]
grep -qx 'export WT_TEST_KEEP=1' "$fresh"
grep -qx '# >>> wt-cli shell integration >>>' "$fresh"
zsh -n "$fresh"

bin_dir="${tmp_dir}/bin"
mkdir -p "$bin_dir" "${tmp_dir}/tree"
cat > "${bin_dir}/wt" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" == "cd" ]]; then
  printf 'binary:%s\n' "$*"
elif [[ "$1" == "tree" && "$2" == "path" ]]; then
  printf '%s\n' "$WT_TEST_TREE"
else
  printf 'binary:%s\n' "$*"
fi
EOF
chmod +x "${bin_dir}/wt"

help_output="$(PATH="${bin_dir}:${PATH}" zsh -c 'source "$1"; wt cd --help' zsh "$fresh")"
[[ "$help_output" == "binary:cd --help" ]] || exit 1
tree_output="$(PATH="${bin_dir}:${PATH}" WT_TEST_TREE="${tmp_dir}/tree" zsh -c 'source "$1"; wt cd example; pwd' zsh "$fresh")"
[[ "$tree_output" == "${tmp_dir}/tree" ]] || exit 1
go_output="$(PATH="${bin_dir}:${PATH}" zsh -c 'source "$1"; wt go example' zsh "$fresh")"
[[ "$go_output" == "binary:go example" ]] || exit 1
empty_output="$(PATH="${bin_dir}:${PATH}" zsh -c 'source "$1"; wt' zsh "$fresh")"
[[ "$empty_output" == "binary:" ]] || exit 1

echo "shell integration tests passed"
