"""Server benchmark: run a server, wait until it is ready, and sample memory, CPU and MSPT.

Runs the same way for RustGot and Purpur so the two can be compared on the same world:

    python tools/rustgot-bench/bench.py --name rustgot --workdir run/rustgot -- path/to/rustgot
    python tools/rustgot-bench/bench.py --name purpur  --workdir run/purpur  -- java -Xmx4G -jar purpur.jar nogui

The server is stopped with `stop` on stdin after the measurement. Results are appended to
`results.jsonl` next to this script.

With `--bots N`, `rustgot-loadbot` (build it with `cargo build --release -p rustgot-loadbot`)
joins N offline-mode bots that fly outward and load fresh chunks; sampling starts after
`--warmup` seconds. Servers must run with online mode off, and Paper/Purpur also need
`allow-flight=true`.

MSPT is read from the server's own console command every 5 seconds: `tps` on RustGot (average
of the last 100 ticks) and `mspt` on Paper/Purpur (5 second average and max). Both count only
the time spent ticking, not the sleep between ticks. CPU is the whole process tree, in cores.
"""

import argparse
import json
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

import psutil

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_LOADBOT = REPO_ROOT / "target" / "release" / ("rustgot-loadbot.exe" if sys.platform == "win32" else "rustgot-loadbot")

# Ready line -> server kind, which decides how MSPT is read.
READY_PATTERNS = [
    (re.compile(r"Started server; took"), "rustgot"),  # RustGot / Pumpkin
    (re.compile(r"Done \([\d.,]+s\)!"), "paper"),  # Paper / Purpur / vanilla
]
MSPT_COMMANDS = {"rustgot": "tps", "paper": "mspt"}
NUMBER = r"(\d+(?:[.,]\d+)?)"  # Paper prints numbers in the system locale (e.g. "0,5")
RUSTGOT_MSPT = re.compile(rf"MSPT: {NUMBER}ms")
PAPER_MSPT = re.compile(rf"{NUMBER}/{NUMBER}/{NUMBER}")  # first triple = last 5s avg/min/max

ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")
JOIN_PATTERN = re.compile(r"\b(bot\d+) joined the game")
LEAVE_PATTERN = re.compile(r"\b(bot\d+) (?:left the game|lost connection)")


def number(text: str) -> float:
    return float(text.replace(",", "."))


def tree(proc: psutil.Process) -> list[psutil.Process]:
    try:
        return [proc, *proc.children(recursive=True)]
    except psutil.NoSuchProcess:
        return []


def tree_rss(proc: psutil.Process) -> int:
    """RSS of the process and all its children, in bytes."""
    total = 0
    for p in tree(proc):
        try:
            total += p.memory_info().rss
        except psutil.NoSuchProcess:
            pass
    return total


def tree_cpu_seconds(proc: psutil.Process) -> float:
    """User + system CPU time of the process and all its children."""
    total = 0.0
    for p in tree(proc):
        try:
            t = p.cpu_times()
            total += t.user + t.system
        except psutil.NoSuchProcess:
            pass
    return total


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--name", required=True, help="label for the result, e.g. rustgot or purpur")
    parser.add_argument("--workdir", required=True, type=Path, help="server directory (world, configs)")
    parser.add_argument("--duration", type=float, default=60, help="seconds to sample after the server is ready")
    parser.add_argument("--interval", type=float, default=1, help="seconds between memory samples")
    parser.add_argument("--timeout", type=float, default=300, help="max seconds to wait for startup")
    parser.add_argument("--bots", type=int, default=0, help="number of load bots to connect")
    parser.add_argument("--bot-fly-secs", type=int, default=60, help="seconds each bot flies outward")
    parser.add_argument("--warmup", type=float, default=0, help="seconds between starting bots and sampling")
    parser.add_argument("--addr", default="127.0.0.1:25565", help="server address for the bots")
    parser.add_argument("--loadbot", type=Path, default=DEFAULT_LOADBOT, help="path to rustgot-loadbot")
    parser.add_argument("--mspt-interval", type=float, default=5, help="seconds between MSPT queries (0 = off)")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="server command, after --")
    args = parser.parse_args()

    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("missing server command after --")

    args.workdir.mkdir(parents=True, exist_ok=True)
    (args.workdir / "eula.txt").write_text("eula=true\n")

    start = time.monotonic()
    server = subprocess.Popen(
        command,
        cwd=args.workdir,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    ready = threading.Event()
    kind: list[str] = []
    joined: set[str] = set()
    left: set[str] = set()
    mspt_avg: list[float] = []
    mspt_max: list[float] = []
    collecting_mspt = threading.Event()

    def pump_output() -> None:
        assert server.stdout is not None
        for line in server.stdout:
            sys.stdout.write(f"[{args.name}] {line}")
            line = ANSI_ESCAPE.sub("", line)
            if not ready.is_set():
                for pattern, server_kind in READY_PATTERNS:
                    if pattern.search(line):
                        kind.append(server_kind)
                        ready.set()
            if m := JOIN_PATTERN.search(line):
                joined.add(m.group(1))
            if m := LEAVE_PATTERN.search(line):
                left.add(m.group(1))
            if collecting_mspt.is_set():
                if m := RUSTGOT_MSPT.search(line):
                    mspt_avg.append(number(m.group(1)))
                elif "TPS from" not in line and (m := PAPER_MSPT.search(line)):
                    mspt_avg.append(number(m.group(1)))
                    mspt_max.append(number(m.group(3)))

    threading.Thread(target=pump_output, daemon=True).start()

    deadline = time.monotonic() + args.timeout
    while not ready.wait(1):
        if server.poll() is not None:
            print(f"server exited with code {server.returncode} before it was ready", file=sys.stderr)
            return 1
        if time.monotonic() > deadline:
            server.kill()
            print(f"server did not become ready within {args.timeout}s", file=sys.stderr)
            return 1
    startup_s = time.monotonic() - start

    assert server.stdin is not None

    def send(console_command: str) -> None:
        try:
            server.stdin.write(console_command + "\n")
            server.stdin.flush()
        except OSError:
            pass

    bots = None
    if args.bots > 0:
        bots = subprocess.Popen(
            [str(args.loadbot), args.addr, str(args.bots), str(args.bot_fly_secs)],
            stdout=subprocess.DEVNULL,
        )
    time.sleep(args.warmup)

    proc = psutil.Process(server.pid)
    samples = []
    cpu_start, wall_start = tree_cpu_seconds(proc), time.monotonic()
    collecting_mspt.set()
    next_mspt = time.monotonic() + args.mspt_interval
    end = time.monotonic() + args.duration
    while time.monotonic() < end and server.poll() is None:
        samples.append(tree_rss(proc))
        if args.mspt_interval > 0 and time.monotonic() >= next_mspt:
            send(MSPT_COMMANDS[kind[0]])
            next_mspt += args.mspt_interval
        time.sleep(args.interval)
    cpu_cores = (tree_cpu_seconds(proc) - cpu_start) / max(time.monotonic() - wall_start, 1e-9)
    time.sleep(1)  # let the last MSPT reply arrive
    collecting_mspt.clear()

    if bots is not None:
        bots.kill()
    bots_online = len(joined - left)

    send("stop")
    try:
        server.wait(timeout=60)
    except subprocess.TimeoutExpired:
        server.kill()

    if not samples:
        print("server exited before any sample was taken", file=sys.stderr)
        return 1

    mib = 1024 * 1024
    result = {
        "name": args.name,
        "time": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "startup_s": round(startup_s, 2),
        "rss_avg_mib": round(sum(samples) / len(samples) / mib, 1),
        "rss_peak_mib": round(max(samples) / mib, 1),
        "cpu_cores_avg": round(cpu_cores, 2),
        "mspt_avg": round(sum(mspt_avg) / len(mspt_avg), 2) if mspt_avg else None,
        "mspt_worst_window_avg": round(max(mspt_avg), 2) if mspt_avg else None,
        "mspt_max_tick": round(max(mspt_max), 2) if mspt_max else None,
        "mspt_samples": len(mspt_avg),
        "samples": len(samples),
        "bots": args.bots,
        "warmup_s": args.warmup,
        "bots_joined": len(joined),
        "bots_online_at_end": bots_online,
    }
    with open(Path(__file__).with_name("results.jsonl"), "a", encoding="utf-8") as f:
        f.write(json.dumps(result) + "\n")
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
