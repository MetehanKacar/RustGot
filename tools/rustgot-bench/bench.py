"""Memory benchmark: run a server, wait until it is ready, and sample its RSS.

Runs the same way for RustGot and Purpur so the two can be compared on the same world:

    python tools/rustgot-bench/bench.py --name rustgot --workdir run/rustgot -- path/to/rustgot
    python tools/rustgot-bench/bench.py --name purpur  --workdir run/purpur  -- java -Xmx4G -jar purpur.jar nogui

The server is stopped with `stop` on stdin after the measurement. Results are appended to
`results.jsonl` next to this script.

With `--bots N`, `rustgot-loadbot` (build it with `cargo build --release -p rustgot-loadbot`)
joins N offline-mode bots that fly outward and load fresh chunks; sampling starts after
`--warmup` seconds. Servers must run with online mode off, and Paper/Purpur also need
`allow-flight=true`.
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

READY_PATTERNS = [
    re.compile(r"Started server; took"),  # RustGot / Pumpkin
    re.compile(r"Done \([\d.,]+s\)!"),  # Paper / Purpur / vanilla
]

ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")
JOIN_PATTERN = re.compile(r"\b(bot\d+) joined the game")
LEAVE_PATTERN = re.compile(r"\b(bot\d+) (?:left the game|lost connection)")


def tree_rss(proc: psutil.Process) -> int:
    """RSS of the process and all its children, in bytes."""
    total = 0
    for p in [proc, *proc.children(recursive=True)]:
        try:
            total += p.memory_info().rss
        except psutil.NoSuchProcess:
            pass
    return total


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--name", required=True, help="label for the result, e.g. rustgot or purpur")
    parser.add_argument("--workdir", required=True, type=Path, help="server directory (world, configs)")
    parser.add_argument("--duration", type=float, default=60, help="seconds to sample after the server is ready")
    parser.add_argument("--interval", type=float, default=1, help="seconds between samples")
    parser.add_argument("--timeout", type=float, default=300, help="max seconds to wait for startup")
    parser.add_argument("--bots", type=int, default=0, help="number of load bots to connect")
    parser.add_argument("--bot-fly-secs", type=int, default=60, help="seconds each bot flies outward")
    parser.add_argument("--warmup", type=float, default=0, help="seconds between starting bots and sampling")
    parser.add_argument("--addr", default="127.0.0.1:25565", help="server address for the bots")
    parser.add_argument("--loadbot", type=Path, default=DEFAULT_LOADBOT, help="path to rustgot-loadbot")
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
    joined = set()
    left = set()

    def pump_output() -> None:
        assert server.stdout is not None
        for line in server.stdout:
            sys.stdout.write(f"[{args.name}] {line}")
            line = ANSI_ESCAPE.sub("", line)
            if any(p.search(line) for p in READY_PATTERNS):
                ready.set()
            if m := JOIN_PATTERN.search(line):
                joined.add(m.group(1))
            if m := LEAVE_PATTERN.search(line):
                left.add(m.group(1))

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

    bots = None
    if args.bots > 0:
        bots = subprocess.Popen(
            [str(args.loadbot), args.addr, str(args.bots), str(args.bot_fly_secs)],
            stdout=subprocess.DEVNULL,
        )
    time.sleep(args.warmup)

    proc = psutil.Process(server.pid)
    samples = []
    end = time.monotonic() + args.duration
    while time.monotonic() < end and server.poll() is None:
        samples.append(tree_rss(proc))
        time.sleep(args.interval)

    if bots is not None:
        bots.kill()
    bots_online = len(joined - left)

    assert server.stdin is not None
    try:
        server.stdin.write("stop\n")
        server.stdin.flush()
        server.wait(timeout=60)
    except (OSError, subprocess.TimeoutExpired):
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
