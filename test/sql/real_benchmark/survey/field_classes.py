"""Static field-class survey for #78: classify each top-level key of a read model's
`data` jsonb_build_object by what it depends on (heuristic, SQL-text based)."""
import re, sys, pathlib, collections

AGG_ARRAY = re.compile(r"\b(jsonb_agg|json_agg|array_agg|jsonb_object_agg|string_agg)\s*\(", re.I)
AGG_SCALAR = re.compile(r"\b(count|sum|avg|min|max|bool_and|bool_or)\s*\(", re.I)
REF = re.compile(r"\b([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)\b", re.I)
ORDER = "ABCDE"

def strip_strings(s):
    return re.sub(r"'(?:[^']|'')*'", "''", s)

def split_args(s):
    out, depth, cur, q = [], 0, [], False
    i = 0
    while i < len(s):
        c = s[i]
        if c == "'":
            q = not q
        if not q:
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
            elif c == "," and depth == 0:
                out.append("".join(cur).strip()); cur = []; i += 1; continue
        cur.append(c); i += 1
    if "".join(cur).strip():
        out.append("".join(cur).strip())
    return out

def call_body(s, start):
    """Body of the call whose '(' is at s[start]."""
    depth, q = 0, False
    for i in range(start, len(s)):
        c = s[i]
        if c == "'":
            q = not q
        elif not q:
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    return s[start + 1:i]
    return None

def unschema(v):
    return re.sub(r"\b[a-z_]\w*\.([a-z_]\w*)\.([a-z_]\w*)\b", r"\1.\2", v, flags=re.I)

def classify(value, root, aliases):
    v = unschema(strip_strings(value)).strip()
    m = re.match(r"(?is)^jsonb?_build_object\s*\(", v)
    if m and call_body(v, m.end() - 1) is not None and len(call_body(v, m.end() - 1)) + m.end() + 1 >= len(v.rstrip()):
        inner = split_args(call_body(v, m.end() - 1))
        classes = [classify(x, root, aliases) for x in inner[1::2]]
        return max(classes, key=ORDER.index) if classes else "A"
    if AGG_ARRAY.search(v):
        return "D"
    if re.search(r"(?i)\bselect\b", v) or AGG_SCALAR.search(v):
        return "E"
    refs = REF.findall(v)
    bare = re.sub(r"::\s*[a-z_ \[\]]+", "", v, flags=re.I).strip()
    if not refs:
        return "A" if re.fullmatch(r"[a-z_][a-z0-9_]*", bare, re.I) else "B"
    quals = {a.lower() for a, _ in refs}
    if quals <= {root}:
        return "A" if re.fullmatch(r"[a-z_][a-z0-9_]*\.[a-z_][a-z0-9_]*", bare, re.I) else "B"
    if len(refs) == 1 and len(quals) == 1:
        return "C"          # one value of a 1:1 joined parent (scalar or .data)
    return "E"               # cross-table expression

def views(text):
    for m in re.finditer(r"(?is)CREATE\s+(?:OR\s+REPLACE\s+)?VIEW\s+([\w.]+)\s+AS(.*?);\s*(?:\n|$)", text):
        yield m.group(1), m.group(2)
    for m in re.finditer(r"(?is)pg_tviews_create\('(\w+)',\s*\$(\w*)\$(.*?)\$\2\$", text):
        yield m.group(1), m.group(3)

def top_level_from(s):
    """Offset of the first FROM at parenthesis depth 0 (the main query's)."""
    depth, q = 0, False
    for m in re.finditer(r"'|\(|\)|\bFROM\b", s, re.I):
        t = m.group(0)
        if t == "'":
            q = not q
        elif q:
            continue
        elif t == "(":
            depth += 1
        elif t == ")":
            depth -= 1
        elif depth == 0:
            return m.start()
    return 0

def survey(body):
    body_nc = unschema(re.sub(r"--[^\n]*", "", body))
    fm = re.compile(r"(?is)\bFROM\s+([\w.]+)(?:\s+(?:AS\s+)?(?!LEFT|JOIN|INNER|WHERE|GROUP|ORDER|CROSS|RIGHT|FULL)([a-z_]\w*))?").match(body_nc, top_level_from(body_nc))
    if not fm:
        return None
    root = (fm.group(2) or fm.group(1).split(".")[-1]).lower()
    aliases = {root}
    for jm in re.finditer(r"(?is)\bJOIN\s+(?:LATERAL\s+)?([\w.]+)\s+(?:AS\s+)?([a-z_]\w*)", body_nc):
        aliases.add(jm.group(2).lower())
    for jm in re.finditer(r"(?is)\bJOIN\s+([\w.]+)\s+ON", body_nc):
        aliases.add(jm.group(1).split(".")[-1].lower())
    dm = re.search(r"(?is)jsonb?_build_object\s*\(", body_nc)
    if not dm:
        return None
    inner = call_body(body_nc, dm.end() - 1)
    if inner is None:
        return None
    args = split_args(inner)
    out = [classify(val, root, aliases) for val in args[1::2]]
    import os
    if os.environ.get("DEBUG"):
        print("root", root, "aliases", sorted(aliases))
        for k, val, c in zip(args[0::2], args[1::2], out):
            print(f"   {c} {k} = {val[:70]}")
    return out

totals = collections.Counter()
per_view = []
for path in sys.argv[1:]:
    for f in pathlib.Path(path).rglob("*.sql") if pathlib.Path(path).is_dir() else [pathlib.Path(path)]:
        if re.search(r"(?i)test|seed|history|archive|backup", str(f)):
            continue
        for name, body in views(f.read_text(errors="ignore")):
            got = survey(body)
            if got:
                c = collections.Counter(got)
                totals.update(c)
                per_view.append((name, c))
n = sum(totals.values())
print(f"views: {len(per_view)}  fields: {n}")
for k in ORDER:
    print(f"  {k}: {totals[k]:5d}  {100*totals[k]/n:5.1f}%")
only_a = sum(1 for _, c in per_view if set(c) <= {"A"})
print(f"views whose data is class A only: {only_a} / {len(per_view)}")
bcd_views = sum(1 for _, c in per_view if set(c) & {"B", "C", "D"})
print(f"views with at least one B/C/D field: {bcd_views}")
