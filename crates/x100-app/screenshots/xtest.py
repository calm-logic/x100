"""Drives a window through the X server, with ctypes over libX11 and libXtst: input for the window
check where no input tool such as xdotool is installed. The pointer moves and clicks through the
XTEST extension. Keys are sent to the window itself with XSendEvent, since under WSLg XTEST's keys
go to whichever window Windows has in the foreground, which a newly opened window often is not.

Usage: xtest.py WINDOW_ID command...
  click X Y         click at window-relative coordinates
  scroll X Y N      N wheel notches down (up if negative) at a point
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


# Xlib's XKeyEvent, to send a key to the window directly.
class XKeyEvent(ctypes.Structure):
    _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
                ("display", ctypes.c_void_p), ("window", ctypes.c_ulong), ("root", ctypes.c_ulong),
                ("subwindow", ctypes.c_ulong), ("time", ctypes.c_ulong), ("x", ctypes.c_int),
                ("y", ctypes.c_int), ("x_root", ctypes.c_int), ("y_root", ctypes.c_int),
                ("state", ctypes.c_uint), ("keycode", ctypes.c_uint), ("same_screen", ctypes.c_int)]


class XEvent(ctypes.Union):
    _fields_ = [("xkey", XKeyEvent), ("pad", ctypes.c_long * 24)]


x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
TARGET = [0]


def send(code, down, state):
    e = XEvent()
    e.xkey.type = 2 if down else 3
    e.xkey.display = d
    e.xkey.window = TARGET[0]
    e.xkey.root = x11.XDefaultRootWindow(d)
    e.xkey.time = 0
    e.xkey.x = e.xkey.y = e.xkey.x_root = e.xkey.y_root = 1
    e.xkey.state = state
    e.xkey.keycode = code
    e.xkey.same_screen = 1
    mask = 1 if down else 2  # KeyPressMask or KeyReleaseMask
    x11.XSendEvent(d, TARGET[0], 1, mask, ctypes.byref(e))


def press(keysym_name, shift=False):
    keysym = x11.XStringToKeysym(keysym_name.encode())
    code = x11.XKeysymToKeycode(d, keysym)
    if not code:
        sys.exit(f"no key for {keysym_name}")
    # A keysym that is not the key's unshifted one needs Shift.
    shift = shift or x11.XKeycodeToKeysym(d, code, 0) != keysym
    state = 1 if shift else 0
    send(code, True, state)
    send(code, False, state)
    x11.XFlush(d)
    time.sleep(0.02)


def main():
    name, args = sys.argv[1], sys.argv[2:]
    ox, oy = origin(name)
    TARGET[0] = int(name, 16)
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
        elif cmd == "scroll":
            # scroll X Y N: the wheel N notches down at a point.
            x, y, n, args = int(args[0]), int(args[1]), int(args[2]), args[3:]
            xtst.XTestFakeMotionEvent(d, -1, ox + x, oy + y, 0)
            x11.XFlush(d)
            time.sleep(0.1)
            for _ in range(abs(n)):
                button = 5 if n > 0 else 4
                xtst.XTestFakeButtonEvent(d, button, 1, 0)
                xtst.XTestFakeButtonEvent(d, button, 0, 0)
                x11.XFlush(d)
                time.sleep(0.08)
            time.sleep(0.3)
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
