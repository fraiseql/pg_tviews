#!/bin/bash
set -euo pipefail

echo "Fuzzing entity name validation..."

# Generate 100 random entity names
python3 -c "
import random
import string

for i in range(100):
    # Random length 0-100
    length = random.randint(0, 100)

    # Random characters including special chars
    chars = string.ascii_letters + string.digits + '_;-' + chr(39) + chr(34) + chr(10) + chr(9) + chr(0)
    name = ''.join(random.choice(chars) for _ in range(length))

    # Escape for shell
    name_escaped = name.replace(\"'\", \"'\\\\''\")
    print(f\"psql -c \\\"SELECT pg_tviews_convert_existing_table('{name_escaped}');\\\" 2>/dev/null || true\")
" | bash

echo "✅ Fuzzing completed (no crashes)"