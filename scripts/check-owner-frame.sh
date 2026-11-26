#!/usr/bin/env bash
# The tests' oracle reads backing views in the owner frame its specification gives
# (docs/reference/owner-frame.md): the frame blocks generated from it are current.
# The specification against the code is a unit test (src/owner.rs).
set -euo pipefail
cd "$(dirname "$0")/.."
python3 test/sql/lib/gen_frame.py --check
echo "check-owner-frame: the oracle's frame is the specification's"
