#!/usr/bin/env python3
"""Measure successful indexed names over HTTP, separately from live discovery."""

import argparse
import concurrent.futures
import csv
import json
import random
import time
import urllib.request
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--csv", type=Path, default=Path.home() / ".cache/torrent-player/torrents.csv")
    parser.add_argument("--url", default="http://127.0.0.1:18181")
    parser.add_argument("--count", type=int, default=100_000)
    parser.add_argument("--workers", type=int, choices=(1, 2), default=2)
    args = parser.parse_args()
    if not 1 <= args.count <= 100_000:
        parser.error("count must be between 1 and 100,000")

    # Reservoir sampling spreads distinct real hashes throughout the dataset.
    rng = random.Random(18181)
    sample = []
    with args.csv.open(newline="", encoding="utf-8") as source:
        for index, row in enumerate(csv.DictReader(source)):
            if index < args.count:
                sample.append(row["infohash"].lower())
            else:
                position = rng.randrange(index + 1)
                if position < args.count:
                    sample[position] = row["infohash"].lower()
    if len(set(sample)) != args.count:
        raise RuntimeError("dataset did not supply the requested distinct hashes")
    rng.shuffle(sample)

    def lookup(hashes):
        request = urllib.request.Request(
            args.url.rstrip("/") + "/api/resolve",
            data=json.dumps({"hashes": hashes}).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=30) as response:
            batch = json.load(response)
        if batch["unique"] != len(hashes) or batch["invalid"] or batch["duplicates"]:
            raise RuntimeError("batch accounting mismatch")
        results = batch["results"]
        if any(not row["name"] for row in results):
            raise RuntimeError("empty label counted as a name")
        if set(row["hash"] for row in results) | set(batch["missing"]) != set(hashes):
            raise RuntimeError("response did not account for every requested hash")
        return len(results), sum(row["verified"] for row in results)

    started = time.perf_counter()
    batches = [sample[start:start + 10_000] for start in range(0, len(sample), 10_000)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        counts = list(pool.map(lookup, batches))
    elapsed = time.perf_counter() - started
    matched = sum(count for count, _ in counts)
    print(json.dumps({
        "kind": "reused indexed names over HTTP; not fresh network discovery",
        "uniqueHashes": len(sample), "matchedNames": matched,
        "unmatchedHashes": len(sample) - matched,
        "withVerifiedMetadata": sum(verified for _, verified in counts),
        "elapsedSeconds": round(elapsed, 4),
        "matchedNamesPerMinute": round(matched * 60 / elapsed),
        "targetPerMinute": 100_000,
        "targetMet": matched * 60 / elapsed >= 100_000,
    }, indent=2))


if __name__ == "__main__":
    main()
