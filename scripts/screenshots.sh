#!/usr/bin/env bash
# Regenerate every screenshot the documentation shows.
#
#   scripts/screenshots.sh            # re-render, then copy into docs/
#   scripts/screenshots.sh --check    # fail if any picture is out of date
#
# Rendering is headless through lavapipe: no GPU and no display required.
set -euo pipefail

cd "$(dirname "$0")/.."

ICD=${VK_ICD_FILENAMES:-/usr/share/vulkan/icd.d/lvp_icd.json}
if [[ ! -f $ICD ]]; then
    echo "no software Vulkan driver at $ICD" >&2
    echo "install mesa's lavapipe, or set VK_ICD_FILENAMES to one" >&2
    exit 1
fi
export VK_ICD_FILENAMES=$ICD

SRC=crates/archet-ui/tests/snapshots
DEST=docs/screenshots

if [[ ${1:-} == --check ]]; then
    cargo test -p archet-ui snapshot -- --ignored
    missing=0
    while read -r name; do
        [[ -z $name || $name == \#* ]] && continue
        if [[ ! -f $SRC/$name.png ]]; then
            echo "no test renders $name" >&2
            missing=1
        elif [[ ! -f $DEST/$name.png ]] || ! cmp -s "$SRC/$name.png" "$DEST/$name.png"; then
            echo "out of date: $DEST/$name.png" >&2
            missing=1
        fi
    done < docs/screenshots.list
    [[ $missing -eq 0 ]] || {
        echo "run scripts/screenshots.sh to bring the documentation in step" >&2
        exit 1
    }
    echo "every screenshot is current"
    exit 0
fi

echo "rendering..."
UPDATE_SNAPSHOTS=1 cargo test -p archet-ui snapshot -- --ignored

mkdir -p "$DEST"
while read -r name; do
    [[ -z $name || $name == \#* ]] && continue
    if [[ -f $SRC/$name.png ]]; then
        cp "$SRC/$name.png" "$DEST/$name.png"
        echo "  $DEST/$name.png"
    else
        echo "  MISSING $SRC/$name.png -- no test renders it" >&2
    fi
done < docs/screenshots.list

echo "done. docs/screenshots.list names what is copied."
