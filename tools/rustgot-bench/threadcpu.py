"""Per-thread CPU breakdown of a running server (Windows thread names, any OS for totals).

    python tools/rustgot-bench/threadcpu.py --process rustgot.exe --after 120 --window 30

Waits until a process whose name matches `--process` has been running for `--after` seconds,
then measures how much CPU each thread uses over `--window` seconds. Threads are grouped by
name with trailing numbers stripped (`Rayon-Worker-3` -> `Rayon-Worker-#`). Thread names come
from `GetThreadDescription` on Windows; elsewhere every thread is reported by id.
"""

import argparse
import ctypes
import re
import sys
import time
from collections import defaultdict

import psutil


def thread_name(tid: int) -> str:
    if sys.platform != "win32":
        return f"tid-{tid}"
    kernel32 = ctypes.windll.kernel32
    handle = kernel32.OpenThread(0x0800, False, tid)  # THREAD_QUERY_LIMITED_INFORMATION
    if not handle:
        return "?"
    try:
        text = ctypes.c_wchar_p()
        if kernel32.GetThreadDescription(handle, ctypes.byref(text)) < 0 or not text.value:
            return "(unnamed)"
        name = text.value
        kernel32.LocalFree(text)
        return name
    finally:
        kernel32.CloseHandle(handle)


def snapshot(proc: psutil.Process) -> dict[int, float]:
    return {t.id: t.user_time + t.system_time for t in proc.threads()}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--process", required=True, help="process name substring, e.g. rustgot or java")
    parser.add_argument("--after", type=float, default=0, help="seconds of process uptime to wait for")
    parser.add_argument("--window", type=float, default=30, help="measurement window in seconds")
    parser.add_argument("--top", type=int, default=15)
    args = parser.parse_args()

    proc = None
    while proc is None:
        for p in psutil.process_iter(["name", "create_time"]):
            if args.process.lower() in (p.info["name"] or "").lower():
                proc = p
                break
        else:
            time.sleep(1)

    while time.time() - proc.create_time() < args.after:
        time.sleep(1)

    names = {tid: thread_name(tid) for tid in snapshot(proc)}
    before = snapshot(proc)
    t0 = time.monotonic()
    time.sleep(args.window)
    after = snapshot(proc)
    elapsed = time.monotonic() - t0
    for tid in after:
        names.setdefault(tid, thread_name(tid))

    groups: dict[str, list[float]] = defaultdict(list)
    for tid, cpu in after.items():
        group = re.sub(r"\d+", "#", names.get(tid, "?"))
        groups[group].append((cpu - before.get(tid, 0.0)) / elapsed)

    total = sum(sum(v) for v in groups.values())
    print(f"{proc.name()} pid={proc.pid}: {total:.2f} cores over {elapsed:.0f}s, {len(after)} threads")
    for group, cores in sorted(groups.items(), key=lambda kv: -sum(kv[1]))[: args.top]:
        print(f"  {sum(cores):6.2f} cores  {len(cores):4d} threads  {group}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
