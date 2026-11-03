"""Seeded view-shape generator for the differential oracle (run.sh in this folder).

From a seed, writes cases: each case is a random schema of 2-4 base tables in the
project's convention (tb_<entity>, pk_<entity> integer primary key, id uuid, value
columns, fk_<entity> references), 3-6 TVIEW definitions drawn from the grammar
below, and random writes, each followed by a check that every TVIEW equals its view.
The same seed writes the same cases.

Usage: python3 -I gen.py <seed> <shapes> <statements> <out_dir> [features]

Writes <out_dir>/case<N>.sql until <shapes> TVIEW definitions are written, and
prints one line per case: its file name and its number of shapes. `features`
is a comma-separated list of the opt-in writes of OPT_IN.

The grammar (one TVIEW per draw, every one selecting pk_<entity>, id and data):
  plain     one table, jsonb_build_object of its columns
  join      inner and LEFT joins up the fk_ chain (depth <= 3), parent columns
            copied flat or as a nested object
  embed     another TVIEW's data, joined on its key (TVIEW over TVIEW)
  subagg    scalar subquery aggregates over a child table (count, sum, max)
  array     jsonb_agg of the children in a correlated subquery, ordered
  union     UNION ALL of two tables, the second keyed by its negated pk
  cte       a CTE over the root, joined to its parent
  group     GROUP BY the child's fk_, keyed by the group (group_keys or not)
  last      DISTINCT ON the child's fk_: one child per parent
  top       DISTINCT ON the root's pk over a LEFT JOIN of its children
Any of them may filter soft-deleted rows (deleted_at IS NULL).
"""

import json
import random
import sys
from dataclasses import dataclass, field
from pathlib import Path

ENTITIES = [
    "org",
    "team",
    "member",
    "project",
    "task",
    "post",
    "comment",
    "label",
    "invoice",
    "line",
    "shop",
    "item",
]
COLUMN_NAMES = {
    "text": ["name", "code"],
    "int": ["qty", "rank"],
    "bool": ["active", "pinned"],
    "numeric": ["amount", "price"],
    "timestamptz": ["seen_at", "due_at"],
    "jsonb": ["meta", "attrs"],
}
ORDERABLE = ("text", "int", "numeric", "timestamptz", "bool")
MAXABLE = ("text", "int", "numeric", "timestamptz")
SUMMABLE = ("int", "numeric")
# Writes left out unless named in the optional fifth argument, each for an open
# defect: the run stays green while it is open, and naming it reproduces it.
# None is open: #288 (a referenced pk_<entity> changed) and #289 (a LEFT JOIN
# filtering its parent) were found this way and are in the default run.
OPT_IN: set[str] = set()
# the share of the writes made by a mutation function, and in a transaction
MUTATION_SHARE = 0.2
TRANSACTION_SHARE = 0.1
# draws of a kind before giving up on a case's shape count (a kind may not apply)
MAX_DRAWS = 200
# TVIEW definitions per case
MIN_CASE, MAX_CASE = 3, 6
KINDS = [
    "plain",
    "join",
    "embed",
    "subagg",
    "array",
    "union",
    "cte",
    "group",
    "last",
    "top",
]


def lit(s: str) -> str:
    return "'" + s.replace("'", "''") + "'"


@dataclass
class Fk:
    column: str
    parent: "Table"
    # None (no constraint: rows may be orphaned), "CASCADE" or "SET NULL"
    on_delete: str | None


@dataclass
class Table:
    entity: str
    pk_type: str
    cols: list[tuple[str, str]]
    soft: bool
    fks: list[Fk] = field(default_factory=list)
    rows: int = 0

    @property
    def name(self) -> str:
        return f"tb_{self.entity}"

    @property
    def pk(self) -> str:
        return f"pk_{self.entity}"

    def cols_of(self, types: tuple[str, ...]) -> list[str]:
        return [c for c, t in self.cols if t in types]


@dataclass
class Shape:
    tv: str
    key: str
    sql: str
    options: dict
    needs: list[str]
    # the table whose pk keys one row of this TVIEW (an embeddable shape), or None
    keyed_by: Table | None
    aggregate: bool = False


class Case:
    def __init__(
        self, rng: random.Random, n_shapes: int, n_statements: int, features: set[str]
    ) -> None:
        self.rng = rng
        self.features = features
        self.n_shapes = n_shapes
        self.n_statements = n_statements
        self.tables: list[Table] = []
        self.shapes: list[Shape] = []
        self.out: list[str] = []
        self.functions = 0

    # ── schema ──────────────────────────────────────────────────────────────
    def make_tables(self) -> None:
        r = self.rng
        entities = r.sample(ENTITIES, r.randint(2, 4))
        for i, e in enumerate(entities):
            types = r.sample(list(COLUMN_NAMES), r.randint(2, 4))
            if "text" not in types:
                types[0] = "text"
            cols = [(COLUMN_NAMES[t][0], t) for t in types]
            t = Table(e, r.choice(["int", "bigint"]), cols, soft=self.chance(0.5))
            if i > 0:
                parents = r.sample(
                    self.tables, min(len(self.tables), r.choice([1, 1, 2]))
                )
                for p in parents:
                    t.fks.append(
                        Fk(
                            f"fk_{p.entity}",
                            p,
                            r.choice([None, None, "CASCADE", "SET NULL"]),
                        )
                    )
            self.tables.append(t)

    def chance(self, p: float) -> bool:
        return self.rng.random() < p

    def children(self, t: Table) -> list[tuple[Table, Fk]]:
        return [(c, fk) for c in self.tables for fk in c.fks if fk.parent is t]

    def value(self, typ: str, i: int) -> str:
        """A literal of `typ` for write #i, NULL now and then."""
        r = self.rng
        if self.chance(0.08):
            return "NULL"
        n = r.randint(0, 99)
        return {
            "text": lambda: lit(r.choice(["a", "b", "c", "O'k", "é"]) + f"{i}_{n}"),
            "int": lambda: str(n % 44 - 3),
            "bool": lambda: r.choice(["true", "false"]),
            "numeric": lambda: f"{r.randint(0, 99999) / 100:.2f}",
            # written in three zones; rendered in UTC by refreshes and fresh_diff
            "timestamptz": lambda: lit(
                f"2026-{r.randint(1, 12):02}-{r.randint(1, 28):02} "
                f"{r.randint(0, 23):02}:{r.choice(['00', '30'])}:00"
                f"{r.choice(['+00', '+02', '-05'])}"
            ),
            "jsonb": lambda: lit(f'{{"k": {n % 10}, "tags": ["t{n % 10}"]}}')
            + "::jsonb",
        }[typ]()

    def write_schema(self) -> None:
        r = self.rng
        self.out.append(
            "-- No foreign key on some fk_ columns: the writes may orphan rows."
        )
        for t in self.tables:
            lines = [
                f"{t.pk} {t.pk_type} GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY",
                "id uuid NOT NULL DEFAULT gen_random_uuid()",
            ]
            lines += [f"{c} {typ}" for c, typ in t.cols]
            if t.soft:
                lines.append("deleted_at timestamptz")
            for fk in t.fks:
                ref = ""
                if fk.on_delete:
                    ref = f" REFERENCES {fk.parent.name} ON DELETE {fk.on_delete}"
                lines.append(f"{fk.column} {fk.parent.pk_type}{ref}")
            self.out.append(f"CREATE TABLE {t.name} (\n  " + ",\n  ".join(lines) + ");")
            for fk in t.fks:
                self.out.append(f"CREATE INDEX ON {t.name} ({fk.column});")
        for t in self.tables:
            t.rows = r.randint(4, 7) if not t.fks else r.randint(5, 10)
            values = []
            for i in range(1, t.rows + 1):
                v = [self.value(typ, i) for _, typ in t.cols]
                if t.soft:
                    v.append(
                        "NULL" if self.chance(0.75) else lit("2026-01-01 00:00:00+00")
                    )
                for fk in t.fks:
                    if self.chance(0.1):
                        v.append("NULL")
                    elif fk.on_delete is None and self.chance(0.05):
                        v.append("999")
                    else:
                        v.append(str(r.randint(1, fk.parent.rows)))
                values.append("(" + ", ".join(v) + ")")
            cols = (
                [c for c, _ in t.cols]
                + (["deleted_at"] if t.soft else [])
                + [fk.column for fk in t.fks]
            )
            self.out.append(
                f"INSERT INTO {t.name} ({', '.join(cols)}) VALUES\n  "
                + ",\n  ".join(values)
                + ";"
            )

    # ── TVIEW grammar ───────────────────────────────────────────────────────
    def tv_name(self, base: str) -> str:
        taken = {s.tv for s in self.shapes}
        name, n = f"tv_{base}", 2
        while name in taken:
            name, n = f"tv_{base}{n}", n + 1
        return name

    def obj(self, alias: str, t: Table, at_least: int = 1) -> str:
        cols = self.rng.sample(t.cols, self.rng.randint(at_least, len(t.cols)))
        return ", ".join(f"{lit(c)}, {alias}.{c}" for c, _ in cols)

    def soft(self, alias: str, t: Table) -> str | None:
        if t.soft and self.chance(0.6):
            return f"{alias}.deleted_at IS NULL"
        return None

    @staticmethod
    def where(conds: list[str | None]) -> str:
        cs = [c for c in conds if c]
        return f"\n  WHERE {' AND '.join(cs)}" if cs else ""

    def extra_cols(self, alias: str, t: Table) -> str:
        """Projected fk_ / filter columns, sometimes."""
        if t.fks and self.chance(0.4):
            return "".join(f", {alias}.{fk.column}" for fk in t.fks)
        return ""

    def shape(self, kind: str) -> Shape | None:
        return getattr(self, f"shape_{kind}")()

    def shape_plain(self) -> Shape:
        t = self.rng.choice(self.tables)
        tv = self.tv_name(t.entity)
        key = "pk_" + tv[3:]
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.extra_cols('t', t)}, "
            f"jsonb_build_object({self.obj('t', t)}) AS data\n  FROM {t.name} t"
            + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_join(self) -> Shape | None:
        r = self.rng
        roots = [t for t in self.tables if t.fks]
        if not roots:
            return None
        t = r.choice(roots)
        tv = self.tv_name(f"{t.entity}_j")
        key = "pk_" + tv[3:]
        joins, parts, conds = [], [self.obj("t", t)], [self.soft("t", t)]
        # walk up the fk_ chain, sometimes branching to a second fk_ of the root
        frontier = [("t", t)]
        depth = r.randint(1, 3)
        n = 0
        while n < depth and frontier:
            alias, cur = r.choice(frontier)
            free = [
                fk
                for fk in cur.fks
                if not any(j[1] == (alias, fk.column) for j in joins)
            ]
            if not free:
                frontier.remove((alias, cur))
                continue
            fk = r.choice(free)
            n += 1
            pa = f"p{n}"
            left = self.chance(0.5)
            on = f"{pa}.{fk.parent.pk} = {alias}.{fk.column}"
            s = self.soft(pa, fk.parent)
            if s and left:
                on += f" AND {s}"
            elif s:
                conds.append(s)
            joins.append(
                (
                    f"{'LEFT ' if left else ''}JOIN {fk.parent.name} {pa} ON {on}",
                    (alias, fk.column),
                )
            )
            if self.chance(0.5):
                nested = self.obj(pa, fk.parent)
                parts.append(f"{lit(fk.parent.entity)}, jsonb_build_object({nested})")
            else:
                c = r.choice(fk.parent.cols)[0]
                parts.append(f"{lit(fk.parent.entity + '_' + c)}, {pa}.{c}")
            frontier.append((pa, fk.parent))
        extra = self.extra_cols("t", t)
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{extra}, "
            f"jsonb_build_object({', '.join(parts)}) AS data\n  FROM {t.name} t\n  "
            + "\n  ".join(j[0] for j in joins)
            + self.where(conds)
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_embed(self) -> Shape | None:
        r = self.rng
        options = [
            (t, fk, s)
            for t in self.tables
            for fk in t.fks
            for s in self.shapes
            if s.keyed_by is fk.parent
        ]
        if not options:
            return None
        t, fk, s = r.choice(options)
        tv = self.tv_name(f"{t.entity}_e")
        key = "pk_" + tv[3:]
        left = self.chance(0.6)
        extra = self.extra_cols("t", t)
        if s.aggregate and f"t.{fk.column}" not in extra:
            # an embedded aggregate is routed through the projected join column
            # (refused with 0A000 otherwise, docs/user-guides/aggregate-tviews.md)
            extra += f", t.{fk.column}"
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{extra}, "
            f"jsonb_build_object({self.obj('t', t)}, {lit(fk.parent.entity)}, v.data)"
            " AS data\n"
            f"  FROM {t.name} t\n  {'LEFT ' if left else ''}JOIN {s.tv} v "
            f"ON v.{s.key} = t.{fk.column}" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [s.tv], t)

    def child_subquery(self, t: Table, c: Table, fk: Fk, select: str) -> str:
        cond = [f"c.{fk.column} = t.{t.pk}", self.soft("c", c)]
        return (
            f"(SELECT {select} FROM {c.name} c WHERE "
            + " AND ".join(x for x in cond if x)
            + ")"
        )

    def shape_subagg(self) -> Shape | None:
        r = self.rng
        parents = [t for t in self.tables if self.children(t)]
        if not parents:
            return None
        t = r.choice(parents)
        c, fk = r.choice(self.children(t))
        tv = self.tv_name(f"{t.entity}_s")
        key = "pk_" + tv[3:]
        parts = [
            self.obj("t", t),
            f"{lit('n_' + c.entity)}, " + self.child_subquery(t, c, fk, "count(*)"),
        ]
        parts += [
            f"{lit('sum_' + col)}, " + self.child_subquery(t, c, fk, f"sum(c.{col})")
            for col in c.cols_of(SUMMABLE)[:1]
        ]
        if self.chance(0.5):
            col = r.choice(c.cols_of(MAXABLE))
            parts.append(
                f"{lit('max_' + col)}, "
                + self.child_subquery(t, c, fk, f"max(c.{col})")
            )
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id, "
            f"jsonb_build_object({', '.join(parts)}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_array(self) -> Shape | None:
        r = self.rng
        parents = [t for t in self.tables if self.children(t)]
        if not parents:
            return None
        t = r.choice(parents)
        c, fk = r.choice(self.children(t))
        tv = self.tv_name(f"{t.entity}_a")
        key = "pk_" + tv[3:]
        elem = f"'id', c.id, {self.obj('c', c)}"
        grand = self.children(c)
        if grand and self.chance(0.5):
            # each element counts its own children: a correlation two levels down
            g, gfk = r.choice(grand)
            elem += (
                f", {lit('n_' + g.entity)}, (SELECT count(*) FROM {g.name} g "
                f"WHERE g.{gfk.column} = c.{c.pk})"
            )
        agg = (
            f"coalesce(jsonb_agg(jsonb_build_object({elem}) ORDER BY c.{c.pk}), "
            "'[]'::jsonb)"
        )
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id, jsonb_build_object({self.obj('t', t)}, "
            f"{lit(c.entity + 's')}, {self.child_subquery(t, c, fk, agg)}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_union(self) -> Shape:
        r = self.rng
        a, b = r.sample(self.tables, 2)
        tv = self.tv_name(f"{a.entity}_{b.entity}_u")
        key = "pk_" + tv[3:]
        sql = (
            f"SELECT a.{a.pk} AS {key}, a.id, "
            f"jsonb_build_object('kind', {lit(a.entity)}, {self.obj('a', a)}) AS data\n"
            f"  FROM {a.name} a"
            + self.where([self.soft("a", a)])
            + f"\nUNION ALL\nSELECT -b.{b.pk}, b.id, "
            f"jsonb_build_object('kind', {lit(b.entity)}, {self.obj('b', b)}) AS data\n"
            f"  FROM {b.name} b" + self.where([self.soft("b", b)])
        )
        return Shape(tv, key, sql, {}, [], None)

    def shape_cte(self) -> Shape:
        r = self.rng
        t = r.choice(self.tables)
        tv = self.tv_name(f"{t.entity}_c")
        key = "pk_" + tv[3:]
        inner = ", ".join(c for c, _ in t.cols)
        fk = r.choice(t.fks) if t.fks else None
        where = self.where([self.soft("t", t)]).replace("\n  ", " ")
        sql = (
            f"WITH x AS (SELECT t.{t.pk}, t.id, {inner}"
            + (f", t.{fk.column}" if fk else "")
            + f" FROM {t.name} t{where})\n"
            f"SELECT x.{t.pk} AS {key}, x.id, jsonb_build_object({self.obj('x', t)}"
        )
        if fk:
            c = r.choice(fk.parent.cols)[0]
            sql += (
                f", {lit(fk.parent.entity + '_' + c)}, p.{c}) AS data\n  FROM x\n"
                f"  LEFT JOIN {fk.parent.name} p ON p.{fk.parent.pk} = x.{fk.column}"
            )
        else:
            sql += ") AS data\n  FROM x"
        return Shape(tv, key, sql, {}, [], t)

    def child_pairs(self) -> list[tuple[Table, Table, Fk]]:
        return [(t, c, fk) for t in self.tables for c, fk in self.children(t)]

    def shape_group(self) -> Shape | None:
        r = self.rng
        pairs = self.child_pairs()
        if not pairs:
            return None
        p, c, fk = r.choice(pairs)
        tv = self.tv_name(f"{c.entity}_by_{p.entity}")
        key = "pk_" + tv[3:]
        pcol = r.choice(p.cols)[0]
        parts = [f"{lit(pcol)}, p.{pcol}", f"{lit('n')}, count(*)"]
        parts += [
            f"{lit('sum_' + col)}, sum(c.{col})" for col in c.cols_of(SUMMABLE)[:1]
        ]
        if self.chance(0.5):
            col = r.choice(c.cols_of(MAXABLE))
            parts.append(f"{lit('max_' + col)}, max(c.{col})")
        sql = (
            f"SELECT c.{fk.column} AS {key}, p.id, "
            f"jsonb_build_object({', '.join(parts)}) AS data\n"
            f"  FROM {c.name} c JOIN {p.name} p ON p.{p.pk} = c.{fk.column}"
            + self.where([self.soft("c", c), self.soft("p", p)])
            + f"\n  GROUP BY c.{fk.column}, p.id, p.{pcol}"
        )
        options = {}
        if self.chance(0.5):
            options = {"group_keys": {c.name: fk.column, p.name: p.pk}}
        return Shape(tv, key, sql, options, [], p, aggregate=True)

    def shape_last(self) -> Shape | None:
        r = self.rng
        pairs = self.child_pairs()
        if not pairs:
            return None
        p, c, fk = r.choice(pairs)
        tv = self.tv_name(f"{p.entity}_last_{c.entity}")
        key = "pk_" + tv[3:]
        order = r.choice(c.cols_of(ORDERABLE))
        sql = (
            f"SELECT DISTINCT ON (c.{fk.column}) c.{fk.column} AS {key}, p.id, "
            f"jsonb_build_object({self.obj('c', c)}, {lit(p.entity)}, p.{p.cols[0][0]})"
            " AS data\n"
            f"  FROM {c.name} c JOIN {p.name} p ON p.{p.pk} = c.{fk.column}"
            + self.where([self.soft("c", c)])
            + f"\n  ORDER BY c.{fk.column}, c.{order} DESC NULLS LAST, c.{c.pk} DESC"
        )
        return Shape(tv, key, sql, {}, [], p)

    def shape_top(self) -> Shape | None:
        r = self.rng
        pairs = self.child_pairs()
        if not pairs:
            return None
        p, c, fk = r.choice(pairs)
        tv = self.tv_name(f"{p.entity}_top_{c.entity}")
        key = "pk_" + tv[3:]
        order = r.choice(c.cols_of(ORDERABLE))
        on = f"c.{fk.column} = p.{p.pk}"
        s = self.soft("c", c)
        if s:
            on += f" AND {s}"
        col = r.choice(c.cols)[0]
        sql = (
            f"SELECT DISTINCT ON (p.{p.pk}) p.{p.pk} AS {key}, p.id, "
            f"jsonb_build_object({self.obj('p', p)}, {lit('top_' + col)}, c.{col})"
            " AS data\n"
            f"  FROM {p.name} p LEFT JOIN {c.name} c ON {on}"
            + self.where([self.soft("p", p)])
            + f"\n  ORDER BY p.{p.pk}, c.{order} DESC NULLS LAST, c.{c.pk} DESC"
        )
        return Shape(tv, key, sql, {}, [], p)

    def make_shapes(self) -> None:
        r = self.rng
        tries = 0
        while len(self.shapes) < self.n_shapes and tries < MAX_DRAWS:
            tries += 1
            s = self.shape(r.choice(KINDS))
            if s:
                self.shapes.append(s)

    def write_shapes(self) -> None:
        for s in self.shapes:
            options = json.dumps(s.options, sort_keys=True)
            needs = "ARRAY[" + ", ".join(lit(n) for n in s.needs) + "]::text[]"
            self.out.append(
                f"SELECT gen_create({lit(s.tv)}, {lit(s.key)}, $def$\n{s.sql}\n$def$,\n"
                f"  {lit(options)}, {needs});"
            )

    # ── writes ──────────────────────────────────────────────────────────────
    def pick(self, t: Table, n: int) -> str:
        return (
            f"(SELECT {t.pk} FROM {t.name} ORDER BY {t.pk} "
            f"OFFSET {self.rng.randint(0, 9)} LIMIT {n})"
        )

    def n(self) -> int:
        return 1 if self.chance(0.55) else self.rng.randint(2, 4)

    def fk_value(self, fk: Fk) -> str:
        if self.chance(0.08):
            return "NULL"
        if fk.on_delete is None and self.chance(0.06):
            return "999"
        return self.pick(fk.parent, 1)

    def update(self, t: Table, sets: list[str], n: int | None = None) -> str:
        rows = self.pick(t, n or self.n())
        return f"UPDATE {t.name} SET {', '.join(sets)} WHERE {t.pk} IN {rows}"

    def has_incoming_constraint(self, t: Table) -> bool:
        return any(fk.on_delete for _, fk in self.children(t))

    def statement(self, i: int) -> str:
        r = self.rng
        t = r.choice(self.tables)
        ops = ["insert"] * 5 + ["update"] * 6 + ["delete"] * 3
        if t.fks:
            ops += ["move"] * 5
        if t.soft:
            ops += ["soft"] * 2
        if not self.has_incoming_constraint(t):
            ops += ["rekey"]
        op = r.choice(ops)
        if op == "insert":
            cols = [c for c, _ in t.cols] + [fk.column for fk in t.fks]
            rows = [
                "("
                + ", ".join(
                    [self.value(typ, i) for _, typ in t.cols]
                    + [self.fk_value(fk) for fk in t.fks]
                )
                + ")"
                for _ in range(self.n())
            ]
            return f"INSERT INTO {t.name} ({', '.join(cols)}) VALUES {', '.join(rows)}"
        if op == "update":
            sets = []
            for c, typ in r.sample(t.cols, r.randint(1, min(2, len(t.cols)))):
                sets.append(
                    {
                        "text": f"{c} = {c} || '.{i}'",
                        "int": f"{c} = coalesce({c}, 0) + {r.randint(1, 5)}",
                        "bool": f"{c} = NOT coalesce({c}, false)",
                        "numeric": f"{c} = coalesce({c}, 0) + 1.25",
                        "jsonb": f"{c} = coalesce({c}, '{{}}') || '{{\"u\": {i}}}'",
                    }.get(typ, f"{c} = {self.value(typ, i)}")
                )
            return self.update(t, sets)
        if op == "move":
            sets = [f"{fk.column} = {self.fk_value(fk)}" for fk in r.sample(t.fks, 1)]
            if self.chance(0.3):
                c, typ = r.choice(t.cols)
                sets.append(f"{c} = {self.value(typ, i)}")
            return self.update(t, sets)
        if op == "soft":
            return (
                f"UPDATE {t.name} SET deleted_at = CASE WHEN deleted_at IS NULL "
                f"THEN '2026-06-0{r.randint(1, 9)} 00:00:00+00'::timestamptz END "
                f"WHERE {t.pk} IN {self.pick(t, self.n())}"
            )
        if op == "rekey":
            return self.update(t, [f"{t.pk} = {t.pk} + 1000"], 1)
        return (
            f"DELETE FROM {t.name} WHERE {t.pk} IN {self.pick(t, r.choice([1, 1, 2]))}"
        )

    def check(self, label: str) -> str:
        return f"SELECT gen_check({lit(label)});"

    def write_statements(self) -> None:
        r = self.rng
        i = 1
        while i <= self.n_statements:
            x = r.random()
            if x < MUTATION_SHARE:
                # a fraiseql mutation: one SELECT of a function writing in a few
                # statements, then reading a TVIEW back; pg_tviews refreshes at
                # the first TVIEW read (or at the end of the outer statement)
                self.functions += 1
                fn = f"gen_mutation_{self.functions}"
                body = []
                for _ in range(r.randint(2, 4)):
                    body.append(self.statement(i) + ";")
                    i += 1
                label = f"{fn} (#{i - len(body)}..#{i - 1})"
                body.append(f"PERFORM gen_read_back({r.randint(0, 99)}, {lit(label)});")
                body.append(f"PERFORM gen_check({lit('the writes of ' + label)});")
                self.out.append(
                    f"CREATE FUNCTION {fn}() RETURNS void LANGUAGE plpgsql AS $fn$\n"
                    "BEGIN\n  " + "\n  ".join(body) + "\nEND $fn$;"
                )
                self.out.append(f"SELECT {fn}();")
                self.out.append(self.check(label))
            elif x < MUTATION_SHARE + TRANSACTION_SHARE:
                # a transaction of a few statements, committed or rolled back
                self.out.append("BEGIN;")
                for _ in range(r.randint(2, 3)):
                    stmt = self.statement(i)
                    self.out.append(stmt + ";")
                    self.out.append(self.check(f"#{i} {stmt} (in a transaction)"))
                    i += 1
                end = r.choice(["COMMIT", "COMMIT", "ROLLBACK"])
                self.out.append(end + ";")
                self.out.append(
                    self.check(f"the transaction ending at #{i - 1}: {end}")
                )
            else:
                stmt = self.statement(i)
                self.out.append(stmt + ";")
                self.out.append(self.check(f"#{i} {stmt}"))
                i += 1

    def render(self) -> str:
        self.make_tables()
        self.write_schema()
        self.make_shapes()
        self.write_shapes()
        self.write_statements()
        return "\n".join(self.out) + "\n"


def main() -> None:
    seed, shapes, statements = (int(a) for a in sys.argv[1:4])
    out_dir = Path(sys.argv[4])
    features = set(filter(None, ",".join(sys.argv[5:6]).split(",")))
    unknown = features - set(OPT_IN)
    if unknown:
        sys.exit(f"unknown feature(s) {sorted(unknown)}: {sorted(OPT_IN)}")
    rng = random.Random(f"pg_tviews-gen:{seed}")
    case_no, left = 0, shapes
    while left > 0:
        case_no += 1
        n = rng.randint(MIN_CASE, MAX_CASE)
        if left - n < MIN_CASE:
            # no case of fewer than MIN_CASE shapes unless fewer were asked for
            n = left if left <= MAX_CASE else left - MIN_CASE
        case = Case(rng, n, statements, features)
        sql = case.render()
        name = f"case{case_no}.sql"
        header = (
            f"-- seed {seed}, case {case_no}: python3 -I gen.py {seed} {shapes} "
            f"{statements} <dir> {','.join(sorted(features))}\n"
        )
        (out_dir / name).write_text(header + sql, encoding="utf-8")
        print(name, len(case.shapes))
        left -= n


if __name__ == "__main__":
    main()
