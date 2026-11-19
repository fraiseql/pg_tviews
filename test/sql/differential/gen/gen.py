"""Seeded view-shape generator for the differential oracle (run.sh in this folder).

From a seed, writes cases: each case is a random schema of 2-4 base tables in the
project's convention (tb_<entity>, pk_<entity> integer primary key, id uuid, value
columns, fk_<entity> references), 3-6 TVIEW definitions drawn from the grammar
below, and random writes, each followed by a check that every TVIEW equals its view.
The same seed writes the same cases.

Usage: python3 -I gen.py <seed> <shapes> <statements> <out_dir> [features]

Writes <out_dir>/case<N>.sql until <shapes> TVIEW definitions are written, and
prints one line per case: its file name and its number of shapes. `features`
is a comma-separated list of the opt-in value forms of OPT_IN, and of kinds to draw
half the time.

The grammar (one TVIEW per draw, every one selecting pk_<entity>, id and data),
each kind tagged with the productions of docs/reference/dialect.toml it exercises
(KIND_PRODUCTIONS; coverage.py checks every production is reached):
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
  lookup    a count of another table's rows filtered by constants (=, IN, IS
            NULL, ANDed), linked to no key, under uncascaded_policy full_refresh
  wholerow  a row read whole: to_jsonb(t), jsonb_agg(c ORDER BY ...) of child
            rows, a function of the row type, md5(t::text)
  syscol    a system column that changes only with a write: xmin::text
  pick      one child per parent by LATERAL ... ORDER BY ..., pk LIMIT 1
  filter    count and jsonb_agg with FILTER, string_agg ordered by its values
  exists    EXISTS of a parent's children
  fence     a subquery fenced by OFFSET 0
  ordinality  unnest(...) WITH ORDINALITY, aggregated in its order
  firstrow  ROW_NUMBER() OVER (PARTITION BY fk_ ORDER BY ..., pk) ... rn = 1
  time      a value read against CURRENT_DATE, declared time_refresh external
Any of them may filter soft-deleted rows (deleted_at IS NULL). An embedding TVIEW
may carry a user trigger writing back, during the flush, to the row it embeds;
a `derived` TVIEW reads a copied column again in an expression.

Kinds outside the dialect, whose creation must fail with 0A000 (Shape.expect):
  unordered an output that depends on row order: an aggregate without ORDER BY,
            LIMIT 1 without ORDER BY, DISTINCT ON and row_number() over ties
  refused   a volatile function in the output or a filter, a built-in reading
            the session, ctid, CURRENT_USER
Every table carries a tenant_id, and every row references parents of its own
tenant only (inserts and moves pick parents of the row's tenant; a flip moves a
row with no children to the other tenant, with its parents). Every TVIEW projects
the tenant of its root row, and the readers' policies on the base tables and on
tv_* filter on it: the documented row security pattern (docs/reference/ddl.md,
"Row-level security on tv_*"), checked by the visibility oracle (visibility.sql)
after each check. The tenant choices draw from their own random stream, so the
shapes and writes of a seed are those it drew before.

`exact` changes the written values instead: numerics with 30 digits and with
trailing zeros (2.50), jsonb holding decimals, so a value that loses digits or
scale on the way into a document is seen (the oracle compares binary images).
"""

import json
import random
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

# The productions of docs/reference/dialect.toml: every shape names those it
# exercises, and coverage.py checks each production is reached.
DIALECT = Path(__file__).resolve().parents[4] / "docs/reference/dialect.toml"
PRODUCTIONS = {p["id"] for p in tomllib.loads(DIALECT.read_text())["production"]}

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
# The opt-in kinds and value forms (the module docstring), until the defects
# they reproduce are fixed.
OPT_IN_KINDS: list[str] = []
OPT_IN: set[str] = {*OPT_IN_KINDS, "exact"}
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
    "lookup",
    "derived",
    "wholerow",
    "syscol",
    "unordered",
    "pick",
    "filter",
    "exists",
    "fence",
    "ordinality",
    "firstrow",
    "time",
    "refused",
]
# The productions each kind exercises (a kind with forms adds its form's own).
KIND_PRODUCTIONS = {
    "plain": ["plain"],
    "join": ["join", "copied-parent"],
    "embed": ["embedded-tview"],
    "subagg": ["correlated-subquery"],
    "array": ["correlated-subquery", "order-rule"],
    "union": ["union"],
    "cte": ["cte"],
    "group": ["group-by"],
    "last": ["distinct-on", "order-rule"],
    "top": ["distinct-on", "order-rule"],
    "lookup": ["uncorrelated-subquery"],
    "derived": ["plain"],
    "wholerow": ["whole-row"],
    "syscol": ["xmin-tableoid"],
    "unordered": ["order-rule"],
    "pick": ["correlated-subquery", "order-rule"],
    "filter": ["correlated-subquery", "order-rule"],
    "exists": ["correlated-subquery"],
    "fence": ["plain"],
    "ordinality": ["correlated-subquery", "order-rule"],
    "firstrow": ["window-partition", "order-rule"],
    "time": ["time-dependent"],
    "refused": [],
}
assert set(KIND_PRODUCTIONS) == set(KINDS), "every kind names its productions"
assert all(p in PRODUCTIONS for ps in KIND_PRODUCTIONS.values() for p in ps)


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
    # the tenant of each initial row (1 or 2), by pk - 1
    tenants: list[int] = field(default_factory=list)

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
    # its rows read rows of every tenant (a lookup, or a TVIEW embedding one):
    # the visibility oracle compares its keys only
    keys_only: bool = False
    # statements run after the TVIEW is created (when it is)
    after: list[str] = field(default_factory=list)
    # the dialect.toml productions it exercises
    productions: list[str] = field(default_factory=list)
    # the SQLSTATE its creation must fail with, None when it must be created
    expect: str | None = None


class Case:
    def __init__(
        self, rng: random.Random, n_shapes: int, n_statements: int, features: set[str]
    ) -> None:
        self.rng = rng
        # The tenant choices: their own stream, so the shapes and writes drawn
        # from `rng` are those a seed drew before tenants existed.
        self.vr = random.Random(repr(rng.getstate()[1][:8]))
        # The column the reader without a policy-free column cannot select.
        self.revoked: tuple[Table, str] | None = None
        self.features = features
        self.n_shapes = n_shapes
        self.n_statements = n_statements
        self.tables: list[Table] = []
        self.shapes: list[Shape] = []
        self.out: list[str] = []
        self.functions = 0
        # A kind named in `features` (an opt-in one, or any other to weight it) is
        # half the draws: it is what the run is for.
        opted = [k for k in [*KINDS, *OPT_IN_KINDS] if k in features]
        self.kinds = KINDS + [k for k in opted if k in OPT_IN_KINDS]
        self.kinds += opted * (len(KINDS) // max(len(opted), 1))

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
        # One column of one table the column-scoped reader may not select.
        rt = self.vr.choice(self.tables)
        self.revoked = (rt, self.vr.choice(rt.cols)[0])

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
        if "exact" in self.features and typ in ("numeric", "jsonb"):
            return self.exact_value(typ, n)
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
            "jsonb": lambda: (
                lit(f'{{"k": {n % 10}, "tags": ["t{n % 10}"]}}') + "::jsonb"
            ),
        }[typ]()

    def exact_value(self, typ: str, n: int) -> str:
        """A numeric with 30 digits or trailing zeros, or a jsonb holding one."""
        r = self.rng
        num = r.choice(
            [
                f"{n}.{r.randint(0, 10**28 - 1):028d}",
                f"{n}.50",
                f"{n}.0",
                f"0.{r.randint(1, 9)}0000000000000000000001",
                f"{r.randint(10**22, 10**23)}",
            ]
        )
        if typ == "numeric":
            return num
        return lit(f'{{"k": {num}, "tags": ["t{n % 10}"]}}') + "::jsonb"

    def grants(self, t: Table) -> str:
        """The roles harness (lib.sql): the TVIEW owner reads, the writer writes.
        Every other table hides some of its rows from the writer (row security
        that does not apply to the owner, who bypasses it): what the writer
        cannot see must still be refreshed."""
        sql = (
            f'GRANT SELECT, TRIGGER ON {t.name} TO :"gen_owner";\n'
            f'GRANT SELECT, INSERT, UPDATE, DELETE ON {t.name} TO :"gen_writer";'
        )
        sql += f"\nALTER TABLE {t.name} ENABLE ROW LEVEL SECURITY;"
        if self.tables.index(t) % 2 == 1:
            sql += (
                f'\nCREATE POLICY gen_writer_rows ON {t.name} TO :"gen_writer"\n'
                f"  USING ({t.pk} % 5 <> 4) WITH CHECK (true);"
            )
        else:
            sql += (
                f'\nCREATE POLICY gen_writer_all ON {t.name} TO :"gen_writer"\n'
                f"  USING (true) WITH CHECK (true);"
            )
        return sql + "\n" + self.reader_grants(t)

    def reader_grants(self, t: Table) -> str:
        """The readers of the visibility oracle (visibility.sql): readers A and B
        of tenants 1 and 2, reader C of tenant 1, who may not select one column
        of one table, and reader N, whom no policy lets read a row."""
        cols = [t.pk, "id", "tenant_id"] + [c for c, _ in t.cols]
        cols += (["deleted_at"] if t.soft else []) + [fk.column for fk in t.fks]
        c_cols = ""
        if self.revoked and self.revoked[0] is t:
            c_cols = " (" + ", ".join(c for c in cols if c != self.revoked[1]) + ")"
        sql = (
            f'GRANT SELECT ON {t.name} TO :"gen_reader_a", :"gen_reader_b", '
            f':"gen_reader_n";\n'
            f'GRANT SELECT{c_cols} ON {t.name} TO :"gen_reader_c";\n'
            f"CREATE POLICY gen_tenant ON {t.name} FOR SELECT\n"
            f'  TO :"gen_reader_a", :"gen_reader_b", :"gen_reader_c"\n'
            f"  USING (tenant_id = nullif(current_setting('gen.tenant', true), '')::int);"
        )
        if t.fks:
            # Self-test 3 ("narrower"): a child table reader A reads less of.
            sql += f"\nSELECT gen_narrow({lit(t.name)}, {lit(t.pk)});"
        return sql

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
            lines.append("tenant_id int NOT NULL")
            if t.soft:
                lines.append("deleted_at timestamptz")
            for fk in t.fks:
                ref = ""
                if fk.on_delete:
                    ref = f" REFERENCES {fk.parent.name} ON DELETE {fk.on_delete}"
                lines.append(f"{fk.column} {fk.parent.pk_type}{ref}")
            self.out.append(f"CREATE TABLE {t.name} (\n  " + ",\n  ".join(lines) + ");")
            self.out.append(self.grants(t))
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
                refs: list[int | None] = []
                for fk in t.fks:
                    if self.chance(0.1):
                        refs.append(None)
                    elif fk.on_delete is None and self.chance(0.05):
                        refs.append(999)
                    else:
                        refs.append(r.randint(1, fk.parent.rows))
                tenant = self.initial_tenant(t, refs)
                t.tenants.append(tenant)
                v.append(str(tenant))
                v += ["NULL" if ref is None else str(ref) for ref in refs]
                values.append("(" + ", ".join(v) + ")")
            cols = (
                [c for c, _ in t.cols]
                + (["deleted_at"] if t.soft else [])
                + ["tenant_id"]
                + [fk.column for fk in t.fks]
            )
            self.out.append(
                f"INSERT INTO {t.name} ({', '.join(cols)}) VALUES\n  "
                + ",\n  ".join(values)
                + ";"
            )

    def initial_tenant(self, t: Table, refs: list[int | None]) -> int:
        """The tenant of an initial row of `t` referencing `refs` (a parent's
        pk, 999 for none, or None), each moved in place to a parent of the row's
        tenant: its first existing parent's, else drawn."""
        tenant = None
        for fk, ref in zip(t.fks, refs, strict=True):
            if ref is not None and ref <= fk.parent.rows:
                tenant = fk.parent.tenants[ref - 1]
                break
        if tenant is None:
            tenant = self.vr.choice([1, 2])
        for i, (fk, ref) in enumerate(zip(t.fks, refs, strict=True)):
            if ref is None or ref > fk.parent.rows:
                continue
            n = fk.parent.rows
            same = [
                (ref - 1 + k) % n + 1
                for k in range(n)
                if fk.parent.tenants[(ref - 1 + k) % n] == tenant
            ]
            refs[i] = same[0] if same else None
        return tenant

    def tenant_col(self, alias: str) -> str:
        """The root row's tenant, projected beside data for the policy on tv_*."""
        return f", {alias}.tenant_id"

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
        s = getattr(self, f"shape_{kind}")()
        if s:
            s.productions = list(dict.fromkeys(KIND_PRODUCTIONS[kind] + s.productions))
            assert all(p in PRODUCTIONS for p in s.productions), s.productions
        return s

    def shape_plain(self) -> Shape:
        t = self.rng.choice(self.tables)
        tv = self.tv_name(t.entity)
        key = "pk_" + tv[3:]
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}{self.extra_cols('t', t)}, "
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
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}{extra}, "
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
            if s.keyed_by is fk.parent and s.expect is None
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
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}{extra}, "
            f"jsonb_build_object({self.obj('t', t)}, {lit(fk.parent.entity)}, v.data)"
            " AS data\n"
            f"  FROM {t.name} t\n  {'LEFT ' if left else ''}JOIN {s.tv} v "
            f"ON v.{s.key} = t.{fk.column}" + self.where([self.soft("t", t)])
        )
        shape = Shape(tv, key, sql, {}, [s.tv], t, keys_only=s.keys_only)
        if self.chance(0.8):
            shape.after = self.writeback(tv, key, t, fk)
        return shape

    def writeback(self, tv: str, key: str, t: Table, fk: Fk) -> list[str]:
        """A user trigger on the embedding TVIEW's table that marks, once, the
        embedded row's text columns: a write during the flush to what the flush
        refreshed already, so the TVIEWs reading it are refreshed again."""
        p = fk.parent
        texts = p.cols_of(("text",))
        if not texts:
            return []
        sets = ", ".join(f"{c} = coalesce(p.{c}, '') || '+'" for c in texts)
        fn = f"gen_writeback_{tv[3:]}"
        return [
            (
                f"CREATE FUNCTION {fn}() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER\n"
                f"  SET search_path = public AS $f$\nBEGIN\n"
                f"  UPDATE {p.name} p SET {sets} FROM {t.name} t\n"
                f"   WHERE t.{t.pk} = NEW.{key} AND p.{p.pk} = t.{fk.column}\n"
                f"     AND coalesce(p.{texts[0]}, '') NOT LIKE '%+';\n"
                f"  RETURN NULL;\nEND $f$;"
            ),
            (
                f"DO $w$ BEGIN IF to_regclass({lit(tv)}) IS NOT NULL THEN\n"
                f"  CREATE TRIGGER gen_writeback AFTER UPDATE ON {tv}\n"
                f"    FOR EACH ROW EXECUTE FUNCTION {fn}();\nEND IF; END $w$;"
            ),
        ]

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
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, "
            f"jsonb_build_object({', '.join(parts)}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_lookup(self) -> Shape | None:
        """A read of another table linked to no key, filtered by constants: a
        write refreshes the TVIEW in full only when a row it changed meets the
        filter (#284)."""
        r = self.rng
        t = r.choice(self.tables)
        others = [o for o in self.tables if o is not t and o.cols_of(("int", "bool"))]
        if not others:
            return None
        o = r.choice(others)
        tv = self.tv_name(f"{t.entity}_k")
        key = "pk_" + tv[3:]
        col, typ = r.choice([(c, ty) for c, ty in o.cols if ty in ("int", "bool")])
        if typ == "bool":
            conds = [f"x.{col} = {r.choice(['true', 'false'])}"]
        elif self.chance(0.5):
            conds = [f"x.{col} = {r.randint(-3, 40)}"]
        else:
            values = ", ".join(str(r.randint(-3, 40)) for _ in range(3))
            conds = [f"x.{col} IN ({values})"]
        if self.chance(0.4):
            conds.append(f"x.{r.choice(o.cols)[0]} IS NULL")
        if o.soft and self.chance(0.3):
            conds.append("x.deleted_at IS NULL")
        lookup = f"(SELECT count(*) FROM {o.name} x WHERE {' AND '.join(conds)})"
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, "
            f"jsonb_build_object({self.obj('t', t)}, 'k', {lookup}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(
            tv, key, sql, {"uncascaded_policy": "full_refresh"}, [], t, keys_only=True
        )

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
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object({self.obj('t', t)}, "
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
            f"SELECT a.{a.pk} AS {key}, a.id{self.tenant_col('a')}, "
            f"jsonb_build_object('kind', {lit(a.entity)}, {self.obj('a', a)}) AS data\n"
            f"  FROM {a.name} a"
            + self.where([self.soft("a", a)])
            + f"\nUNION ALL\nSELECT -b.{b.pk}, b.id{self.tenant_col('b')}, "
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
            f"WITH x AS (SELECT t.{t.pk}, t.id, t.tenant_id, {inner}"
            + (f", t.{fk.column}" if fk else "")
            + f" FROM {t.name} t{where})\n"
            f"SELECT x.{t.pk} AS {key}, x.id{self.tenant_col('x')}, jsonb_build_object({self.obj('x', t)}"
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
            f"SELECT c.{fk.column} AS {key}, p.id{self.tenant_col('p')}, "
            f"jsonb_build_object({', '.join(parts)}) AS data\n"
            f"  FROM {c.name} c JOIN {p.name} p ON p.{p.pk} = c.{fk.column}"
            + self.where([self.soft("c", c), self.soft("p", p)])
            + f"\n  GROUP BY c.{fk.column}, p.id, p.tenant_id, p.{pcol}"
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
            f"SELECT DISTINCT ON (c.{fk.column}) c.{fk.column} AS {key}, p.id{self.tenant_col('p')}, "
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
            f"SELECT DISTINCT ON (p.{p.pk}) p.{p.pk} AS {key}, p.id{self.tenant_col('p')}, "
            f"jsonb_build_object({self.obj('p', p)}, {lit('top_' + col)}, c.{col})"
            " AS data\n"
            f"  FROM {p.name} p LEFT JOIN {c.name} c ON {on}"
            + self.where([self.soft("p", p)])
            + f"\n  ORDER BY p.{p.pk}, c.{order} DESC NULLS LAST, c.{c.pk} DESC"
        )
        return Shape(tv, key, sql, {}, [], p)

    def shape_wholerow(self) -> Shape | None:
        """A row read whole, in one of the forms the dialect admits."""
        r = self.rng
        form = r.choice(["to_jsonb", "agg", "function", "text"])
        if form == "agg":
            pairs = self.child_pairs()
            if not pairs:
                return None
            p, c, fk = r.choice(pairs)
            tv = self.tv_name(f"{p.entity}_w")
            key = "pk_" + tv[3:]
            agg = f"jsonb_agg(c ORDER BY c.{c.pk})"
            sql = (
                f"SELECT t.{p.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object({self.obj('t', p)}, "
                f"{lit(c.entity + 's')}, {self.child_subquery(p, c, fk, agg)}) AS data\n"
                f"  FROM {p.name} t" + self.where([self.soft("t", p)])
            )
            return Shape(tv, key, sql, {}, [], p)
        t = r.choice(self.tables)
        tv = self.tv_name(f"{t.entity}_w")
        key = "pk_" + tv[3:]
        if form == "to_jsonb":
            value = "to_jsonb(t)"
        elif form == "text":
            value = "md5(t::text)"
        else:
            fn = f"gen_row_{t.entity}"
            if not any(line.startswith(f"CREATE FUNCTION {fn}(") for line in self.out):
                self.out.append(
                    f"CREATE FUNCTION {fn}({t.name}) RETURNS text IMMUTABLE "
                    f"LANGUAGE sql AS $f$ SELECT $1::text $f$;"
                )
            value = f"{fn}(t)"
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', t)}, 'row', {value}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_derived(self) -> Shape:
        """A column copied into data and read again by an expression of it."""
        t = self.rng.choice(self.tables)
        c, typ = self.rng.choice(t.cols)
        derive = {
            "text": f"upper(t.{c})",
            "int": f"t.{c} * 2",
            "numeric": f"t.{c} * 2",
            "bool": f"NOT t.{c}",
            "timestamptz": f"t.{c} + interval '1 day'",
            "jsonb": f"t.{c} ? 'u'",
        }[typ]
        tv = self.tv_name(f"{t.entity}_d")
        key = "pk_" + tv[3:]
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{lit(c)}, t.{c}, {lit(c + '_d')}, {derive}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_syscol(self) -> Shape:
        """A system column of the row: its version."""
        t = self.rng.choice(self.tables)
        tv = self.tv_name(f"{t.entity}_x")
        key = "pk_" + tv[3:]
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', t)}, 'version', t.xmin::text) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_unordered(self) -> Shape | None:
        """An output that depends on the order rows are read in."""
        r = self.rng
        pairs = self.child_pairs()
        if not pairs:
            return None
        p, c, fk = r.choice(pairs)
        tv = self.tv_name(f"{p.entity}_o")
        key = "pk_" + tv[3:]
        col = r.choice(c.cols)[0]
        form = r.choice(["agg", "limit", "distinct_on", "row_number"])
        if form == "agg":
            value = self.child_subquery(p, c, fk, f"jsonb_agg(c.{col})")
        elif form == "limit":
            value = self.child_subquery(p, c, fk, f"c.{col}")[:-1] + " LIMIT 1)"
        elif form == "distinct_on":
            # one child per parent, ordered by a column that ties
            other = r.choice(c.cols)[0]
            value = (
                f"(SELECT DISTINCT ON (c.{fk.column}) c.{col} FROM {c.name} c "
                f"WHERE c.{fk.column} = t.{p.pk} ORDER BY c.{fk.column}, c.{other} IS NULL)"
            )
        else:
            value = (
                f"(SELECT jsonb_agg(x.{col} ORDER BY x.rn) FROM (SELECT c.{col}, "
                f"row_number() OVER (ORDER BY c.{fk.column}) AS rn FROM {c.name} c "
                f"WHERE c.{fk.column} = t.{p.pk}) x)"
            )
        sql = (
            f"SELECT t.{p.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', p)}, 'o', {value}) AS data\n"
            f"  FROM {p.name} t" + self.where([self.soft("t", p)])
        )
        return Shape(tv, key, sql, {}, [], p, expect="0A000")

    def shape_pick(self) -> Shape | None:
        """One child per parent, picked by LATERAL … ORDER BY … LIMIT 1."""
        r = self.rng
        pairs = self.child_pairs()
        if not pairs:
            return None
        p, c, fk = r.choice(pairs)
        tv = self.tv_name(f"{p.entity}_pick")
        key = "pk_" + tv[3:]
        order = r.choice(c.cols_of(ORDERABLE))
        col = r.choice(c.cols)[0]
        cond = " AND ".join(
            x for x in [f"c.{fk.column} = t.{p.pk}", self.soft("c", c)] if x
        )
        sql = (
            f"SELECT t.{p.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', p)}, 'pick', f.{col}) AS data\n"
            f"  FROM {p.name} t\n  LEFT JOIN LATERAL (SELECT c.{col} FROM {c.name} c "
            f"WHERE {cond}\n    ORDER BY c.{order} DESC NULLS LAST, c.{c.pk} LIMIT 1) f ON true"
            + self.where([self.soft("t", p)])
        )
        return Shape(tv, key, sql, {}, [], p)

    def shape_filter(self) -> Shape | None:
        """Aggregates with FILTER, and string_agg, ordered by the key."""
        r = self.rng
        pairs = self.child_pairs()
        if not pairs:
            return None
        p, c, fk = r.choice(pairs)
        tv = self.tv_name(f"{p.entity}_f")
        key = "pk_" + tv[3:]
        text = c.cols_of(("text",))[0]
        cond = r.choice(
            [f"c.{text} LIKE 'a%'", f"c.{text} IS NOT NULL"]
            + [f"c.{n} > 10" for n in c.cols_of(("int",))]
        )
        sel = (
            f"jsonb_build_object('n', count(*) FILTER (WHERE {cond}), "
            f"'l', jsonb_agg(c.{text} ORDER BY c.{c.pk}) FILTER (WHERE {cond}), "
            f"'s', string_agg(c.{text}, ',' ORDER BY c.{text}))"
        )
        sql = (
            f"SELECT t.{p.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', p)}, 'f', {self.child_subquery(p, c, fk, sel)}) AS data\n"
            f"  FROM {p.name} t" + self.where([self.soft("t", p)])
        )
        return Shape(tv, key, sql, {}, [], p)

    def shape_exists(self) -> Shape | None:
        """Whether a parent has children: EXISTS, and NOT EXISTS in a filter."""
        r = self.rng
        pairs = self.child_pairs()
        if not pairs:
            return None
        p, c, fk = r.choice(pairs)
        tv = self.tv_name(f"{p.entity}_x")
        key = "pk_" + tv[3:]
        has = f"EXISTS {self.child_subquery(p, c, fk, '1')}"
        sql = (
            f"SELECT t.{p.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', p)}, 'has_{c.entity}', {has}) AS data\n"
            f"  FROM {p.name} t" + self.where([self.soft("t", p)])
        )
        return Shape(tv, key, sql, {}, [], p)

    def shape_fence(self) -> Shape:
        """A subquery fenced by OFFSET 0 (the planner does not pull it up)."""
        t = self.rng.choice(self.tables)
        tv = self.tv_name(f"{t.entity}_o0")
        key = "pk_" + tv[3:]
        inner = ", ".join(f"t.{c}" for c, _ in t.cols)
        where = self.where([self.soft("t", t)]).replace("\n  ", " ")
        sql = (
            f"SELECT x.{t.pk} AS {key}, x.id{self.tenant_col('x')}, jsonb_build_object("
            f"{self.obj('x', t)}) AS data\n"
            f"  FROM (SELECT t.{t.pk}, t.id, t.tenant_id, {inner} FROM {t.name} t{where} "
            "OFFSET 0) x"
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_ordinality(self) -> Shape:
        """The words of a text column, WITH ORDINALITY, joined in their order."""
        t = self.rng.choice(self.tables)
        text = t.cols_of(("text",))[0]
        tv = self.tv_name(f"{t.entity}_w")
        key = "pk_" + tv[3:]
        words = (
            f"(SELECT string_agg(w.part, '/' ORDER BY w.n) FROM unnest(string_to_array("
            f"t.{text}, '_')) WITH ORDINALITY w(part, n))"
        )
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', t)}, 'parts', {words}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {}, [], t)

    def shape_firstrow(self) -> Shape | None:
        """Each parent's first child: ROW_NUMBER() … rn = 1, joined back."""
        r = self.rng
        pairs = self.child_pairs()
        if not pairs:
            return None
        p, c, fk = r.choice(pairs)
        tv = self.tv_name(f"{p.entity}_first_{c.entity}")
        key = "pk_" + tv[3:]
        order = r.choice(c.cols_of(ORDERABLE))
        col = r.choice(c.cols)[0]
        soft = self.where([self.soft("c", c)]).replace("\n  ", " ")
        sql = (
            f"SELECT t.{p.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', p)}, 'first', f.{col}) AS data\n"
            f"  FROM {p.name} t\n  LEFT JOIN (SELECT c.{fk.column}, c.{col}, row_number() OVER "
            f"(PARTITION BY c.{fk.column} ORDER BY c.{order} NULLS FIRST, c.{c.pk}) AS rn "
            f"FROM {c.name} c{soft}) f\n    ON f.{fk.column} = t.{p.pk} AND f.rn = 1"
            + self.where([self.soft("t", p)])
        )
        return Shape(tv, key, sql, {}, [], p)

    def shape_time(self) -> Shape:
        """A value read against the current date, declared refreshed externally.
        The written times are in 2026, far from any boundary the run crosses."""
        t = self.rng.choice(self.tables)
        tv = self.tv_name(f"{t.entity}_t")
        key = "pk_" + tv[3:]
        times = t.cols_of(("timestamptz",))
        test = (
            f"t.{times[0]} < CURRENT_DATE + 36500"
            if times
            else "CURRENT_DATE > DATE '2000-01-01'"
        )
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', t)}, 'current', {test}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t)])
        )
        return Shape(tv, key, sql, {"time_refresh": "external"}, [], t)

    def shape_refused(self) -> Shape:
        """A definition outside the dialect (Axes C and D): refused with 0A000."""
        r = self.rng
        t = r.choice(self.tables)
        tv = self.tv_name(f"{t.entity}_r")
        key = "pk_" + tv[3:]
        production, value = r.choice(
            [
                ("volatile", "random() < 2"),
                ("volatile", "gen_random_uuid()"),
                ("session-reader", "pg_backend_pid()"),
                ("session-reader", "SESSION_USER"),
                ("unstable-system-column", "t.ctid::text"),
                ("reader-function", "CURRENT_USER"),
            ]
        )
        if production == "volatile" and self.chance(0.3):
            # in a filter rather than the output
            cond, value = "random() >= 0", "1"
        else:
            cond = None
        sql = (
            f"SELECT t.{t.pk} AS {key}, t.id{self.tenant_col('t')}, jsonb_build_object("
            f"{self.obj('t', t)}, 'v', {value}) AS data\n"
            f"  FROM {t.name} t" + self.where([self.soft("t", t), cond])
        )
        return Shape(
            tv, key, sql, {}, [], None, productions=[production], expect="0A000"
        )

    def make_shapes(self) -> None:
        r = self.rng
        tries = 0
        while len(self.shapes) < self.n_shapes and tries < MAX_DRAWS:
            tries += 1
            s = self.shape(r.choice(self.kinds))
            if s:
                self.shapes.append(s)

    def write_shapes(self) -> None:
        for s in self.shapes:
            options = json.dumps(s.options, sort_keys=True)
            needs = "ARRAY[" + ", ".join(lit(n) for n in s.needs) + "]::text[]"
            productions = (
                "ARRAY[" + ", ".join(lit(p) for p in s.productions) + "]::text[]"
            )
            expect = lit(s.expect) if s.expect else "NULL"
            self.out.append(
                f"SELECT gen_create({lit(s.tv)}, {lit(s.key)}, $def$\n{s.sql}\n$def$,\n"
                f"  {lit(options)}, {needs}, {str(s.keys_only).lower()}, {productions}, {expect});"
            )
            self.out.extend(s.after)

    # ── writes ──────────────────────────────────────────────────────────────
    def pick(self, t: Table, n: int, tenant: str | None = None) -> str:
        """`n` keys of `t` (of tenant `tenant`, an SQL expression, when given)."""
        where = f"WHERE {t.name}.tenant_id = {tenant} " if tenant else ""
        return (
            f"(SELECT {t.pk} FROM {t.name} {where}ORDER BY {t.pk} "
            f"OFFSET {self.rng.randint(0, 9)} LIMIT {n})"
        )

    def n(self) -> int:
        return 1 if self.chance(0.55) else self.rng.randint(2, 4)

    def fk_value(self, fk: Fk, tenant: str) -> str:
        """A reference to a parent of tenant `tenant` (an SQL expression)."""
        if self.chance(0.08):
            return "NULL"
        if fk.on_delete is None and self.chance(0.06):
            return "999"
        return self.pick(fk.parent, 1, tenant)

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
            cols = [c for c, _ in t.cols] + ["tenant_id"] + [fk.column for fk in t.fks]
            rows = []
            for _ in range(self.n()):
                tenant = str(self.vr.choice([1, 2]))
                values = [self.value(typ, i) for _, typ in t.cols] + [tenant]
                values += [self.fk_value(fk, tenant) for fk in t.fks]
                rows.append("(" + ", ".join(values) + ")")
            return f"INSERT INTO {t.name} ({', '.join(cols)}) VALUES {', '.join(rows)}"
        if op == "update":
            sets = []
            for c, typ in r.sample(t.cols, r.randint(1, min(2, len(t.cols)))):
                sets.append(
                    {
                        "text": f"{c} = {c} || '.{i}'",
                        "int": f"{c} = coalesce({c}, 0) + {r.randint(1, 5)}",
                        "bool": f"{c} = NOT coalesce({c}, false)",
                        "numeric": f"{c} = coalesce({c}, 0) + 1.25"
                        if "exact" not in self.features
                        else f"{c} = {self.exact_value('numeric', i)}",
                        "jsonb": f"{c} = coalesce({c}, '{{}}') || '{{\"u\": {i}}}'"
                        if "exact" not in self.features
                        else f"{c} = coalesce({c}, '{{}}') || "
                        f"jsonb_build_object('u', {self.exact_value('numeric', i)})",
                    }.get(typ, f"{c} = {self.value(typ, i)}")
                )
            return self.update(t, sets)
        if op == "move":
            sets = [
                f"{fk.column} = {self.fk_value(fk, f'{t.name}.tenant_id')}"
                for fk in r.sample(t.fks, 1)
            ]
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

    def flip(self) -> str:
        """Move rows of a table that no row references to the other tenant,
        each fk_ to a parent of that tenant: a write to the column the policies
        filter on. Run as the session's superuser: a child the writer cannot
        see still counts."""
        vr = self.vr
        t = vr.choice(self.tables)
        sets = ["tenant_id = 3 - tenant_id"] + [
            f"{fk.column} = (SELECT p.{fk.parent.pk} FROM {fk.parent.name} p "
            f"WHERE p.tenant_id = 3 - {t.name}.tenant_id ORDER BY p.{fk.parent.pk} "
            f"OFFSET {vr.randint(0, 3)} LIMIT 1)"
            for fk in t.fks
        ]
        unreferenced = "".join(
            f" AND NOT EXISTS (SELECT FROM {c.name} c WHERE c.{fk.column} = {t.name}.{t.pk})"
            for c, fk in self.children(t)
        )
        return (
            f"UPDATE {t.name} SET {', '.join(sets)} WHERE {t.pk} IN "
            f"(SELECT {t.pk} FROM {t.name} ORDER BY {t.pk} OFFSET {vr.randint(0, 9)} "
            f"LIMIT {vr.randint(1, 3)}){unreferenced}"
        )

    def check(self, label: str) -> str:
        """Every TVIEW equals its view (as its owner), then each reader reads of
        it only what its evaluation as that reader returns (as the session's
        superuser, who may take the readers' roles)."""
        return (
            f"SELECT gen_check({lit(label)});\nRESET ROLE;\n"
            f"SELECT gen_visible({lit(label)});\n"
            'SET ROLE :"gen_writer";'
        )

    def write_statements(self) -> None:
        r = self.rng
        i = 1
        # Every write, mutation functions included, is the writer's.
        self.out.append('SET ROLE :"gen_writer";')
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
            if self.vr.random() < 0.15:
                flip = self.flip()
                self.out.append(f'RESET ROLE;\n{flip};\nSET ROLE :"gen_writer";')
                self.out.append(self.check(f"a flip: {flip}"))

    def render(self) -> str:
        self.make_tables()
        self.write_schema()
        self.make_shapes()
        self.write_shapes()
        # The read sets, checked by perturbation before any write.
        self.out.append("SELECT gen_perturb();")
        self.write_statements()
        return "\n".join(self.out) + "\n"


def main() -> None:
    seed, shapes, statements = (int(a) for a in sys.argv[1:4])
    out_dir = Path(sys.argv[4])
    features = set(filter(None, ",".join(sys.argv[5:6]).split(",")))
    unknown = features - set(OPT_IN) - set(KINDS)
    if unknown:
        sys.exit(f"unknown feature(s) {sorted(unknown)}: {sorted(OPT_IN | set(KINDS))}")
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
