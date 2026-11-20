#!/bin/sh
# Installs or updates viz from its GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/josh-sola/jrepo/main/viz/install.sh | sh
#
# Environment:
#   VIZ_VERSION        version to install, such as 0.1.0 (default: newest release)
#   VIZ_BIN_DIR        where the viz link goes (default: ~/.local/bin)
#   XDG_DATA_HOME      versions go in $XDG_DATA_HOME/viz/versions (default: ~/.local/share)
#   VIZ_DOWNLOAD_BASE  release download URL (default: GitHub)
#   VIZ_GITHUB_API     GitHub API URL (default: https://api.github.com)
#   GITHUB_TOKEN       sent to the GitHub API when set
set -eu

# Everything runs from main, so a download cut off midway runs nothing.
main() {
  repo=josh-sola/jrepo
  api=${VIZ_GITHUB_API:-https://api.github.com}
  download_base=${VIZ_DOWNLOAD_BASE:-https://github.com/$repo/releases/download}
  bin_dir=${VIZ_BIN_DIR:-$HOME/.local/bin}
  versions_dir=${XDG_DATA_HOME:-$HOME/.local/share}/viz/versions

  fail() {
    echo "viz install: $*" >&2
    exit 1
  }

  case $(uname -s) in
    Darwin) os=darwin ;;
    Linux) os=linux ;;
    *) fail "unsupported system $(uname -s). viz runs on macOS and Linux." ;;
  esac
  case $(uname -m) in
    arm64 | aarch64) arch=arm64 ;;
    x86_64 | amd64) arch=x64 ;;
    *) fail "unsupported CPU $(uname -m). viz runs on arm64 and x64." ;;
  esac

  api_get() {
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      curl -fsSL -H "Accept: application/vnd.github+json" \
        -H "Authorization: Bearer $GITHUB_TOKEN" "$1"
    else
      curl -fsSL -H "Accept: application/vnd.github+json" "$1"
    fi
  }

  # Prints X.Y.Z for each published viz-vX.Y.Z release in the API's JSON. Each
  # release lists tag_name before draft and prerelease, and only the first of
  # each after a tag counts, so release notes cannot change the answer.
  published_versions() {
    grep -oE '"tag_name": *"[^"]*"|"draft": *[a-z]+|"prerelease": *[a-z]+' |
      awk -F'"' '
        function flush() {
          if (tag ~ /^viz-v[0-9]+\.[0-9]+\.[0-9]+$/ && draft == "false" && pre == "false")
            print substr(tag, 6)
        }
        $2 == "tag_name" { flush(); tag = $4; draft = ""; pre = ""; next }
        $2 == "draft" && draft == "" { sub(/^: */, "", $3); draft = $3; next }
        $2 == "prerelease" && pre == "" { sub(/^: */, "", $3); pre = $3; next }
        END { flush() }
      '
  }

  if [ -n "${VIZ_VERSION:-}" ]; then
    version=${VIZ_VERSION#viz-v}
    version=${version#v}
  else
    releases=$(api_get "$api/repos/$repo/releases?per_page=100") ||
      fail "cannot list releases from $api."
    version=$(printf '%s\n' "$releases" | published_versions |
      sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
    [ -n "$version" ] || fail "no viz release is published yet."
  fi

  asset=viz-$os-$arch.gz
  url=$download_base/viz-v$version
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT

  echo "Downloading viz $version for $os-$arch."
  curl -fsSL -o "$tmp/$asset" "$url/$asset" || fail "cannot download $url/$asset."
  curl -fsSL -o "$tmp/SHA256SUMS" "$url/SHA256SUMS" ||
    fail "cannot download $url/SHA256SUMS."

  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | awk '{print $1}')
  else
    actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
  fi
  expected=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
  [ -n "$expected" ] || fail "SHA256SUMS has no entry for $asset."
  [ "$actual" = "$expected" ] || fail "$asset does not match its checksum."

  # The checksum can't prove where the file came from, since SHA256SUMS ships
  # in the same release. The attestation can.
  if command -v gh >/dev/null 2>&1; then
    gh attestation verify "$tmp/$asset" --repo "$repo" >/dev/null ||
      fail "the build attestation for $asset did not verify."
  else
    echo "gh not found; skipped the build attestation check for $asset." >&2
  fi

  target_dir=$versions_dir/$version
  mkdir -p "$target_dir"
  gunzip -c "$tmp/$asset" >"$target_dir/viz.new"
  chmod +x "$target_dir/viz.new"
  mv -f "$target_dir/viz.new" "$target_dir/viz"
  target=$target_dir/viz

  link=$bin_dir/viz
  previous=
  first_install=1
  if [ -L "$link" ] || [ -e "$link" ]; then
    first_install=0
  fi
  if [ -L "$link" ]; then
    previous=$(readlink "$link")
  fi
  mkdir -p "$bin_dir"
  rm -f "$link.new"
  ln -s "$target" "$link.new"
  mv -f "$link.new" "$link"

  # Keep the new version and the one it replaced, for rollback.
  case $previous in
    "$versions_dir"/*/viz) kept=$(basename "$(dirname "$previous")") ;;
    *) kept= ;;
  esac
  for dir in "$versions_dir"/*; do
    [ -d "$dir" ] || continue
    name=$(basename "$dir")
    if [ "$name" != "$version" ] && [ "$name" != "$kept" ]; then
      rm -rf "$dir"
    fi
  done

  echo "Installed $("$link" --version) at $link."
  case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to PATH to run viz." >&2 ;;
  esac

  if [ "$first_install" = 1 ]; then
    cat <<'EOF'
Next:
  viz guide                           read the KDL v2 reference and CLI commands
  viz open <file.viz.kdl>             open a visualization in a Rex floating window
EOF
  fi
}

main "$@"
