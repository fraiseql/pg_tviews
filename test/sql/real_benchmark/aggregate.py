#!/usr/bin/env python3
"""Aggregate raw \\timing measurements into per-(scale,arm,op) stats and a
comparison table. Reads a TSV of `scale<TAB>arm<TAB>op<TAB>ms` rows."""
import statistics
import sys
from collections import defaultdict

ARM_NAME = {"a": "pg_tviews+jsonb_delta", "b": "pg_tviews+native", "c": "full_refresh_matview"}
OP_ORDER = ["build", "update_single", "update_batch", "insert_single", "delete_single"]
SCALE_ORDER = ["small", "medium", "large"]

raw_path, summary_path = sys.argv[1], sys.argv[2]

vals = defaultdict(list)  # (scale, arm, op) -> [ms, ...]
with open(raw_path) as fh:
    for line in fh:
        parts = line.rstrip("\n").split("\t")
        if len(parts) != 4:
            continue
        scale, arm, op, ms = parts
        try:
            vals[(scale, arm, op)].append(float(ms))
        except ValueError:
            continue


def stat(key):
    xs = vals.get(key)
    if not xs:
        return None
    return {"n": len(xs), "min": min(xs), "median": statistics.median(xs), "mean": statistics.mean(xs)}


scales = [s for s in SCALE_ORDER if any(k[0] == s for k in vals)]

with open(summary_path, "w") as out:
    out.write("scale\tarm\top\tn\tmin_ms\tmedian_ms\tmean_ms\n")
    for scale in scales:
        for arm in ("a", "b", "c"):
            for op in OP_ORDER:
                s = stat((scale, arm, op))
                if s:
                    out.write(f"{scale}\t{ARM_NAME[arm]}\t{op}\t{s['n']}\t"
                              f"{s['min']:.3f}\t{s['median']:.3f}\t{s['mean']:.3f}\n")


def fmt(ms):
    if ms is None:
        return "     —"
    return f"{ms:9.3f}" if ms < 1000 else f"{ms/1000:8.3f}s"


print(f"\nPer-operation median (ms), by scale. arms: A={ARM_NAME['a']}  "
      f"B={ARM_NAME['b']}  C={ARM_NAME['c']}\n")
for scale in scales:
    print(f"── {scale} " + "─" * 48)
    print(f"  {'op':<15} {'A median':>10} {'B median':>10} {'C median':>10} "
          f"{'A vs C':>9} {'A vs B':>8}")
    for op in OP_ORDER:
        a, b, c = (stat((scale, x, op)) for x in ("a", "b", "c"))
        am = a["median"] if a else None
        bm = b["median"] if b else None
        cm = c["median"] if c else None
        avc = f"{cm/am:8.1f}x" if (am and cm and am > 0) else "       —"
        avb = f"{bm/am:6.2f}x" if (am and bm and am > 0) else "      —"
        print(f"  {op:<15} {fmt(am):>10} {fmt(bm):>10} {fmt(cm):>10} {avc:>9} {avb:>8}")
    print()

print(f"wrote {summary_path}")
