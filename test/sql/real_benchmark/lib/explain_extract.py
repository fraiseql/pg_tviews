#!/usr/bin/env python3
"""Extract auto_explain JSON plans from a psql output log.

lib/explain_on.sql makes auto_explain report each plan as a client NOTICE:

    NOTICE:  duration: 0.123 ms  plan:
    {
      "Query Text": "...",
      "Plan": { ... }
    }

Every plan whose "Query Text" matches --match is written to OUT_DIR/NNN.json
(the auto_explain document plus "Duration ms"), and a one-line-per-plan
summary of the top plan node goes to OUT_DIR/summary.tsv.

Usage: explain_extract.py LOG OUT_DIR [--match REGEX]
"""

import argparse
import json
import re
from pathlib import Path

NOTICE = re.compile(r"NOTICE:\s+duration: ([0-9.]+) ms\s+plan:\s*")
SUMMARY_KEYS = (
    "Actual Total Time",
    "Actual Rows",
    "Shared Hit Blocks",
    "Shared Read Blocks",
    "Shared Dirtied Blocks",
    "Shared Written Blocks",
    "WAL Records",
    "WAL FPI",
    "WAL Bytes",
)


def iter_plans(text: str):
    decoder = json.JSONDecoder()
    for m in NOTICE.finditer(text):
        start = text.find("{", m.end())
        if start < 0:
            continue
        try:
            doc, _ = decoder.raw_decode(text, start)
        except json.JSONDecodeError:
            continue
        doc["Duration ms"] = float(m.group(1))
        yield doc


def one_line(query: str) -> str:
    return " ".join(query.split())[:160]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("log", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--match", default="", help="regex over Query Text")
    args = ap.parse_args()

    pattern = re.compile(args.match)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    rows = []
    for doc in iter_plans(args.log.read_text(errors="replace")):
        query = doc.get("Query Text", "")
        if not pattern.search(query):
            continue
        n = len(rows) + 1
        (args.out_dir / f"{n:03d}.json").write_text(json.dumps(doc, indent=2) + "\n")
        top = doc.get("Plan", {})
        rows.append(
            [f"{n:03d}", *(str(top.get(k, "")) for k in SUMMARY_KEYS), one_line(query)]
        )

    with (args.out_dir / "summary.tsv").open("w") as fh:
        fh.write("\t".join(["plan", *SUMMARY_KEYS, "query"]) + "\n")
        for row in rows:
            fh.write("\t".join(row) + "\n")
    print(f"extracted {len(rows)} plan(s) into {args.out_dir}")
    return 0 if rows else 1


if __name__ == "__main__":
    raise SystemExit(main())
