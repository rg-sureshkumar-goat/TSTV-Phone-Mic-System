#!/bin/sh
# Re-vendors libopus into third_party/opus/libopus.
# Usage: ./update.sh /path/to/extracted/opus-X.Y.Z
# Copies only the portable floating-point sources (no x86/ARM/MIPS
# optimisations, no DNN features), so every platform builds the same files.
set -eu
SRC="$1"
DEST="$(cd "$(dirname "$0")" && pwd)/libopus"
rm -rf "$DEST"
mkdir -p "$DEST"

list() { # print the files of one Makefile variable from a *_sources.mk file
  awk -v var="$2" '
    $1 == var && $2 == "=" { on = 1; next }
    on { gsub(/\\/, ""); if ($0 ~ /^[ \t]*$/) exit; for (i = 1; i <= NF; i++) print $i }
  ' "$SRC/$1"
}

{
  list celt_sources.mk CELT_SOURCES
  list silk_sources.mk SILK_SOURCES
  list silk_sources.mk SILK_SOURCES_FLOAT
  list opus_sources.mk OPUS_SOURCES
  list opus_sources.mk OPUS_SOURCES_FLOAT
} | while read -r f; do
  mkdir -p "$DEST/$(dirname "$f")"
  cp "$SRC/$f" "$DEST/$f"
done

for d in include celt silk silk/float src; do
  mkdir -p "$DEST/$d"
  cp "$SRC/$d"/*.h "$DEST/$d/"
done
cp "$SRC/COPYING" "$SRC/AUTHORS" "$DEST/"
cat "$SRC/package_version" > "$DEST/VERSION"
echo "Vendored $(cat "$DEST/VERSION") into $DEST"
