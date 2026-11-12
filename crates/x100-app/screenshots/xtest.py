"""Drives a window through the X server's XTEST extension, with ctypes over libX11 and libXtst:
input for the window check where no input tool such as xdotool is installed.

Usage: xtest.py WINDOW_ID command...
  click X Y         click at window-relative coordinates
  type TEXT         type text (letters, digits, punctuation, spaces)
  key NAME          press a key, e.g. Return or shift+Return
  sleep SECONDS
"""
import ctypes
import subprocess
import sys
import time

x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
x11.XKeysymToKeycode.restype = ctypes.c_ubyte
x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XKeycodeToKeysym.restype = ctypes.c_ulong
x11.XKeycodeToKeysym.argtypes = [ctypes.c_void_p, ctypes.c_ubyte, ctypes.c_int]
x11.XFlush.argtypes = [ctypes.c_void_p]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]

d = x11.XOpenDisplay(None)
if not d:
    sys.exit("no display")

NAMES = {" ": "space", ",": "comma", ".": "period", ":": "colon", "?": "question", "!": "exclam",
         "-": "minus", "'": "apostrophe", "/": "slash", "(": "parenleft", ")": "parenright"}


def origin(window):
    out = subprocess.run(["xwininfo", "-id", window], capture_output=True, text=True).stdout
    get = lambda key: int(next(l for l in out.splitlines() if key in l).split(":")[1])
    return get("Absolute upper-left X"), get("Absolute upper-left Y")


def press(keysym_name, shift=False):
    keysym = x11.XStringToKeysym(keysym_name.encode())
    code = x11.XKeysymToKeycode(d, keysym)
    if not code:
        sys.exit(f"no key for {keysym_name}")
    # A keysym that is not the key's unshifted one needs Shift.
    shift = shift or x11.XKeycodeToKeysym(d, code, 0) != keysym
    shift_code = x11.XKeysymToKeycode(d, x11.XStringToKeysym(b"Shift_L"))
    if shift:
        xtst.XTestFakeKeyEvent(d, shift_code, 1, 0)
    xtst.XTestFakeKeyEvent(d, code, 1, 0)
    xtst.XTestFakeKeyEvent(d, code, 0, 0)
    if shift:
        xtst.XTestFakeKeyEvent(d, shift_code, 0, 0)
    x11.XFlush(d)
    time.sleep(0.02)


def main():
    name, args = sys.argv[1], sys.argv[2:]
    ox, oy = origin(name)
    while args:
        cmd, args = args[0], args[1:]
        if cmd == "click":
            x, y, args = int(args[0]), int(args[1]), args[2:]
            xtst.XTestFakeMotionEvent(d, -1, ox + x, oy + y, 0)
            x11.XFlush(d)
            time.sleep(0.1)
            xtst.XTestFakeButtonEvent(d, 1, 1, 0)
            xtst.XTestFakeButtonEvent(d, 1, 0, 0)
            x11.XFlush(d)
            time.sleep(0.2)
        elif cmd == "type":
            for ch in args[0]:
                press(NAMES.get(ch, ch))
            args = args[1:]
        elif cmd == "key":
            parts = args[0].split("+")
            press(parts[-1], shift="shift" in parts[:-1])
            args = args[1:]
        elif cmd == "sleep":
            time.sleep(float(args[0]))
            args = args[1:]
        else:
            sys.exit(f"unknown command {cmd}")


main()
