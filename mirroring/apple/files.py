"""Files between the PC and an iPhone/iPad, both ways.

iOS has no shared Download folder that a PC can write to. What Apple's own
Finder/iTunes "File Sharing" uses is house_arrest: an app that declares
``UIFileSharingEnabled`` exposes its ``Documents`` folder over AFC, and the
file then shows up in that app and in the Files app under "On My iPhone".
So the user picks one of those apps (VLC, Documents, Pages …) and the file
is streamed into its Documents folder.

From the device: the camera roll (DCIM, over AFC) and the Documents folder
of apps with File Sharing. iOS gives a PC no other access: no general file
system, and photos sent from the PC cannot go into the Photos app directly.

Needs Apple Mobile Device Service (usbmux) and a trusted device, i.e.
Apple Driver mode; Studio mode replaces Apple's USB driver and has no
usbmux. Nothing here changes drivers or settings on the device.
"""

from __future__ import annotations

import asyncio
import logging
import os
import re
import time
from pathlib import Path

log = logging.getLogger(__name__)

DOCUMENTS = "/Documents"
CHUNK = 1 << 20  # 1 MB per AFC write: smooth progress, bounded memory

# The running (or last) transfer, read by GET /api/files/job.
job: dict = {"state": "idle", "direction": "", "name": "", "app": "", "index": 0, "count": 0, "sent": 0, "total": 0,
            "speed": 0, "done": 0, "message": ""}


def safe_name(name: str) -> str | None:
    """A file name without folders or characters iOS/AFC would refuse."""
    clean = "".join("_" if (ord(c) < 32 or c in '"*/:<>?\\|') else c for c in name).strip().rstrip(".")
    if not clean or set(clean) <= {".", "_"}:
        return None
    if len(clean.encode()) > 200:
        stem, ext = os.path.splitext(clean)
        ext = ext if len(ext) <= 10 else ""
        while len((stem + ext).encode()) > 200:
            stem = stem[:-1]
        clean = stem + ext
    return clean


def numbered(name: str, n: int) -> str:
    stem, ext = os.path.splitext(name)
    if not stem:  # ".bashrc"
        stem, ext = name, ""
    return f"{stem} ({n}){ext}"


def sharing_apps(apps: dict) -> list[dict]:
    """Apps from installation_proxy whose Documents folder is shared."""
    out = []
    for bundle, info in apps.items():
        if not info.get("UIFileSharingEnabled"):
            continue
        name = info.get("CFBundleDisplayName") or info.get("CFBundleName") or bundle
        out.append({"bundle": bundle, "name": str(name)})
    return sorted(out, key=lambda a: a["name"].lower())


async def _lockdown(udid: str):
    from pymobiledevice3.lockdown import create_using_usbmux

    # Already trusted for Apple Driver mode: never start a new pairing here.
    return await create_using_usbmux(serial=udid, autopair=False)


async def list_apps(udid: str) -> list[dict]:
    from pymobiledevice3.services.installation_proxy import InstallationProxyService

    lockdown = await _lockdown(udid)
    try:
        proxy = InstallationProxyService(lockdown)
        try:
            apps = await proxy.get_apps(application_type="User")
        finally:
            await proxy.close()
    finally:
        await lockdown.close()
    return sharing_apps(apps)


class Cancelled(Exception):
    """The user pressed Cancel."""


_cancel = {"asked": False}


def cancel() -> bool:
    """Asks the running transfer to stop (it removes the half-copied file)."""
    if job.get("state") != "running":
        return False
    _cancel["asked"] = True
    return True


def _check_cancel() -> None:
    if _cancel["asked"]:
        raise Cancelled("cancelled")


def _begin(direction: str, count: int, app_name: str = "", dest: str = "") -> None:
    _cancel["asked"] = False
    job.clear()
    job.update(state="running", direction=direction, name="", app=app_name, dest=dest, index=0, count=count,
               sent=0, total=0, speed=0, done=0, message="", started=time.monotonic())


def _progress(n: int) -> None:
    job["sent"] += n
    elapsed = time.monotonic() - job.get("file_started", time.monotonic())
    if elapsed > 0.3:
        job["speed"] = int(job["sent"] / elapsed)


def local_files(paths: list[Path]) -> list[tuple[Path, str]]:
    """Files to send: each file, and every file inside each folder with its
    folder path relative to the dropped folder (posix, cleaned names)."""
    out: list[tuple[Path, str]] = []
    for path in paths:
        if path.is_file():
            out.append((path, ""))
        elif path.is_dir():
            root_name = safe_name(path.name) or "Folder"
            for folder, dirs, files in os.walk(path):
                dirs.sort()
                rel = Path(folder).relative_to(path)
                parts = [root_name] + [safe_name(p) or "_" for p in rel.parts]
                for f in sorted(files):
                    out.append((Path(folder) / f, "/".join(parts)))
    return out


async def _unique(exists, folder: str, name: str) -> str:
    candidate, n = name, 0
    while await exists(f"{folder}/{candidate}"):
        n += 1
        if n > 999:
            raise RuntimeError("no free file name")
        candidate = numbered(name, n)
    return candidate


async def _send_one(afc, local: Path, folder: str) -> str:
    """Streams one file into `folder`; checks its size there afterwards and
    removes a half-written copy on any failure or cancel."""
    name = safe_name(local.name)
    if name is None:
        raise ValueError("unusable file name")
    total = local.stat().st_size
    job.update(name=name, total=total, sent=0, speed=0, file_started=time.monotonic())
    remote = f"{folder}/{await _unique(afc.exists, folder, name)}"
    handle = await afc.fopen(remote, "w")
    complete = False
    try:
        with local.open("rb") as f:
            while chunk := f.read(CHUNK):
                _check_cancel()
                await afc.fwrite(handle, chunk)
                _progress(len(chunk))
        await afc.fclose(handle)
        handle = None
        size = (await afc.stat(remote)).get("st_size")
        if size != total:
            raise RuntimeError(f"size on the device {size} ≠ {total}")
        complete = True
    finally:
        if handle is not None:
            try:
                await afc.fclose(handle)
            except Exception:  # noqa: BLE001
                pass
        if not complete:
            try:
                await afc.rm_single(remote, force=True)
            except Exception:  # noqa: BLE001
                pass
    return remote.rsplit("/", 1)[1]


async def send_many(udid: str, bundle: str, app_name: str, paths: list[Path]) -> int:
    """Sends files and folders into the app's Documents folder; returns how
    many arrived complete."""
    from pymobiledevice3.services.house_arrest import HouseArrestService

    items = local_files(paths)
    _begin("send", len(items), app_name)
    lockdown = await _lockdown(udid)
    try:
        afc = await HouseArrestService.create(lockdown, bundle, documents_only=True)
        try:
            made: set[str] = set()
            for i, (local, rel) in enumerate(items):
                _check_cancel()
                job["index"] = i + 1
                folder = DOCUMENTS + (f"/{rel}" if rel else "")
                if rel and folder not in made:
                    await afc.makedirs(folder)
                    made.add(folder)
                await _send_one(afc, local, folder)
                job["done"] += 1
        finally:
            await afc.close()
    finally:
        await lockdown.close()
    return job["done"]


# ---- From the device to the PC -------------------------------------------

MEDIA_ROOT = "/DCIM"
_PHOTO = {".jpg", ".jpeg", ".heic", ".heif", ".png", ".gif", ".dng", ".webp", ".tif", ".tiff"}
_VIDEO = {".mov", ".mp4", ".m4v", ".hevc", ".avi"}


def media_kind(name: str) -> str:
    ext = os.path.splitext(name)[1].lower()
    return "photo" if ext in _PHOTO else "video" if ext in _VIDEO else "other"


def device_path_ok(path: str, root: str) -> bool:
    """A path the window may ask for: under `root`, no "..", no NUL."""
    parts = path.split("/")
    return path.startswith(root + "/") and ".." not in parts and "\x00" not in path and "" not in parts[1:]


async def _list(afc, root: str, limit: int) -> list[dict]:
    out: list[dict] = []
    async for folder, _dirs, files in afc.walk(root):
        for name in files:
            if name.startswith("."):
                continue
            path = f"{folder}/{name}"
            try:
                st = await afc.stat(path)
            except Exception:  # noqa: BLE001 - vanished meanwhile
                continue
            mtime = st.get("st_mtime")
            out.append({"path": path, "name": name, "size": int(st.get("st_size") or 0),
                        "time": int(mtime.timestamp()) if mtime else 0, "kind": media_kind(name)})
            if len(out) >= limit:
                break
    out.sort(key=lambda x: x["time"], reverse=True)
    return out


async def list_media(udid: str, limit: int = 3000) -> list[dict]:
    """Photos and videos in the camera roll folder (DCIM), newest first.
    Only what is on the device: iCloud photos not downloaded are not there."""
    from pymobiledevice3.services.afc import AfcService

    lockdown = await _lockdown(udid)
    try:
        async with AfcService(lockdown) as afc:
            if not await afc.exists(MEDIA_ROOT):
                return []
            return await _list(afc, MEDIA_ROOT, limit)
    finally:
        await lockdown.close()


async def list_app_files(udid: str, bundle: str, limit: int = 3000) -> list[dict]:
    """Files in an app's shared Documents folder (File Sharing), newest first."""
    from pymobiledevice3.services.house_arrest import HouseArrestService

    lockdown = await _lockdown(udid)
    try:
        afc = await HouseArrestService.create(lockdown, bundle, documents_only=True)
        try:
            return await _list(afc, DOCUMENTS, limit)
        finally:
            await afc.close()
    finally:
        await lockdown.close()


def unique_local(dest: Path, name: str) -> Path:
    target, n = dest / name, 0
    while target.exists() or target.with_name(target.name + ".part").exists():
        n += 1
        target = dest / numbered(name, n)
    return target


async def _pull_one(afc, remote: str, dest: Path) -> Path:
    """Copies one file as `name.part`, checks its size, then renames it;
    the partial file is removed on any failure or cancel."""
    name = safe_name(remote.rsplit("/", 1)[1]) or "file"
    total = int((await afc.stat(remote)).get("st_size") or 0)
    target = unique_local(dest, name)
    part = target.with_name(target.name + ".part")
    job.update(name=name, total=total, sent=0, speed=0, file_started=time.monotonic())
    handle = await afc.fopen(remote, "r")
    ok = False
    try:
        with part.open("xb") as f:
            while True:
                _check_cancel()
                chunk = await afc.fread(handle, CHUNK)
                if not chunk:
                    break
                f.write(chunk)
                _progress(len(chunk))
        if part.stat().st_size != total:
            raise RuntimeError(f"size on the PC {part.stat().st_size} ≠ {total}")
        part.replace(target)
        ok = True
    finally:
        try:
            await afc.fclose(handle)
        except Exception:  # noqa: BLE001
            pass
        if not ok:
            part.unlink(missing_ok=True)
    return target


async def pull_many(udid: str, source: str, items: list[str], dest: Path) -> int:
    """Copies device files (`source` "media" = DCIM, else an app bundle id)
    into `dest`, as they are (HEIC stays HEIC, Live Photos come as their
    photo and video files). Returns how many arrived complete."""
    from pymobiledevice3.services.afc import AfcService
    from pymobiledevice3.services.house_arrest import HouseArrestService

    root = MEDIA_ROOT if source == "media" else DOCUMENTS
    items = [p for p in items if device_path_ok(p, root)]
    dest.mkdir(parents=True, exist_ok=True)
    _begin("import", len(items), dest=str(dest))
    lockdown = await _lockdown(udid)
    try:
        if source == "media":
            afc = AfcService(lockdown)
            await afc.__aenter__()
        else:
            afc = await HouseArrestService.create(lockdown, source, documents_only=True)
        try:
            for i, remote in enumerate(items):
                _check_cancel()
                job["index"] = i + 1
                await _pull_one(afc, remote, dest)
                job["done"] += 1
        finally:
            await afc.close()
    finally:
        await lockdown.close()
    return job["done"]


def start(loop: asyncio.AbstractEventLoop | None, coro_factory) -> None:
    """Runs a transfer as a task; the result lands in `job`. File names never
    go to the log (only counts and sizes)."""

    async def run() -> None:
        t = time.monotonic()
        try:
            done = await coro_factory()
            job.update(state="done")
            log.info("files: %s %d of %d file(s) in %.1f s", job.get("direction"), done, job.get("count", 0),
                     time.monotonic() - t)
        except Cancelled:
            job.update(state="cancelled", message="")
            log.info("files: %s cancelled after %d file(s)", job.get("direction"), job.get("done", 0))
        except Exception as error:  # reported to the window, never fatal
            log.warning("files: %s failed after %d file(s): %s", job.get("direction"), job.get("done", 0),
                        type(error).__name__)
            job.update(state="error", message=_short(error))

    job.update(state="running", direction="", name="", sent=0, total=0, index=0, count=0, done=0, message="")
    asyncio.ensure_future(run(), loop=loop)


def _short(error: Exception) -> str:
    text = str(error) or type(error).__name__
    return re.sub(r"\s+", " ", text)[:200]
