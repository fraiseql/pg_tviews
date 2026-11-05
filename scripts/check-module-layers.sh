#!/usr/bin/env bash
# The lower modules of the crate form layers: each names (`crate::<module>`) only
# modules below it, so no two of them depend on each other, directly or through a
# third. The order, bottom first, is LAYERS below; a module not listed (the
# triggers, hooks, DDL, API, ...) sits above all of them. Comments and tests do not
# count. Prints the violations; exits 1 on any.
set -u
cd "$(dirname "$0")/.."
LAYERS=(config error cache metrics utils owner visibility lineage catalog stats queue concurrency refresh propagate flush)
python3 - "${LAYERS[@]}" <<'PY'
import os, re, sys
layers = sys.argv[1:]
rank = {m: i for i, m in enumerate(layers)}
def files(m):
    if os.path.isdir(f'src/{m}'):
        for r, _, fs in os.walk(f'src/{m}'):
            if '/tests' in r:
                continue
            for f in fs:
                if f.endswith('.rs') and f != 'tests.rs':
                    yield os.path.join(r, f)
    elif os.path.exists(f'src/{m}.rs'):
        yield f'src/{m}.rs'
bad = []
for m in layers:
    for f in files(m):
        in_tests = False
        for n, line in enumerate(open(f), 1):
            s = line.strip()
            if s.startswith('#[cfg(test)]'):
                in_tests = True
            if in_tests or s.startswith('//'):
                continue
            for t in sorted(set(re.findall(r'crate::([a-z_]+)', line))):
                if t in rank and rank[t] > rank[m]:
                    bad.append(f'{f}:{n}: {m} names {t}, above it')
if bad:
    print('lower modules naming a module above them (order: ' + ' < '.join(layers) + '):')
    print('\n'.join(bad))
    sys.exit(1)
print('the lower modules form layers: ' + ' < '.join(layers))
PY
