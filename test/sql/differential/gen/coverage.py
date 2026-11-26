#!/usr/bin/env python3
"""Check the generator covers the dialect (docs/reference/dialect.toml).

Each [[production]] names the generator kinds (gen.py) that create a definition
exercising it (`gen`), those whose definition it must refuse (`gen_refused`), or why
the generator does not draw it (`gen_exempt`, naming the tests that cover it
instead). Without arguments, checks that every production says one of these, that
each kind named exists and lists the production in gen.py's KIND_PRODUCTIONS, and
that every production a kind lists names the kind back.

With coverage files (the REACHED lines run.sh appends to GEN_COVERAGE), also checks
the run reached each production: some definition exercising it was created, for
`gen`, and some was refused with its SQLSTATE, for `gen_refused`.

Usage: coverage.py [GEN_COVERAGE file ...]
"""

import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import gen  # the generator next to this file


def static_problems(productions: list[dict]) -> list[str]:
    out = []
    kinds = gen.KIND_PRODUCTIONS
    named: dict[str, set[str]] = {k: set() for k in kinds}
    for p in productions:
        pid = p["id"]
        if not (p.get("gen") or p.get("gen_refused") or p.get("gen_exempt")):
            out.append(f"{pid}: no generator rule (gen, gen_refused or gen_exempt)")
        if p.get("gen_exempt") and (p.get("gen") or p.get("gen_refused")):
            out.append(f"{pid}: exempt, yet drawn by a kind")
        if p.get("gen_refused") and not p.get("sqlstate"):
            out.append(f"{pid}: gen_refused without the sqlstate it is refused with")
        for kind in p.get("gen", []) + p.get("gen_refused", []):
            if kind not in kinds:
                out.append(f"{pid}: no generator kind {kind}")
                continue
            named[kind].add(pid)
        out += [
            f"{pid}: gen.py's KIND_PRODUCTIONS[{kind!r}] does not list it"
            for kind in p.get("gen", [])
            if kind in kinds and pid not in kinds[kind]
        ]
    for kind, pids in kinds.items():
        out += [
            f"{pid}: gen.py's kind {kind} exercises it, but its gen does not name the kind"
            for pid in pids
            if pid not in named[kind]
        ]
    return out


def reached(files: list[str]) -> tuple[set[str], set[tuple[str, str]]]:
    """The productions some created definition exercised, and the (production,
    SQLSTATE) pairs some refused one did."""
    created: set[str] = set()
    refused: set[tuple[str, str]] = set()
    for f in files:
        for line in Path(f).read_text().splitlines():
            parts = line.split()
            if len(parts) < 2 or parts[0] != "REACHED":
                continue
            ids = parts[2].split(",") if len(parts) > 2 else []
            if parts[1] == "created":
                created.update(ids)
            elif parts[1].startswith("refused:"):
                state = parts[1].removeprefix("refused:")
                refused.update((i, state) for i in ids)
    return created, refused


def main() -> None:
    productions = tomllib.loads(gen.DIALECT.read_text())["production"]
    wrong = static_problems(productions)
    if sys.argv[1:]:
        created, refused = reached(sys.argv[1:])
        for p in productions:
            if p.get("gen") and p["id"] not in created:
                wrong.append(f"{p['id']}: no definition exercising it was created")
            if p.get("gen_refused") and (p["id"], p.get("sqlstate")) not in refused:
                wrong.append(
                    f"{p['id']}: no definition exercising it was refused with {p.get('sqlstate')}"
                )
    if wrong:
        sys.exit("generator coverage of dialect.toml:\n  " + "\n  ".join(wrong))
    drawn = sum(1 for p in productions if not p.get("gen_exempt"))
    print(f"coverage: {drawn} of {len(productions)} productions drawn by the generator")


if __name__ == "__main__":
    main()
