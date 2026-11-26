#!/usr/bin/env bash
# docs/proofs/theorems.md and src/decide/ name each other, and every check they
# name exists:
#   1. every assumption row (PG1, …, and each PG11 entry) has a "Checked by" cell
#      naming at least one file that exists, or saying "outside the theorems";
#   2. every theorem T<n> has a "*Decided by:*" line naming `decide::<fn>` (a public
#      function of src/decide/mod.rs) or a path that exists, and T1–T7 and T10 name
#      a decide function; and a "*Checked by:*" line naming at least one file that
#      exists;
#   3. every backticked path the document names exists;
#   4. every public function of src/decide/mod.rs carries `/// Theorem: T<n>`, for a
#      theorem the document states.
# Prints each violation; exits 1 on any. Optional arguments: the document and the
# decide module (the self-test passes fixtures).
set -u
cd "$(dirname "$0")/.."
python3 -I - "${1:-docs/proofs/theorems.md}" "${2:-src/decide/mod.rs}" <<'PY'
import os, re, sys
doc_path, decide_path = sys.argv[1], sys.argv[2]
doc = open(doc_path).read()
decide = open(decide_path).read()
bad = []
path_re = re.compile(r'`((?:src|test|scripts|docs|decide|sql)/[^`\s]*)`')
decide_fns = set(re.findall(r'^pub (?:const )?fn (\w+)', decide, re.M))

def paths(text):
    return path_re.findall(text)

def exists(p):
    return os.path.exists(p.rstrip('/'))

# 1. Assumption rows: tables whose header has a "Checked by" column.
for table in re.findall(r'(\|[^\n]*Checked by[^\n]*\|\n\|[-| ]+\|\n(?:\|[^\n]*\|\n?)+)', doc):
    rows = table.strip().split('\n')[2:]
    for row in rows:
        cells = [c.strip() for c in row.strip().strip('|').split('|')]
        name, checked = cells[0], cells[-1]
        if 'outside the theorems' in checked or checked == 'each entry below':
            continue
        if not any(exists(p) for p in paths(checked)):
            bad.append(f'{doc_path}: assumption {name[:60]!r} names no existing check')

# 2. Theorems.
theorems = re.findall(r'^\*\*(T\d+)\. (.*?)(?=^\*\*T\d+\. |^Outside every theorem|^## )', doc, re.M | re.S)
stated = {t for t, _ in theorems}
for t, body in theorems:
    decided = re.search(r'\*Decided by:\*(.*?)(?:\n\*|\Z)', body, re.S)
    checked = re.search(r'\*Checked by:\*(.*?)(?:\n\*|\Z)', body, re.S)
    if not decided:
        bad.append(f'{doc_path}: {t} has no "*Decided by:*" line')
    else:
        fns = re.findall(r'`decide::(\w+)`', decided.group(1))
        for f in fns:
            if f not in decide_fns:
                bad.append(f'{doc_path}: {t} names decide::{f}, which {decide_path} does not define')
        if t in {'T1', 'T2', 'T3', 'T4', 'T5', 'T6', 'T7', 'T10'} and not fns:
            bad.append(f'{doc_path}: {t} names no decide function')
        if not fns and not any(exists(p) for p in paths(decided.group(1))):
            bad.append(f'{doc_path}: {t} names no code that exists')
    if not checked or not any(exists(p) for p in paths(checked.group(1))):
        bad.append(f'{doc_path}: {t} names no existing check')

# 3. Every path named exists.
for p in sorted(set(paths(doc))):
    if not exists(p):
        bad.append(f'{doc_path}: names {p}, which does not exist')

# 4. Every decide function names its theorem.
lines = decide.split('\n')
for i, line in enumerate(lines):
    m = re.match(r'pub (?:const )?fn (\w+)', line)
    if not m:
        continue
    j, tag = i - 1, None
    while j >= 0 and (lines[j].startswith('///') or lines[j].startswith('#[')):
        tm = re.match(r'/// Theorem: (T\d+)\b', lines[j])
        tag = tag or (tm and tm.group(1))
        j -= 1
    if not tag:
        bad.append(f'{decide_path}:{i + 1}: {m.group(1)} carries no "/// Theorem: T<n>"')
    elif tag not in stated:
        bad.append(f'{decide_path}:{i + 1}: {m.group(1)} names {tag}, which {doc_path} does not state')

if bad:
    print('\n'.join(bad))
    sys.exit(1)
print(f'theorems: {len(stated)} stated, every assumption and theorem names an existing check, '
      f'{len(decide_fns)} decisions name their theorem')
PY
