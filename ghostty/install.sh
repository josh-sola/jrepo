#!/bin/sh

set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
target_dir="$HOME/Library/Application Support/com.mitchellh.ghostty/shaders"

refuse() {
    printf '%s\n' 'Refusing: merge, sync main, and rerun from /Users/joshbassin/repos/jrepo.' >&2
    exit 1
}

git_dir=$(git -C "$repo_dir" rev-parse --absolute-git-dir 2>/dev/null) || refuse
common_dir=$(git -C "$repo_dir" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || refuse
branch=$(git -C "$repo_dir" branch --show-current 2>/dev/null) || refuse

[ "$git_dir" = "$common_dir" ] && [ "$branch" = main ] || refuse
git -C "$repo_dir" diff --quiet -- ghostty/shaders || refuse
git -C "$repo_dir" diff --cached --quiet -- ghostty/shaders || refuse
[ -z "$(git -C "$repo_dir" ls-files --others --exclude-standard -- ghostty/shaders)" ] || refuse

mkdir -p "$target_dir"

status=0

for source in "$script_dir"/shaders/*.glsl; do
    [ -f "$source" ] || continue

    target="$target_dir/$(basename -- "$source")"
    if [ -L "$target" ]; then
        rm "$target"
    elif [ -e "$target" ]; then
        if [ -f "$target" ] && cmp -s "$source" "$target"; then
            rm "$target"
        else
            printf 'Refusing to replace unexpected target: %s\n' "$target" >&2
            status=1
            continue
        fi
    fi

    ln -s "$source" "$target"
done

exit "$status"
