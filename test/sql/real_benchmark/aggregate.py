#!/usr/bin/env python3
"""Aggregate real-benchmark results.

aggregate.py timing RAW_TSV SUMMARY_TSV
    Median \\timing per (scale, mode, arm, op) from run.sh's raw.tsv
    (`scale<TAB>arm<TAB>mode<TAB>op<TAB>ms`; legacy 4-column rows without
    mode are read as the shipped default, unlogged).

aggregate.py physical RUN_DIR [-o REPORT_MD]
    Markdown report of a physical run directory (env.tsv, physical.csv,
    fanout.csv, explain/*/summary.tsv).
"""

import argparse
import csv
import statistics
from collections import defaultdict
from pathlib import Path

ARM_NAME = {
    "a": "pg_tviews+jsonb_delta",
    "b": "pg_tviews+native",
    "c": "full_refresh_matview",
}
OP_ORDER = ["build", "update_single", "update_batch", "insert_single", "delete_single"]
SCALE_ORDER = ["small", "medium", "large"]
MODE_ORDER = ["unlogged", "logged"]
MIB = 1024 * 1024


# --------------------------------------------------------------------- timing


def read_raw(path: Path) -> dict[tuple[str, str, str, str], list[float]]:
    vals: dict[tuple[str, str, str, str], list[float]] = defaultdict(list)
    for line in path.read_text().splitlines():
        parts = line.split("\t")
        if len(parts) == 4:
            parts.insert(2, "unlogged")
        if len(parts) != 5:
            continue
        scale, arm, mode, op, ms = parts
        try:
            vals[(scale, arm, mode, op)].append(float(ms))
        except ValueError:
            continue
    return vals


def fmt_ms(ms: float | None) -> str:
    if ms is None:
        return "     —"
    return f"{ms:9.3f}" if ms < 1000 else f"{ms / 1000:8.3f}s"


def timing(raw_path: Path, summary_path: Path) -> None:
    vals = read_raw(raw_path)

    def stat(key):
        xs = vals.get(key)
        if not xs:
            return None
        return {
            "n": len(xs),
            "min": min(xs),
            "median": statistics.median(xs),
            "mean": statistics.mean(xs),
        }

    def median(scale, arm, mode, op):
        if arm == "c":  # a matview has no TVIEW persistence mode
            keys = [k for k in vals if k[:2] == (scale, "c") and k[3] == op]
            s = stat(keys[0]) if keys else None
        else:
            s = stat((scale, arm, mode, op))
        return s["median"] if s else None

    scales = [s for s in SCALE_ORDER if any(k[0] == s for k in vals)]
    modes = [m for m in MODE_ORDER if any(k[2] == m and k[1] != "c" for k in vals)]

    with summary_path.open("w") as out:
        out.write("scale\tarm\tmode\top\tn\tmin_ms\tmedian_ms\tmean_ms\n")
        for key in sorted(vals):
            scale, arm, mode, op = key
            s = stat(key)
            out.write(
                f"{scale}\t{ARM_NAME[arm]}\t{mode}\t{op}\t{s['n']}\t"
                f"{s['min']:.3f}\t{s['median']:.3f}\t{s['mean']:.3f}\n"
            )

    print(
        f"\nPer-operation median (ms), by scale and TVIEW mode. arms: "
        f"A={ARM_NAME['a']}  B={ARM_NAME['b']}  C={ARM_NAME['c']}\n"
    )
    for scale in scales:
        for mode in modes:
            print(f"── {scale} / {mode} " + "─" * 40)
            print(
                f"  {'op':<15} {'A median':>10} {'B median':>10} {'C median':>10} "
                f"{'A vs C':>9} {'A vs B':>8}"
            )
            for op in OP_ORDER:
                am, bm, cm = (median(scale, x, mode, op) for x in ("a", "b", "c"))
                avc = f"{cm / am:8.1f}x" if (am and cm) else "       —"
                avb = f"{bm / am:6.2f}x" if (am and bm) else "      —"
                print(
                    f"  {op:<15} {fmt_ms(am):>10} {fmt_ms(bm):>10} {fmt_ms(cm):>10} "
                    f"{avc:>9} {avb:>8}"
                )
            print()
    print(f"wrote {summary_path}")


# ------------------------------------------------------------------- physical


def num(v: str) -> float:
    try:
        return float(v)
    except (TypeError, ValueError):
        return 0.0


def mib(v: float) -> str:
    return f"{v / MIB:.2f}"


def signed_mib(v: float) -> str:
    return f"{v / MIB:+.2f}"


def table(header: list[str], rows: list[list[str]]) -> list[str]:
    out = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    out += ["| " + " | ".join(r) + " |" for r in rows]
    return out + [""]


def env_section(run_dir: Path) -> list[str]:
    path = run_dir / "env.tsv"
    if not path.exists():
        return []
    rows = [
        line.split("\t", 1) for line in path.read_text().splitlines() if "\t" in line
    ]
    return [
        "## Environment",
        "",
        *table(["key", "value"], [[k, f"`{v}`"] for k, v in rows]),
    ]


def fanout_section(run_dir: Path) -> list[str]:
    path = run_dir / "fanout.csv"
    if not path.exists():
        return []
    with path.open() as fh:
        rows = list(csv.DictReader(fh))
    cols = ["scenario", "edge", "parents", "p50", "p95", "p99", "max", "mean"]
    return [
        "## Cascade fan-out (dependents per parent key)",
        "",
        *table(cols, [[r[c] for c in cols] for r in rows]),
    ]


def step_rows(rows: list[dict]) -> list[list[str]]:
    seen: dict[tuple[str, str], dict] = {}
    for r in rows:
        seen.setdefault((r["mode"], r["step"]), r)
    out = []
    for (mode, step), r in seen.items():
        out.append(
            [
                mode,
                step,
                r["ops"] or "—",
                r["elapsed_ms"],
                r["wal_records"],
                r["wal_fpi"],
                mib(num(r["wal_bytes"])),
                r["wal_bytes_per_op"] or "—",
            ]
        )
    return out


def rel_rows(rows: list[dict]) -> list[list[str]]:
    out = []
    for r in rows:
        if not r["relname"].startswith(("tv_", "mv_")):
            continue
        heap_d = num(r["heap_bytes_after"]) - num(r["heap_bytes_before"])
        idx_d = num(r["index_bytes_after"]) - num(r["index_bytes_before"])
        toast_d = num(r["toast_bytes_after"]) - num(r["toast_bytes_before"])
        out.append(
            [
                r["mode"],
                r["step"],
                f"{r['relname']} ({r['relpersistence']})",
                r["n_tup_upd"],
                r["hot_pct"] or "—",
                r["n_tup_newpage_upd"],
                r["n_tup_ins"],
                r["n_tup_del"],
                r["n_dead_tup_after"],
                r["seq_scan"],
                r["idx_scan"],
                f"{mib(num(r['heap_bytes_after']))} ({signed_mib(heap_d)})",
                f"{mib(num(r['index_bytes_after']))} ({signed_mib(idx_d)})",
                f"{mib(num(r['toast_bytes_after']))} ({signed_mib(toast_d)})",
                r["all_visible_frac_after"] or "—",
                r["bytes_per_row"] or "—",
            ]
        )
    return out


def explain_rows(run_dir: Path, scenario: str) -> list[list[str]]:
    """Plans grouped by statement text, per mode, for one scenario."""
    out = []
    for summary in sorted((run_dir / "explain").glob(f"{scenario}_*/summary.tsv")):
        mode = summary.parent.name.removeprefix(f"{scenario}_")
        if mode not in MODE_ORDER:
            continue
        groups: dict[str, list[dict]] = defaultdict(list)
        with summary.open() as fh:
            for r in csv.DictReader(fh, delimiter="\t"):
                groups[r["query"][:110]].append(r)
        for query, rs in groups.items():

            def total(key, rs=rs):
                return sum(num(r[key]) for r in rs)

            out.append(
                [
                    mode,
                    f"`{query}`",
                    str(len(rs)),
                    f"{total('Actual Total Time'):.2f}",
                    f"{total('Actual Rows'):.0f}",
                    f"{total('Shared Hit Blocks'):.0f}/{total('Shared Read Blocks'):.0f}",
                    (
                        f"{total('Shared Dirtied Blocks'):.0f}/"
                        f"{total('Shared Written Blocks'):.0f}"
                    ),
                    f"{total('WAL Records'):.0f}",
                    f"{total('WAL FPI'):.0f}",
                    f"{total('WAL Bytes'):.0f}",
                ]
            )
    return out


def physical(run_dir: Path, report: Path | None) -> None:
    with (run_dir / "physical.csv").open() as fh:
        rows = list(csv.DictReader(fh))
    by_scenario: dict[str, list[dict]] = defaultdict(list)
    for r in rows:
        by_scenario[r["scenario"]].append(r)

    lines = [f"# Physical benchmark run `{run_dir.name}`", ""]
    lines += env_section(run_dir) + fanout_section(run_dir)
    for scenario, rs in by_scenario.items():
        lines += [
            f"## `{scenario}`",
            "",
            "Per step (WAL is cluster-wide for the step):",
            "",
        ]
        lines += table(
            ["mode", "step", "ops", "ms", "WAL rec", "FPI", "WAL MiB", "WAL B/op"],
            step_rows(rs),
        )
        lines += ["Per relation (sizes in MiB after the step, Δ in parentheses):", ""]
        lines += table(
            [
                "mode",
                "step",
                "relation",
                "upd",
                "HOT %",
                "newpage upd",
                "ins",
                "del",
                "dead after",
                "seq scan",
                "idx scan",
                "heap",
                "index",
                "TOAST",
                "all-visible",
                "B/row",
            ],
            rel_rows(rs),
        )
        plans = explain_rows(run_dir, scenario)
        if plans:
            lines += [
                "Flush statements of one representative refresh (auto_explain):",
                "",
            ]
            lines += table(
                [
                    "mode",
                    "statement",
                    "calls",
                    "ms",
                    "rows",
                    "hit/read",
                    "dirtied/written",
                    "WAL rec",
                    "FPI",
                    "WAL B",
                ],
                plans,
            )

    text = "\n".join(lines)
    if report:
        report.write_text(text)
        print(f"wrote {report}")
    else:
        print(text)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("timing")
    t.add_argument("raw", type=Path)
    t.add_argument("summary", type=Path)
    p = sub.add_parser("physical")
    p.add_argument("run_dir", type=Path)
    p.add_argument("-o", "--output", type=Path)
    args = ap.parse_args()
    if args.cmd == "timing":
        timing(args.raw, args.summary)
    else:
        physical(args.run_dir, args.output)


if __name__ == "__main__":
    main()
