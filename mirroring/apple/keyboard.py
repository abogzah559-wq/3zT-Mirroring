"""Keyboard shortcuts sent to the device through the Apple stream server's
HID endpoints (/key with HID keyboard usages, /button with named buttons).

Actions:
  copy, paste, cut, select-all, undo   Cmd + C / V / X / A / Z on the device
  paste-pc                             type the PC clipboard text (US layout)
  toggle-keyboard                      show / hide the on-screen keyboard (Eject)
  lock                                 restore brightness if dimmed, then sleep
  wake / auto-unlock                   unlock: only when the device is locked,
                                       wake, swipe up, type the saved passcode
                                       (a numeric code, or an alphanumeric
                                       password followed by Return), then check
                                       that it unlocked (one retry at most)
  stealth                              dim the device's own screen to 0% / restore
                                       (the mirrored picture stays full quality)
"""

from __future__ import annotations

import asyncio
from typing import Awaitable, Callable, Optional

import aiohttp

CMD = 0xE3
SHIFT = 0xE1
_CMD_KEYS = {"copy": 0x06, "paste": 0x19, "cut": 0x1B, "select-all": 0x04, "undo": 0x1D}
_BUTTONS = {"toggle-keyboard": "eject"}
_state = {"dimmed": False}
_DIGITS = {str(d): 0x1E + d - 1 for d in range(1, 10)} | {"0": 0x27}
MAX_TEXT = 4000

_PLAIN = "1234567890-=[]\\;',./`"
_SHIFTED = "!@#$%^&*()_+{}|:\"<>?~"
_PLAIN_CODES = [0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
                0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x33, 0x34, 0x36, 0x37, 0x38, 0x35]


def char_usage(ch: str) -> tuple[int, bool] | None:
    """(HID usage, needs shift) for one character on a US layout."""
    if "a" <= ch <= "z":
        return 0x04 + ord(ch) - ord("a"), False
    if "A" <= ch <= "Z":
        return 0x04 + ord(ch) - ord("A"), True
    if ch in _PLAIN:
        return _PLAIN_CODES[_PLAIN.index(ch)], False
    if ch in _SHIFTED:
        return _PLAIN_CODES[_SHIFTED.index(ch)], True
    return {" ": (0x2C, False), "\n": (0x28, False), "\t": (0x2B, False)}.get(ch)


MAX_PASSCODE = 32
ENTER = 0x28


def clean_passcode(raw: str) -> str:
    """The saved passcode: up to 32 characters 3zT Mirroring can type on a US
    keyboard (iOS numeric codes, or an alphanumeric password)."""
    return "".join(ch for ch in raw if ch not in "\r\n\t" and char_usage(ch) is not None)[:MAX_PASSCODE]


def tap_reports(report: list[int]) -> list[list[int]]:
    """The HID reports of one key tap. iOS ignores a modifier that arrives in
    the same report as the key (iOS 27.2: Shift+A typed "a"), so modifiers go
    down first, are held with the key, and come up last."""
    mods = [u for u in report if u >= 0xE0]
    if mods and len(mods) < len(report):
        return [mods, list(report), mods, []]
    return [list(report), []]


def passcode_keys(code: str) -> list[list[int]]:
    """HID key reports that type `code` on the lock screen. A 4- or 6-digit
    code unlocks by itself; any other code needs Return."""
    reports: list[list[int]] = []
    for ch in code:
        mapped = char_usage(ch)
        if mapped is None:
            continue
        usage, shift = mapped
        reports.append([SHIFT, usage] if shift else [usage])
    if code and not (code.isdigit() and len(code) in (4, 6)):
        reports.append([ENTER])
    return reports


def install_buttons() -> None:
    """Add Eject (on-screen keyboard toggle) to the stream server's buttons."""
    from pymobiledevice3.remote.core_device import screen_stream

    screen_stream._NAMED_BUTTONS.setdefault("eject", (0x0C, 0xB8, 0.05))
    screen_stream._NAMED_BUTTONS.setdefault("brightness-down", (0x0C, 0x70, 0.025))
    screen_stream._NAMED_BUTTONS.setdefault("brightness-up", (0x0C, 0x6F, 0.025))
    _state["dimmed"] = False


# Seconds between the swipe up and the first passcode key. Face ID devices
# try the face first and show the keypad after it; keys sent earlier are lost.
UNLOCK_KEYPAD_WAIT = (1.2, 2.0)  # first try, retry
UNLOCK_CHECK_SECONDS = 4.0

LockCheck = Callable[[], Awaitable[Optional[bool]]]


async def _no_lock_check() -> Optional[bool]:
    return None


async def unlock(button, touch, key, passcode: str, is_locked: LockCheck, auto: bool = False) -> tuple[bool, str]:
    """Wake → keypad → passcode → check. Messages: "already-unlocked",
    "woken" (no passcode saved), "unlocked", "unverified" (typed, but the
    lock state can't be read), "unknown-state" (automatic unlock skipped:
    the lock state can't be read, and typing the passcode into an unlocked
    device would put it in whatever app is open), "failed". At most one
    retry: wrong attempts count on the device and too many disable it."""
    locked = await is_locked()
    if locked is False:
        return True, "already-unlocked"
    if locked is None and auto:
        return False, "unknown-state"
    code = clean_passcode(passcode)

    async def swipe_up():
        await button("home")
        await asyncio.sleep(0.45)
        await touch("contact", 32768, 58000)
        for i in range(1, 9):
            await asyncio.sleep(0.025)
            await touch("contact", 32768, int(58000 - 46000 * i / 8))
        await touch("release", 32768, 12000)

    await swipe_up()
    if not code:
        return True, "woken"
    for attempt, wait in enumerate(UNLOCK_KEYPAD_WAIT):
        if attempt:
            await swipe_up()
        await asyncio.sleep(wait)
        for report in passcode_keys(code):
            for step in tap_reports(report):
                await key(step)
                await asyncio.sleep(0.03)
            await asyncio.sleep(0.04)
        state = None
        deadline = asyncio.get_running_loop().time() + UNLOCK_CHECK_SECONDS
        while asyncio.get_running_loop().time() < deadline:
            await asyncio.sleep(0.4)
            state = await is_locked()
            if state is not True:
                break
        if state is False:
            return True, "unlocked"
        if state is None:
            return True, "unverified"
    return False, "failed"


async def run(client: aiohttp.ClientSession, base: str, action: str, text: str = "", passcode: str = "",
              is_locked: LockCheck = _no_lock_check) -> tuple[bool, str]:
    async def key(usages):
        async with client.post(f"{base}/key", json={"usages": usages}) as r:
            if r.status >= 400:
                raise RuntimeError(f"key {r.status}")

    async def button(name):
        async with client.post(f"{base}/button", json={"name": name, "state": "press"}) as r:
            if r.status >= 400:
                raise RuntimeError(f"button {r.status}")

    async def touch(kind, x, y):
        async with client.post(f"{base}/touch", json={"type": kind, "x": x, "y": y}) as r:
            if r.status >= 400:
                raise RuntimeError(f"touch {r.status}")

    if action == "stealth":
        # 16 steps down reach 0%; restoring sends 4 steps up (~25%), and only
        # when this app dimmed it, so the screen never jumps to full.
        if not _state["dimmed"]:
            for _ in range(16):
                await button("brightness-down")
                await asyncio.sleep(0.03)
            _state["dimmed"] = True
            return True, "dimmed"
        for _ in range(4):
            await button("brightness-up")
            await asyncio.sleep(0.03)
        _state["dimmed"] = False
        return True, "restored"
    if action == "lock":
        if _state["dimmed"]:
            await run(client, base, "stealth")
        await button("lock")
        return True, ""
    if action in ("wake", "auto-unlock"):
        return await unlock(button, touch, key, passcode, is_locked, auto=action == "auto-unlock")

    if action in _CMD_KEYS:
        await key([])
        await key([CMD])
        await asyncio.sleep(0.03)
        await key([CMD, _CMD_KEYS[action]])
        await asyncio.sleep(0.05)
        await key([CMD])
        await key([])
        return True, ""
    if action in _BUTTONS:
        async with client.post(f"{base}/button", json={"name": _BUTTONS[action], "state": "press"}) as r:
            return r.status < 400, "" if r.status < 400 else f"button {r.status}"
    if action == "paste-pc":
        text = text.replace("\r\n", "\n")[:MAX_TEXT]
        skipped = 0
        for ch in text:
            mapped = char_usage(ch)
            if mapped is None:
                skipped += 1
                continue
            usage, shift = mapped
            for step in tap_reports([SHIFT, usage] if shift else [usage]):
                await key(step)
                await asyncio.sleep(0.006)
        note = f"{skipped} characters can't be typed on a US keyboard and were skipped." if skipped else ""
        return True, note
    return False, f"Unknown action: {action}"


async def device_locked(udid: str) -> Optional[bool]:
    """Whether the device is locked now (lockdown's PasswordProtected, which
    reflects the current lock state), or None when it can't be read."""
    try:
        from pymobiledevice3.lockdown import create_using_usbmux

        lockdown = await asyncio.wait_for(create_using_usbmux(serial=udid, autopair=False), 5)
    except Exception:  # noqa: BLE001 - no usbmux route (Wi-Fi only), not paired...
        return None
    try:
        value = await asyncio.wait_for(lockdown.get_value(key="PasswordProtected"), 5)
        return value if isinstance(value, bool) else None
    except Exception:  # noqa: BLE001
        return None
    finally:
        try:
            await lockdown.close()
        except Exception:  # noqa: BLE001
            pass
