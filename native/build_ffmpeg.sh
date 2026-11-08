#!/usr/bin/env bash
# Builds the minimal FFmpeg that 3zT Mirroring needs, as static libraries,
# with MSYS2 UCRT64 gcc and nasm:
#   - AAC / AAC-ELD decoder + swresample (aacdec.dll, device sound)
#   - HEVC decoder with Direct3D 11 hardware decoding (videodec.dll, the
#     iPhone/iPad picture on PCs without Microsoft's HEVC extension)
# Run from an MSYS2 bash:
#   PATH=/ucrt64/bin:/usr/bin bash native/build_ffmpeg.sh <ffmpeg-src> <build-dir> <prefix>
# LGPL-2.1+ (no --enable-gpl); see THIRD_PARTY_NOTICES.md.
set -euo pipefail
SRC=$1
BUILD=$2
PREFIX=$3
mkdir -p "$BUILD"
cd "$BUILD"
"$SRC/configure" \
  --prefix="$PREFIX" \
  --target-os=mingw32 --arch=x86_64 --cc=gcc \
  --enable-static --disable-shared \
  --disable-everything \
  --disable-programs --disable-doc --disable-network --disable-autodetect \
  --disable-avdevice --disable-avformat --disable-avfilter --disable-swscale \
  --disable-pthreads --disable-os2threads --enable-w32threads \
  --disable-debug \
  --enable-avcodec --enable-avutil --enable-swresample \
  --enable-decoder=aac \
  --enable-decoder=hevc --enable-parser=hevc \
  --enable-d3d11va --enable-hwaccel=hevc_d3d11va --enable-hwaccel=hevc_d3d11va2
make -j"$(nproc)"
make install
