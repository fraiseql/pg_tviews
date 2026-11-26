"""Summarize time-refresh runs (run.sh): per run and save kind, latency percentiles of
the saves that overlapped the time refresh against those that did not, the lock
waits seen during it, and the full refreshes it ran.

Usage: python3 -I summarize.py OUT_DIR [LABEL ...]   (default: every run in OUT_DIR)
"""

from __future__ import annotations

import csv
import json
import sys
from pathlib import Path

KINDS = ["placement", "asset", "site", "addon"]
WARMUP = 30.0  # seconds at the start excluded from "outside"


def pct(values: list[float], q: float) -> float:
    if not values:
        return float("nan")
    values = sorted(values)
    i = min(len(values) - 1, max(0, round(q / 100 * (len(values) - 1))))
    return values[i]


def run(dir_: Path) -> dict:
    meta = json.loads((dir_ / "meta.json").read_text())
    rs, re_ = meta["refresh_start"], meta["refresh_end"]
    first = None
    lat: dict[tuple[str, bool], list[float]] = {}
    failed: dict[str, int] = {}
    for log in sorted(dir_.glob("pgb.*")):
        for line in log.read_text().splitlines():
            f = line.split()
            script, end = int(f[3]), int(f[4]) + int(f[5]) / 1e6
            if f[2] == "failed":
                failed[KINDS[script]] = failed.get(KINDS[script], 0) + 1
                continue
            ms = int(f[2]) / 1000
            start = end - ms / 1000
            first = start if first is None else min(first, start)
            inside = start < re_ and end > rs
            lat.setdefault((KINDS[script], inside), []).append(ms)
    # "outside" excludes the warm-up and the refresh window
    out: dict[str, dict] = {}
    for kind in KINDS:
        inside = lat.get((kind, True), [])
        outside = lat.get((kind, False), [])
        out[kind] = {
            "n_in": len(inside),
            "n_out": len(outside),
            **{f"in_p{q}": pct(inside, q) for q in (50, 95, 99)},
            **{f"out_p{q}": pct(outside, q) for q in (50, 95, 99)},
            "in_max": max(inside, default=float("nan")),
            "failed": failed.get(kind, 0),
        }
    waits: dict[str, int] = {}
    for row in csv.reader((dir_ / "waits.csv").open()):
        at, locktype, mode, _rel, field4, n = row
        if rs <= float(at) <= re_:
            key = f"{locktype}/{mode}" + (f"/{int(field4):#x}" if int(field4) >= 0 else "")
            waits[key] = waits.get(key, 0) + int(n)
    before = {r[0]: int(r[1]) for r in csv.reader((dir_ / "stats_before.csv").open())}
    after = {r[0]: int(r[1]) for r in csv.reader((dir_ / "stats_after.csv").open())}
    full = sum(after[e] - before.get(e, 0) for e in after)
    refresh_failed = [
        line.removeprefix("FAILED ")
        for line in (dir_ / "refresh.out").read_text().splitlines()
        if line.startswith("FAILED ")
    ]
    return {"meta": meta, "refresh_s": re_ - rs, "kinds": out, "waits": waits, "full": full,
            "refresh_failed": refresh_failed}


def main() -> None:
    root = Path(sys.argv[1])
    labels = sys.argv[2:] or sorted(p.name for p in root.iterdir() if (p / "meta.json").exists())
    print("| run | form | refresh s | save | n in/out | p50 in/out ms | p95 in/out ms | p99 in/out ms | max in ms | failed |")
    print("|---|---|---:|---|---:|---:|---:|---:|---:|---:|")
    summaries = []
    for label in labels:
        r = run(root / label)
        summaries.append(r)
        for kind, k in r["kinds"].items():
            print(
                f"| {label} | {r['meta']['form']} | {r['refresh_s']:.2f} | {kind} | "
                f"{k['n_in']}/{k['n_out']} | {k['in_p50']:.1f}/{k['out_p50']:.1f} | "
                f"{k['in_p95']:.1f}/{k['out_p95']:.1f} | {k['in_p99']:.1f}/{k['out_p99']:.1f} | "
                f"{k['in_max']:.0f} | {k['failed']} |"
            )
    print()
    print("| run | refreshes failed | full refreshes | lock waits during the refresh (waiter-samples of 100 ms) |")
    print("|---|---|---:|---|")
    for label, r in zip(labels, summaries, strict=True):
        waits = ", ".join(f"{k} {v}" for k, v in sorted(r["waits"].items(), key=lambda kv: -kv[1]))
        failed = ", ".join(r["refresh_failed"]) or "none"
        print(f"| {label} | {failed} | {r['full']} | {waits or 'none'} |")


if __name__ == "__main__":
    main()
