"""3zT Mirroring backend entry point.

The window (3zTMirroring.exe) starts this service with ``--headless`` and
``--parent-pid``; it serves the device protocols on 127.0.0.1 and exits with
the window.
"""

from __future__ import annotations

import ctypes
import logging
import os
import sys
import threading

from mirroring.castmanager import CastManager
from mirroring.paths import APP_ROOT, LOG_DIR
from mirroring.server import HOST, PORT, serve_forever


def _wait_for_parent() -> None:
    """Headless backend: run until the process given by --parent-pid exits
    (the native 3zT Mirroring app), or forever when there is none."""
    pid = None
    if "--parent-pid" in sys.argv:
        try:
            pid = int(sys.argv[sys.argv.index("--parent-pid") + 1])
        except (IndexError, ValueError):
            pid = None
    if not pid:
        threading.Event().wait()
        return
    synchronize, infinite = 0x00100000, 0xFFFFFFFF
    handle = ctypes.windll.kernel32.OpenProcess(synchronize, False, pid)
    if not handle:
        return
    ctypes.windll.kernel32.WaitForSingleObject(handle, infinite)
    ctypes.windll.kernel32.CloseHandle(handle)

def _already_running() -> bool:
    """A second launch brings the running 3zT Mirroring to the front and exits,
    instead of waiting invisibly for a port the first one holds."""
    import json
    import urllib.request

    try:
        with urllib.request.urlopen(f"http://{HOST}:{PORT}/api/state", timeout=1.5) as r:
            json.loads(r.read())
    except Exception:  # noqa: BLE001 - nothing (or not 3zT Mirroring) is listening
        return False
    try:
        req = urllib.request.Request(f"http://{HOST}:{PORT}/api/app/show", data=b"{}", method="POST",
                                     headers={"Content-Type": "application/json"})
        urllib.request.urlopen(req, timeout=3).read()
    except Exception:  # noqa: BLE001
        pass
    logging.getLogger(__name__).info("3zT Mirroring is already running: brought it to the front")
    return True


def _open_rust_window() -> None:
    """MirroringService.exe was started on its own: start 3zTMirroring.exe instead."""
    import subprocess

    exe = APP_ROOT / "3zTMirroring.exe"
    if exe.is_file():
        subprocess.Popen([str(exe)], close_fds=True)  # noqa: S603
        return
    try:
        ctypes.windll.user32.MessageBoxW(None, "Open 3zTMirroring.exe to start 3zT Mirroring.", "3zT Mirroring", 0x40)
    except Exception:  # noqa: BLE001
        pass


def _version() -> str:
    from mirroring import __version__

    return __version__


def main() -> None:
    from logging.handlers import RotatingFileHandler

    # logs/mirroring-service.log, 2 MB per file, the last two kept (.1, .2).
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
        handlers=[RotatingFileHandler(LOG_DIR / "mirroring-service.log", maxBytes=2 << 20, backupCount=2,
                                      encoding="utf-8")],
        force=True,  # a library import may have configured logging already (empty log otherwise)
    )
    logging.getLogger("mirroring").info("3zT Mirroring backend %s started (pid %d, Python %s, %s)", _version(), os.getpid(),
                                      sys.version.split()[0], sys.platform)
    import faulthandler

    try:  # a native crash (audio driver, decoder) leaves a trace instead of nothing
        faulthandler.enable(open(LOG_DIR / "crash.log", "a", encoding="utf-8"))  # noqa: SIM115
    except OSError:
        pass
    from mirroring import restart

    restart.wait_for_port(HOST, PORT)
    if _already_running():
        return
    manager = CastManager()
    ready = threading.Event()
    threading.Thread(target=serve_forever, args=(manager, ready), name="server", daemon=True).start()
    if not ready.wait(10):
        msg = (f"3zT Mirroring could not start: port {PORT} is already in use.\n\n"
               "3zT Mirroring may already be running (check the system tray), or another program uses the port. "
               "Close it and open 3zT Mirroring again.")
        try:
            import ctypes

            ctypes.windll.user32.MessageBoxW(None, msg, "3zT Mirroring", 0x10)
        except Exception:  # noqa: BLE001
            pass
        sys.exit(msg)

    if "--headless" in sys.argv:
        if sys.stdout:
            print(f"3zT Mirroring service running at http://{HOST}:{PORT}/", flush=True)
        _wait_for_parent()
        manager.stop()
        return

    # Started on its own: open the window instead, which manages this service.
    manager.stop()
    _open_rust_window()


if __name__ == "__main__":
    main()
