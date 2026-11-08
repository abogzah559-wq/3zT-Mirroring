"""AAC / AAC-ELD decoding through aacdec.dll (native/aacdec).

aacdec.dll is a minimal static FFmpeg (only the AAC decoder and swresample,
LGPL-2.1+) behind four C functions. It replaces PyAV, whose wheel ships the
whole FFmpeg (~65 MB) for this one decoder. Output: interleaved s16 stereo.
"""

from __future__ import annotations

import ctypes
import sys
from pathlib import Path

_DLL_NAME = "aacdec.dll"
# Largest output of one access unit: 2048 samples (HE-AAC) resampled up to
# 48 kHz from 22.05 kHz is ~4460 frames; 8192 leaves room.
_CAP = 8192
_lib = None


class AacError(RuntimeError):
    pass


def _load():
    global _lib
    if _lib is not None:
        return _lib
    if sys.platform != "win32":
        raise AacError("aacdec is only built for Windows")
    path = Path(__file__).resolve().parent / _DLL_NAME
    lib = ctypes.CDLL(str(path))
    lib.zl_aac_open.restype = ctypes.c_void_p
    lib.zl_aac_open.argtypes = [ctypes.c_char_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int]
    lib.zl_aac_decode.restype = ctypes.c_int
    lib.zl_aac_decode.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
    lib.zl_aac_close.restype = None
    lib.zl_aac_close.argtypes = [ctypes.c_void_p]
    lib.zl_aac_avcodec_version.restype = ctypes.c_uint
    lib.zl_aac_avcodec_version.argtypes = []
    _lib = lib
    return lib


def avcodec_version() -> tuple[int, int, int]:
    v = _load().zl_aac_avcodec_version()
    return v >> 16, (v >> 8) & 0xFF, v & 0xFF


class AacDecoder:
    """One AAC stream -> s16le stereo at out_rate (bytes per call)."""

    def __init__(self, asc: bytes, sample_rate: int, channels: int = 2, out_rate: int = 48000) -> None:
        self._lib = _load()
        self._h = self._lib.zl_aac_open(bytes(asc), len(asc), sample_rate, channels, out_rate)
        if not self._h:
            raise AacError("AAC decoder did not open (bad codec config)")
        self._buf = (ctypes.c_int16 * (_CAP * 2))()

    def decode(self, au: bytes) -> bytes:
        if not au:
            return b""
        n = self._lib.zl_aac_decode(self._h, bytes(au), len(au), self._buf, _CAP)
        if n < 0:
            raise AacError(f"AAC decode failed ({n})")
        return ctypes.string_at(self._buf, n * 4)

    def close(self) -> None:
        if self._h:
            self._lib.zl_aac_close(self._h)
            self._h = None

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:  # noqa: BLE001 - interpreter shutdown
            pass
