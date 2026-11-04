"""Run every UI test suite, each in its own process, and sum up.

    .venv\\Scripts\\python.exe tests\\run_all.py            (all suites)
    .venv\\Scripts\\python.exe tests\\run_all.py slide      (just the ones whose name contains "slide")

The app's windows open on screen while this runs (about a minute); don't run it while you're in a
game. Nothing reaches your mic, cable, speakers or clipboard: see _harness.py."""
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main():
    only = sys.argv[1:]
    suites = sorted(p for p in HERE.glob("test_*.py") if not only or any(o in p.stem for o in only))
    failed = []
    t0 = time.perf_counter()
    for p in suites:
        print(f"\n=== {p.stem[5:].replace('_', ' ')} ===", flush=True)
        try:
            code = subprocess.run([sys.executable, "-u", str(p)], cwd=HERE, timeout=180,
                                  env={**os.environ, "PYTHONIOENCODING": "utf-8"}).returncode
        except subprocess.TimeoutExpired:
            print("TIMED OUT", flush=True)
            code = 1
        if code:
            failed.append(p.stem)
    took = time.perf_counter() - t0
    print(f"\n{len(suites) - len(failed)}/{len(suites)} suites passed in {took:.0f} s"
          + (f"; failed: {', '.join(failed)}" if failed else ""))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
