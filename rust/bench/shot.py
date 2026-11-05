"""Screenshot a JondisBoard window (even behind others) with PrintWindow, for visual checks.

    python rust/bench/shot.py <out.png> [pid]

Without a pid it takes the first visible top-level window titled "JondisBoard". Uses
PW_RENDERFULLCONTENT so Direct2D content is captured. Never moves the mouse or focuses anything.
"""
import ctypes
import ctypes.wintypes as wt
import sys

from PIL import Image

user32, gdi32 = ctypes.windll.user32, ctypes.windll.gdi32
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))


def find(pid=None):
    found = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(h, _):
        p = wt.DWORD()
        user32.GetWindowThreadProcessId(h, ctypes.byref(p))
        n = ctypes.create_unicode_buffer(200)
        user32.GetWindowTextW(h, n, 200)
        if user32.IsWindowVisible(h) and n.value == "JondisBoard" and (pid is None or p.value == pid):
            found.append(h)
        return True
    user32.EnumWindows(cb, 0)
    return found[0] if found else None


def grab(hwnd):
    r = wt.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    w, h = r.right - r.left, r.bottom - r.top
    hdc = user32.GetWindowDC(hwnd)
    mdc = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, w, h)
    gdi32.SelectObject(mdc, bmp)
    user32.PrintWindow(hwnd, mdc, 2)
    buf = ctypes.create_string_buffer(w * h * 4)

    class BMI(ctypes.Structure):
        _fields_ = [("size", ctypes.c_uint32), ("w", ctypes.c_int32), ("h", ctypes.c_int32), ("planes", ctypes.c_uint16),
                    ("bits", ctypes.c_uint16), ("comp", ctypes.c_uint32), ("rest", ctypes.c_uint32 * 5)]
    bmi = BMI(ctypes.sizeof(BMI), w, -h, 1, 32, 0)
    gdi32.GetDIBits(mdc, bmp, 0, h, buf, ctypes.byref(bmi), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mdc)
    user32.ReleaseDC(hwnd, hdc)
    return Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB")


if __name__ == "__main__":
    hwnd = find(int(sys.argv[2]) if len(sys.argv) > 2 else None)
    if not hwnd:
        print("no window")
        sys.exit(1)
    grab(hwnd).save(sys.argv[1])
    print("saved", sys.argv[1])
