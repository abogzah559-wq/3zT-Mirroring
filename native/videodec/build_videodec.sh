#!/usr/bin/env bash
# Links videodec.dll against the static FFmpeg from native/build_ffmpeg.sh.
#   PATH=/ucrt64/bin:/usr/bin bash native/videodec/build_videodec.sh <ffmpeg-prefix> <out-dir> <version>
# The DLL only imports Windows system DLLs.
set -euo pipefail
FF=$1
OUT=$2
VER=$3
HERE=$(cd "$(dirname "$0")" && pwd)
NUM=$(echo "$VER" | sed 's/-.*//' | awk -F. '{printf "%d,%d,%d,0", $1, $2, $3}')
mkdir -p "$OUT"
printf '#define VIDEODEC_VER_NUM %s\n#define VIDEODEC_VER_STR "%s"\n' "$NUM" "$VER" > "$OUT/videodec_ver.h"
windres --use-temp-file -I"$OUT" -i "$HERE/videodec.rc" -o "$OUT/videodec_rc.o"
SYSLIBS=$(PKG_CONFIG_PATH="$FF/lib/pkgconfig" pkg-config --static --libs-only-l libavcodec | sed 's/-lavcodec//; s/-lavutil//; s/-lswresample//')
gcc -O2 -shared -s -o "$OUT/videodec.dll" "$HERE/videodec.c" "$OUT/videodec_rc.o" \
  -I"$FF/include" -L"$FF/lib" -lavcodec -lavutil $SYSLIBS -ld3d11 -ldxgi -lole32 -luuid \
  -static -static-libgcc -Wl,--gc-sections -Wl,--no-insert-timestamp
rm -f "$OUT/videodec_rc.o" "$OUT/videodec_ver.h"
ls -l "$OUT/videodec.dll"
