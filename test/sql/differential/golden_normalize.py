"""Normalize a stored TVIEW plan for golden.sh: relation OIDs and attribute numbers
become `schema.table` and column names, so the file is the same on every database.

Usage: python3 golden_normalize.py <relations.json> < plan.json > plan.normalized.json

relations.json maps each relation OID (as a string) to
{"name": "schema.table", "cols": {"<attnum>": "column"}}.
"""

import json
import re
import sys

TEMPLATE = re.compile(r"\{c:(\d+):(-?\d+)\}|\{r:(\d+)\}")
# Fields holding attribute numbers of the relation named by the enclosing
# object's `relid` (a key mapping) or `source_oid` (a local path).
ATTNUM_LISTS = ("attnums", "links")
ATTNUM_SCALARS = ("initial_attnum",)


def main() -> None:
    with open(sys.argv[1], encoding="utf-8") as f:
        rels = json.load(f)

    def rel(oid: int | str) -> str:
        return rels.get(str(oid), {}).get("name", f"<relation {oid}>")

    def col(oid: int | str, attnum: int | str) -> str:
        if str(attnum) == "0":
            return "<whole row>"
        cols = rels.get(str(oid), {}).get("cols", {})
        return f"{rel(oid)}.{cols.get(str(attnum), f'<attnum {attnum}>')}"

    def template(text: str) -> str:
        def sub(m: re.Match[str]) -> str:
            if m.group(3):
                return "{r:" + rel(m.group(3)) + "}"
            return "{c:" + col(m.group(1), m.group(2)) + "}"

        return TEMPLATE.sub(sub, text)

    def walk(node, owner=None):  # noqa: ANN001, ANN202
        if isinstance(node, dict):
            here = node.get("relid", node.get("source_oid", owner))
            out = {}
            for key, value in node.items():
                if key in ("relid", "source_oid"):
                    out[key] = rel(value)
                elif key in ATTNUM_LISTS and isinstance(value, list):
                    out[key] = [col(here, a) for a in value]
                elif (key in ATTNUM_SCALARS or (key == "attnum" and owner)) and isinstance(
                    value, int
                ):
                    out[key] = col(here, value)
                else:
                    out[key] = walk(value, here)
            return out
        if isinstance(node, list):
            return [walk(v, owner) for v in node]
        if isinstance(node, str):
            return template(node)
        return node

    plan = json.load(sys.stdin)
    json.dump(walk(plan), sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
