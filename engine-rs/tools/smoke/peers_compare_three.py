#!/usr/bin/env python3
"""Serial three-way peer/speed gate: Go → our Rust → rqbit.

One engine at a time (no shared bandwidth). Default 20s stream each.

Usage:
  peers_compare_three.py [seconds] [label-suffix]

Env:
  GO_HOST     default ~/.cache/wt-tmp/engine-go-host
  RUST_HOST   default ~/.cache/wt-targets/codex/release/engine-host
  RQBIT_BIN   default ~/.cache/wt-tmp/rqbit-bin/rqbit
  MAGNET_FILE default /tmp/bunny-magnet.txt

Writes /tmp/peer-parity-reports/bunny-three-<suffix>.jsonl + summary JSON on stdout.
"""
from __future__ import annotations

import collections
import json
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

COMPARE = pathlib.Path(__file__).resolve().parent / "peers_compare.py"
REPORT_DIR = pathlib.Path("/tmp/peer-parity-reports")


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def http_json(url: str, method: str = "GET", data=None, timeout: float = 10):
    body = None
    headers = {}
    if data is not None:
        if isinstance(data, (dict, list)):
            body = json.dumps(data).encode()
            headers["Content-Type"] = "application/json"
        elif isinstance(data, str):
            body = data.encode()
        else:
            body = data
    req = urllib.request.Request(url, method=method, data=body, headers=headers)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        raw = resp.read()
        if not raw:
            return None
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return raw


def run_our_engine(binary: str, label: str, magnet: str, seconds: int) -> dict:
    proc = subprocess.run(
        [sys.executable, str(COMPARE), binary, label, magnet, str(seconds)],
        capture_output=True,
        text=True,
    )
    log_path = REPORT_DIR / f"{label}.log"
    log_path.write_text(proc.stdout + "\n" + proc.stderr)
    if proc.returncode != 0:
        raise RuntimeError(f"{label} failed exit={proc.returncode}\n{proc.stdout[-1500:]}")
    result_path = pathlib.Path.home() / ".cache/wt-tmp" / f"peers-{label}" / "result.json"
    return json.loads(result_path.read_text())


def run_rqbit(rqbit_bin: str, label: str, magnet: str, seconds: int) -> dict:
    work = pathlib.Path.home() / ".cache/wt-tmp" / f"peers-{label}"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    data = work / "data"
    data.mkdir()
    port = free_port()
    logf = open(work / "host.log", "wb")
    env = {**os.environ, "RUST_LOG": "warn"}
    proc = subprocess.Popen(
        [
            rqbit_bin,
            "--http-api-listen-addr",
            f"127.0.0.1:{port}",
            "server",
            "start",
            "--disable-persistence",
            str(data),
        ],
        stdout=logf,
        stderr=subprocess.STDOUT,
        env=env,
    )
    result = {
        "label": label,
        "engine": "rqbit",
        "requested_seconds": seconds,
        "control_port": port,
        "samples": [],
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    base = f"http://127.0.0.1:{port}"
    stop = threading.Event()
    reader_state = {"bytes": 0}

    try:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(f"rqbit exited early: {proc.returncode}")
            try:
                http_json(f"{base}/", timeout=1)
                break
            except (OSError, urllib.error.URLError):
                time.sleep(0.1)
        else:
            raise RuntimeError("rqbit HTTP API not ready")

        t0 = time.monotonic()
        # POST magnet as raw body (rqbit API)
        add = http_json(f"{base}/torrents", method="POST", data=magnet, timeout=30)
        if not isinstance(add, dict):
            raise RuntimeError(f"add torrent failed: {add!r}")
        tid = add.get("id")
        if tid is None:
            # some versions nest
            tid = (add.get("details") or {}).get("id") or add.get("info_hash")
        if tid is None:
            raise RuntimeError(f"no torrent id in add response: {add}")

        # Wait until live / has files
        meta_deadline = time.monotonic() + 90
        details = None
        while time.monotonic() < meta_deadline:
            details = http_json(f"{base}/torrents/{tid}", timeout=5)
            files = (details or {}).get("files") or (details or {}).get("file_names") or []
            state = (details or {}).get("state") or ""
            stats = http_json(f"{base}/torrents/{tid}/stats/v1", timeout=5) or {}
            if files or stats.get("total_bytes") or state in ("live", "paused"):
                if stats.get("total_bytes") or files:
                    break
            time.sleep(0.5)
        result["metadata_s"] = round(time.monotonic() - t0, 2)
        result["add_response"] = {k: add.get(k) for k in ("id", "info_hash", "details") if k in add}
        result["torrent"] = {
            "name": (details or {}).get("name"),
            "files": (details or {}).get("files"),
        }

        # Pick largest file index for streaming (match our smoke)
        files = (details or {}).get("files") or []
        file_idx = 0
        if isinstance(files, list) and files:
            def flen(f):
                if isinstance(f, dict):
                    return f.get("length") or f.get("size") or 0
                return 0
            file_idx = max(range(len(files)), key=lambda i: flen(files[i]))

        def reader():
            try:
                url = f"{base}/torrents/{tid}/stream/{file_idx}"
                req = urllib.request.Request(url)
                with urllib.request.urlopen(req, timeout=120) as resp:
                    while not stop.is_set():
                        chunk = resp.read(1 << 16)
                        if not chunk:
                            break
                        reader_state["bytes"] += len(chunk)
            except Exception as err:
                reader_state["error"] = str(err)

        threading.Thread(target=reader, daemon=True).start()
        started = time.monotonic()
        with open(work / "diagnostics.jsonl", "w") as diag:
            for sample_at in range(5, seconds + 1, 5):
                stop.wait(max(0, started + sample_at - time.monotonic()))
                stats = http_json(f"{base}/torrents/{tid}/stats/v1", timeout=5) or {}
                peers = http_json(f"{base}/torrents/{tid}/peer_stats", timeout=5)
                live = stats.get("live") or {}
                speed = live.get("download_speed") or {}
                mbps = speed.get("mbps") if isinstance(speed, dict) else None
                # peer_stats shape varies: list or {peers: [...]} or filter object
                peer_list = []
                if isinstance(peers, list):
                    peer_list = peers
                elif isinstance(peers, dict):
                    peer_list = (
                        peers.get("peers")
                        or peers.get("live")
                        or peers.get("stats")
                        or []
                    )
                    if peer_list and isinstance(peer_list, dict):
                        # sometimes map addr -> stats
                        peer_list = list(peer_list.values())
                n_peers = len(peer_list) if peer_list else (
                    (live.get("snapshot") or {}).get("peer_stats", {}) or {}
                )
                if isinstance(n_peers, dict):
                    # snapshot peer counters
                    n_peers = int(
                        n_peers.get("live")
                        or n_peers.get("connected")
                        or sum(v for v in n_peers.values() if isinstance(v, int))
                        or 0
                    )
                sample = {
                    "elapsed_s": round(time.monotonic() - started, 2),
                    "peers": n_peers if isinstance(n_peers, int) else 0,
                    "mib_s": round(float(mbps or 0), 3),
                    "downloaded": (live.get("snapshot") or {}).get("fetched_bytes")
                    or stats.get("progress_bytes")
                    or 0,
                    "stats_state": stats.get("state"),
                    "peer_stats_raw_type": type(peers).__name__,
                    "reader": dict(reader_state),
                }
                # Better peer count from prometheus or snapshot if list empty
                snap = live.get("snapshot") or {}
                if sample["peers"] == 0 and isinstance(snap.get("peer_stats"), dict):
                    ps = snap["peer_stats"]
                    sample["peers"] = int(ps.get("live") or ps.get("connected") or 0)
                    sample["peer_counters"] = ps
                result["samples"].append(sample)
                diag.write(json.dumps(sample) + "\n")
                diag.flush()
                print(json.dumps({"label": label, "sample": sample}), flush=True)

        samples = result["samples"]
        result["peers_speed_every_5s"] = [[s["peers"], s["mib_s"]] for s in samples]
        result["avg_mib_s"] = round(sum(s["mib_s"] for s in samples) / max(len(samples), 1), 3)
        result["peak_mib_s"] = max((s["mib_s"] for s in samples), default=0)
        result["peers_min_max"] = [
            min((s["peers"] for s in samples), default=0),
            max((s["peers"] for s in samples), default=0),
        ]
        result["conn_summary"] = {
            "n": samples[-1]["peers"] if samples else 0,
            "reader_bytes": reader_state.get("bytes", 0),
        }
        result["reader"] = dict(reader_state)
    except Exception as err:
        result["error"] = f"{type(err).__name__}: {err}"
    finally:
        stop.set()
        proc.terminate()
        try:
            proc.wait(8)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        logf.close()
        result["host_exit_code"] = proc.returncode
        shutil.rmtree(data, ignore_errors=True)
        (work / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    if "error" in result:
        raise RuntimeError(result["error"])
    return result


def summarize(row: dict) -> dict:
    return {
        "label": row.get("label"),
        "peers_min_max": row.get("peers_min_max"),
        "avg_mib_s": row.get("avg_mib_s"),
        "peak_mib_s": row.get("peak_mib_s"),
        "metadata_s": row.get("metadata_s"),
        "conn_n": (row.get("conn_summary") or {}).get("n"),
        "transports": (row.get("conn_summary") or {}).get("transports"),
        "peers_series": [s["peers"] for s in (row.get("samples") or [])],
        "speed_series": [s["mib_s"] for s in (row.get("samples") or [])],
    }


def main() -> int:
    seconds = int(sys.argv[1]) if len(sys.argv) > 1 else 20
    suffix = sys.argv[2] if len(sys.argv) > 2 else time.strftime("%H%M%S")
    go = os.environ.get("GO_HOST", str(pathlib.Path.home() / ".cache/wt-tmp/engine-go-host"))
    rust = os.environ.get(
        "RUST_HOST",
        str(pathlib.Path.home() / ".cache/wt-targets/codex/release/engine-host"),
    )
    rqbit = os.environ.get(
        "RQBIT_BIN", str(pathlib.Path.home() / ".cache/wt-tmp/rqbit-bin/rqbit")
    )
    magnet = pathlib.Path(os.environ.get("MAGNET_FILE", "/tmp/bunny-magnet.txt")).read_text().strip()
    for path, name in ((go, "GO_HOST"), (rust, "RUST_HOST"), (rqbit, "RQBIT_BIN")):
        if not pathlib.Path(path).is_file():
            print(f"missing {name}: {path}", file=sys.stderr)
            return 2

    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    out = REPORT_DIR / f"bunny-three-{suffix}.jsonl"
    rows = []
    t_wall = time.monotonic()

    plan = [
        ("go", go, f"go-bunny-three-{suffix}", run_our_engine),
        ("rust", rust, f"rust-bunny-three-{suffix}", run_our_engine),
        ("rqbit", rqbit, f"rqbit-bunny-three-{suffix}", None),
    ]

    for kind, binary, label, runner in plan:
        subprocess.run(
            ["pkill", "-f", "engine-host|engine-go-host|/rqbit "],
            check=False,
            capture_output=True,
        )
        time.sleep(1)
        print(f"===== {kind} {label} ({seconds}s) =====", flush=True)
        if kind == "rqbit":
            row = run_rqbit(binary, label, magnet, seconds)
        else:
            row = runner(binary, label, magnet, seconds)
        with out.open("a") as f:
            f.write(json.dumps(summarize(row)) + "\n")
        rows.append(summarize(row))
        print(json.dumps(rows[-1], indent=2), flush=True)

    wall = round(time.monotonic() - t_wall, 1)
    summary = {"wall_s": wall, "stream_s": seconds, "engines": rows, "jsonl": str(out)}
    (REPORT_DIR / f"bunny-three-{suffix}-summary.json").write_text(
        json.dumps(summary, indent=2) + "\n"
    )
    print(json.dumps(summary, indent=2), flush=True)
    print(
        f"wall={wall}s  "
        + "  ".join(
            f"{r['label'].split('-')[0]} max={r['peers_min_max'][1]} avg={r['avg_mib_s']}"
            for r in rows
        ),
        flush=True,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
