#!/usr/bin/env bash
# Links aacdec.dll against the static minimal FFmpeg from build_ffmpeg.sh.
#   PATH=/ucrt64/bin:/usr/bin bash native/aacdec/build_aacdec.sh <ffmpeg-prefix> <out-dir> <version>
# The DLL only imports Windows system DLLs (no libgcc / winpthread DLLs).
set -euo pipefail
FF=$1
OUT=$2
VER=$3
HERE=$(cd "$(dirname "$0")" && pwd)
NUM=$(echo "$VER" | sed 's/-.*//' | awk -F. '{printf "%d,%d,%d,0", $1, $2, $3}')
mkdir -p "$OUT"
printf '#define AACDEC_VER_NUM %s\n#define AACDEC_VER_STR "%s"\n' "$NUM" "$VER" > "$OUT/aacdec_ver.h"
windres --use-temp-file -I"$OUT" -i "$HERE/aacdec.rc" -o "$OUT/aacdec_rc.o"
gcc -O2 -shared -s -o "$OUT/aacdec.dll" "$HERE/aacdec.c" "$OUT/aacdec_rc.o" \
  -I"$FF/include" -L"$FF/lib" -lswresample -lavcodec -lavutil -lbcrypt \
  -static -static-libgcc -Wl,--gc-sections -Wl,--no-insert-timestamp
rm -f "$OUT/aacdec_rc.o" "$OUT/aacdec_ver.h"
ls -l "$OUT/aacdec.dll"
