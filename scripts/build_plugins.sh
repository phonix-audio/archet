#!/usr/bin/env bash
#
# Build the VST3/CLAP bundle.
#
#   scripts/build_plugins.sh              # this platform
#   scripts/build_plugins.sh --install    # and copy it where a host scans
#   scripts/build_plugins.sh --windows    # cross-compile to Windows (MSVC)
#
# Layout, per the VST3 spec:
#   Archet.vst3/Contents/x86_64-linux/Archet.so    (Linux)
#   Archet.vst3/Contents/x86_64-win/Archet.vst3    (Windows)
#
# A CLAP is the same shared object under a `.clap` name.
set -euo pipefail
cd "$(dirname "$0")/.."

# Not necessarily ./target: a shared [build] target-dir in a parent
# .cargo/config.toml moves it. Ask cargo where it actually is.
TD="$(cargo metadata --no-deps --format-version 1 |
      python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

WINDOWS=0
INSTALL=0
for arg in "$@"; do
    case "$arg" in
        --windows) WINDOWS=1 ;;
        --install) INSTALL=1 ;;
        *) echo "unknown option: $arg" >&2; exit 1 ;;
    esac
done

if [[ $WINDOWS -eq 1 ]]; then
    TARGET="x86_64-pc-windows-msvc"
    PLATFORM="windows"
    LIB_EXT="dll"
    ARCH_DIR="x86_64-win"
    VST3_EXT="vst3"
    command -v cargo-xwin >/dev/null || { echo "needs cargo-xwin" >&2; exit 1; }
    # The distro ships clang-cl unversioned only inside /usr/lib/llvm-*/bin.
    for d in $(ls -d /usr/lib/llvm-*/bin 2>/dev/null | sort -V -r); do
        [[ -x "$d/clang-cl" ]] && { export PATH="$d:$PATH"; break; }
    done
    rustup target add "$TARGET" >/dev/null 2>&1 || true
    cargo xwin build --release --target "$TARGET" -p archet-plugin
    ARTIFACTS="$TD/$TARGET/release"
else
    PLATFORM="$(uname -s | tr '[:upper:]' '[:lower:]')"
    LIB_EXT="so"
    ARCH_DIR="$(uname -m)-linux"
    VST3_EXT="so"
    cargo build --release -p archet-plugin
    ARTIFACTS="$TD/release"
fi

NAME=$(python3 - <<'PY'
import re
print(re.search(r'^name\s*=\s*"([^"]+)"', open('bundler.toml').read(), re.M).group(1))
PY
)

SRC="$ARTIFACTS/archet_plugin.$LIB_EXT"
[[ -f "$SRC" ]] || SRC="$ARTIFACTS/libarchet_plugin.$LIB_EXT"
[[ -f "$SRC" ]] || { echo "no artifact at $ARTIFACTS" >&2; exit 1; }

OUT="$TD/bundled/$PLATFORM"
DEST="$OUT/${NAME}.vst3/Contents/${ARCH_DIR}"
mkdir -p "$DEST"
cp "$SRC" "$DEST/${NAME}.${VST3_EXT}"
cp "$SRC" "$OUT/${NAME}.clap"

echo "bundled:"
echo "  $DEST/${NAME}.${VST3_EXT}"
echo "  $OUT/${NAME}.clap"

# A host finds a plugin by scanning; installing is putting the bundle where it
# already looks. The VST3 spec names ~/.vst3 on Linux and
# ~/Library/Audio/Plug-Ins/VST3 on macOS; CLAP names the same directories under
# .clap and CLAP. VST3_PATH and CLAP_PATH override both.
if [[ $INSTALL -eq 1 ]]; then
    if [[ $WINDOWS -eq 1 ]]; then
        echo "--install has nowhere to put a Windows bundle on this machine" >&2
        exit 1
    fi
    if [[ $PLATFORM == darwin ]]; then
        VST3_HOME="${VST3_PATH:-$HOME/Library/Audio/Plug-Ins/VST3}"
        CLAP_HOME="${CLAP_PATH:-$HOME/Library/Audio/Plug-Ins/CLAP}"
    else
        VST3_HOME="${VST3_PATH:-$HOME/.vst3}"
        CLAP_HOME="${CLAP_PATH:-$HOME/.clap}"
    fi
    mkdir -p "$VST3_HOME" "$CLAP_HOME"
    # Replaced whole: a bundle left over from an older build can carry an
    # architecture directory this one no longer writes.
    rm -rf "${VST3_HOME:?}/${NAME}.vst3"
    cp -r "$OUT/${NAME}.vst3" "$VST3_HOME/"
    cp "$OUT/${NAME}.clap" "$CLAP_HOME/${NAME}.clap"
    echo "installed:"
    echo "  $VST3_HOME/${NAME}.vst3"
    echo "  $CLAP_HOME/${NAME}.clap"
fi
