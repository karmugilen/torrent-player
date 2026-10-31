#!/usr/bin/env python3
"""Sub-1-minute peer parity gate: Go ∥ Rust, short stream.

Wall target: <60s. Default stream 25s (early ramp already shows the peer gap).

Usage:
  peers_campaign_fast.py [seconds] [label-suffix]
  peers_campaign_fast.py 25 gate1

Env (optional):
  RUST_HOST  default ~/.cache/wt-targets/codex/release/engine-host
  GO_HOST    default ~/.cache/wt-tmp/engine-go-host
  MAGNET_FILE default /tmp/bunny-magnet.txt

Writes JSONL summary lines to stdout and
  /tmp/peer-parity-reports/bunny-fast-<suffix>.jsonl

For promotion / final claim still run serial 3×60s (peers_compare.py).
Parallel sharing of uplink/DHT can bias both engines the same way — good for
a fast gate, not for publishing absolute peer means.
"""
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]  # engine-rs/
COMPARE = pathlib.Path(__file__).resolve().parent / "peers_compare.py"
REPORT_DIR = pathlib.Path("/tmp/peer-parity-reports")


def main() -> int:
    seconds = int(sys.argv[1]) if len(sys.argv) > 1 else 25
    suffix = sys.argv[2] if len(sys.argv) > 2 else time.strftime("%H%M%S")
    if seconds < 10 or seconds > 60:
        print("seconds must be 10..60 (default 25)", file=sys.stderr)
        return 2

    rust = os.environ.get(
        "RUST_HOST",
        str(pathlib.Path.home() / ".cache/wt-targets/codex/release/engine-host"),
    )
    go = os.environ.get("GO_HOST", str(pathlib.Path.home() / ".cache/wt-tmp/engine-go-host"))
    magnet_path = pathlib.Path(os.environ.get("MAGNET_FILE", "/tmp/bunny-magnet.txt"))
    magnet = magnet_path.read_text().strip()
    if not magnet.startswith("magnet:"):
        print(f"bad magnet in {magnet_path}", file=sys.stderr)
        return 2
    for path, name in ((rust, "RUST_HOST"), (go, "GO_HOST")):
        if not pathlib.Path(path).is_file():
            print(f"missing {name}: {path}", file=sys.stderr)
            return 2

    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    out_jsonl = REPORT_DIR / f"bunny-fast-{suffix}.jsonl"
    go_label = f"go-bunny-fast-{suffix}"
    rust_label = f"rust-bunny-fast-{suffix}"

    t0 = time.monotonic()
    # Parallel: separate control ports + data dirs inside peers_compare.
    go_proc = subprocess.Popen(
        [sys.executable, str(COMPARE), go, go_label, magnet, str(seconds)],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    rust_proc = subprocess.Popen(
        [sys.executable, str(COMPARE), rust, rust_label, magnet, str(seconds)],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    go_out, _ = go_proc.communicate()
    rust_out, _ = rust_proc.communicate()
    wall = time.monotonic() - t0

    def last_json_line(blob: str) -> dict:
        for line in reversed(blob.splitlines()):
            line = line.strip()
            if line.startswith("{") and '"label"' in line and '"peers_min_max"' in line:
                return json.loads(line)
        raise RuntimeError("no result json in peers_compare output")

    rows = []
    for blob, code, label in (
        (go_out, go_proc.returncode, go_label),
        (rust_out, rust_proc.returncode, rust_label),
    ):
        (REPORT_DIR / f"{label}.log").write_text(blob)
        if code != 0:
            print(f"FAIL {label} exit={code}\n{blob[-2000:]}", file=sys.stderr)
            return code or 1
        row = last_json_line(blob)
        rows.append(row)
        with out_jsonl.open("a") as f:
            f.write(json.dumps(row) + "\n")

    go_r, rust_r = rows[0], rows[1]
    summary = {
        "wall_s": round(wall, 1),
        "stream_s": seconds,
        "go": {
            "peers_min_max": go_r.get("peers_min_max"),
            "avg_mib_s": go_r.get("avg_mib_s"),
            "conn_n": (go_r.get("conn_summary") or {}).get("n"),
            "transports": (go_r.get("conn_summary") or {}).get("transports"),
        },
        "rust": {
            "peers_min_max": rust_r.get("peers_min_max"),
            "avg_mib_s": rust_r.get("avg_mib_s"),
            "conn_n": (rust_r.get("conn_summary") or {}).get("n"),
            "transports": (rust_r.get("conn_summary") or {}).get("transports"),
            "final_swarm": (rust_r.get("samples") or [{}])[-1].get("swarm")
            if rust_r.get("samples")
            else rust_r.get("final_swarm"),
        },
        "under_60s": wall < 60,
        "jsonl": str(out_jsonl),
    }
    # Prefer last sample swarm from rust result file if present
    rust_result = (
        pathlib.Path.home() / ".cache/wt-tmp" / f"peers-{rust_label}" / "result.json"
    )
    if rust_result.is_file():
        full = json.loads(rust_result.read_text())
        samples = full.get("samples") or []
        if samples and samples[-1].get("swarm"):
            summary["rust"]["final_swarm"] = samples[-1]["swarm"]
        summary["rust"]["peers_series"] = [s["peers"] for s in samples]
    go_result = pathlib.Path.home() / ".cache/wt-tmp" / f"peers-{go_label}" / "result.json"
    if go_result.is_file():
        full = json.loads(go_result.read_text())
        summary["go"]["peers_series"] = [s["peers"] for s in (full.get("samples") or [])]

    print(json.dumps(summary, indent=2))
    gmax = (go_r.get("peers_min_max") or [0, 0])[1]
    rmax = (rust_r.get("peers_min_max") or [0, 0])[1]
    print(
        f"wall={wall:.1f}s stream={seconds}s  "
        f"Go max={gmax} Rust max={rmax}  "
        f"{'OK under 60s' if wall < 60 else 'OVER 60s'}",
        flush=True,
    )
    return 0 if wall < 90 else 1  # soft: still succeed if slightly over; print flag


if __name__ == "__main__":
    raise SystemExit(main())
