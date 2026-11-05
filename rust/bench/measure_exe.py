"""Measure an app build from the outside: startup, idle CPU and memory.

    python rust/bench/measure_exe.py <exe> <library folder> [runs]

The exe is copied into the library folder (both builds read sounds/ and config.json next to
themselves), started, and watched:
  visible   ms from launch until its main window is on screen
  settled   ms from launch until it stops using CPU (no more than 2 ms of CPU in 150 ms)
  idle_cpu  % of one core used over 10 s after settling (window open, nothing happening)
  ws_mb / private_mb   working set and private bytes after the idle period
The first run is "cold" (first launch of that copy), the rest "warm". The library configs name
audio devices that don't exist, so neither build opens a real mic, cable or speaker.
Prints one JSON line per run. Don't run while gaming: the windows open on screen.
"""
import ctypes
import ctypes.wintypes as wt
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

user32, kernel32, psapi = ctypes.windll.user32, ctypes.windll.kernel32, ctypes.windll.psapi
PROCESS_QUERY = 0x1000 | 0x0010


class PMC(ctypes.Structure):
    _fields_ = [("cb", wt.DWORD), ("PageFaultCount", wt.DWORD), ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t), ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t), ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t), ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t), ("PrivateUsage", ctypes.c_size_t)]


def cpu_ms(h):
    c, e, k, u = wt.FILETIME(), wt.FILETIME(), wt.FILETIME(), wt.FILETIME()
    kernel32.GetProcessTimes(h, ctypes.byref(c), ctypes.byref(e), ctypes.byref(k), ctypes.byref(u))
    f = lambda t: (t.dwHighDateTime << 32 | t.dwLowDateTime) / 10000  # noqa: E731
    return f(k) + f(u)


def memory(h):
    m = PMC()
    m.cb = ctypes.sizeof(PMC)
    psapi.GetProcessMemoryInfo(h, ctypes.byref(m), m.cb)
    return m.WorkingSetSize / 2**20, m.PrivateUsage / 2**20


def windows_of(pid):
    found = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        p = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(p))
        if p.value == pid and user32.IsWindowVisible(hwnd):
            n = ctypes.create_unicode_buffer(200)
            user32.GetWindowTextW(hwnd, n, 200)
            if n.value.startswith("JondisBoard"):
                found.append(hwnd)
        return True
    user32.EnumWindows(cb, 0)
    return found


class PE32(ctypes.Structure):
    _fields_ = [("dwSize", wt.DWORD), ("cntUsage", wt.DWORD), ("th32ProcessID", wt.DWORD),
                ("th32DefaultHeapID", ctypes.c_size_t), ("th32ModuleID", wt.DWORD), ("cntThreads", wt.DWORD),
                ("th32ParentProcessID", wt.DWORD), ("pcPriClassBase", ctypes.c_long), ("dwFlags", wt.DWORD),
                ("szExeFile", ctypes.c_wchar * 260)]


def all_pids(root_pid):
    """The process and its children (PyInstaller's one-file exe runs the app in a child)."""
    kernel32.CreateToolhelp32Snapshot.restype = ctypes.c_void_p
    snap = kernel32.CreateToolhelp32Snapshot(2, 0)
    e = PE32()
    e.dwSize = ctypes.sizeof(PE32)
    kids = {}
    ok = kernel32.Process32FirstW(ctypes.c_void_p(snap), ctypes.byref(e))
    while ok:
        kids.setdefault(e.th32ParentProcessID, []).append(e.th32ProcessID)
        ok = kernel32.Process32NextW(ctypes.c_void_p(snap), ctypes.byref(e))
    kernel32.CloseHandle(ctypes.c_void_p(snap))
    pids, todo = [], [root_pid]
    while todo:
        p = todo.pop()
        pids.append(p)
        todo += kids.get(p, [])
    return pids


def run_once(exe):
    t0 = time.perf_counter()
    proc = subprocess.Popen([str(exe)], cwd=str(exe.parent))
    visible = None
    hwnd = None
    deadline = t0 + 60
    while time.perf_counter() < deadline:
        for pid in all_pids(proc.pid):
            w = windows_of(pid)
            if w:
                visible, hwnd, app_pid = (time.perf_counter() - t0) * 1000, w[0], pid
                break
        if visible is not None:
            break
        time.sleep(0.005)
    if visible is None:
        proc.kill()
        return {"error": "no window"}
    h = kernel32.OpenProcess(PROCESS_QUERY, False, app_pid)
    last, quiet_since = cpu_ms(h), None
    while time.perf_counter() < deadline:
        time.sleep(0.05)
        now = cpu_ms(h)
        if now - last <= 2.0 * 0.05 / 0.15 + 0.01:
            quiet_since = quiet_since or time.perf_counter()
            if time.perf_counter() - quiet_since >= 0.15:
                break
        else:
            quiet_since = None
        last = now
    settled = (time.perf_counter() - t0) * 1000 - 150
    time.sleep(1.0)
    c0, w0 = cpu_ms(h), time.perf_counter()
    time.sleep(10.0)
    idle = (cpu_ms(h) - c0) / ((time.perf_counter() - w0) * 1000) * 100
    ws, private = memory(h)
    user32.PostMessageW(hwnd, 0x0010, 0, 0)   # WM_CLOSE: let it save and exit normally
    try:
        proc.wait(15)
    except subprocess.TimeoutExpired:
        proc.kill()
    kernel32.CloseHandle(h)
    return {"visible_ms": round(visible), "settled_ms": round(settled), "idle_cpu_pct": round(idle, 3),
            "ws_mb": round(ws, 1), "private_mb": round(private, 1)}


def main():
    exe, lib = Path(sys.argv[1]), Path(sys.argv[2])
    runs = int(sys.argv[3]) if len(sys.argv) > 3 else 3
    target = lib / exe.name
    shutil.copy2(exe, target)
    for helper in ("ffmpeg.exe", "yt-dlp.exe"):
        if (exe.parent / helper).exists() and not (lib / helper).exists():
            shutil.copy2(exe.parent / helper, lib / helper)
    for i in range(runs):
        r = run_once(target)
        r.update(run="cold" if i == 0 else "warm", exe=exe.name, sounds=lib.name)
        print(json.dumps(r), flush=True)
        time.sleep(1.0)


if __name__ == "__main__":
    main()
