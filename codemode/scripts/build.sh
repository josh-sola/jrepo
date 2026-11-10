#!/bin/sh
# Builds cm executables with Bun: scripts/build.sh <version> [target...]
# A target is darwin-arm64, darwin-x64, linux-arm64 or linux-x64, and the
# default is this machine. Each one lands in dist/cm-<os>-<arch>.
set -eu

if [ $# -lt 1 ]; then
  echo "usage: scripts/build.sh <version> [target...]" >&2
  exit 2
fi
version=$1
shift

cd "$(dirname "$0")/.."

if [ $# -eq 0 ]; then
  case $(uname -s) in
    Darwin) os=darwin ;;
    Linux) os=linux ;;
    *) echo "build.sh: unsupported system $(uname -s)" >&2 && exit 1 ;;
  esac
  case $(uname -m) in
    arm64 | aarch64) arch=arm64 ;;
    *) arch=x64 ;;
  esac
  set -- "$os-$arch"
fi

mkdir -p dist
for target in "$@"; do
  case $target in
    darwin-arm64 | linux-arm64) bun_target=bun-$target ;;
    # The standard x64 build crashes on CPUs without AVX, Rosetta included.
    darwin-x64 | linux-x64) bun_target=bun-$target-baseline ;;
    *) echo "build.sh: unknown target $target" >&2 && exit 2 ;;
  esac
  out=dist/cm-$target
  bun build src/cli.ts --compile --target="$bun_target" \
    --define CM_VERSION="\"$version\"" --outfile "$out"
  # Bun appends the program after the linker signs the binary, which leaves
  # the signature invalid until it is signed again.
  case $target in
    darwin-*)
      if command -v codesign >/dev/null 2>&1; then
        codesign --force --sign - "$out"
      else
        echo "build.sh: codesign not found; $out keeps an invalid signature." >&2
      fi
      ;;
    *) ;;
  esac
done
