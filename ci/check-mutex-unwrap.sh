#!/usr/bin/env bash
# Check for `.lock().unwrap()` calls in production Rust code.
#
# Mutex lock poison is a permanent, unrecoverable failure mode: if any thread
# panics while holding a shared Mutex, every subsequent `.lock().unwrap()` on
# that Mutex panics forever.  The `catch_unwind` boundary in the event loop
# catches the panic but the poisoned lock persists, creating an infinite loop
# where the UI processes zero events.
#
# Use `.lock().unwrap_or_else(|err| err.into_inner())` instead — it recovers
# from poisoning by taking the inner guard, so a single panic never cascades.
#
# This script greps for `.lock().unwrap()` in non-test Rust source files and
# fails if any matches are found.  Test code (`#[cfg(test)]` modules,
# `tests/` directories) is excluded.

set -euo pipefail

FAIL=0

# Find all .rs files, excluding test directories and generated code.
while IFS= read -r file; do
    # Skip test directories entirely.
    if echo "$file" | grep -qE '(^|/)tests/|/test[_s]?/' ; then
        continue
    fi
    # Skip files that are entirely test code.
    if grep -qE '^#\[cfg\(test\)\]' "$file" 2>/dev/null; then
        # Check if the match is inside a #[cfg(test)] module by looking
        # for the pattern after the last occurrence of #[cfg(test)].
        # This is a heuristic — exact scope analysis requires a Rust parser.
        matches=$(grep -n '\.lock()\.unwrap()' "$file" 2>/dev/null || true)
        if [ -n "$matches" ]; then
            # For each match, check if it's before or after the first #[cfg(test)]
            first_test_line=$(grep -n '^#\[cfg(test)\]' "$file" | head -1 | cut -d: -f1)
            if [ -n "$first_test_line" ]; then
                while IFS= read -r match; do
                    line_num=$(echo "$match" | cut -d: -f1)
                    if [ "$line_num" -lt "$first_test_line" ]; then
                        echo "PRODUCTION: $file:$match"
                        FAIL=1
                    fi
                done <<< "$matches"
            else
                # No #[cfg(test)] found — all matches are production code.
                while IFS= read -r match; do
                    echo "PRODUCTION: $file:$match"
                    FAIL=1
                done <<< "$matches"
            fi
        fi
    else
        # No #[cfg(test)] at all — all matches are production code.
        matches=$(grep -n '\.lock()\.unwrap()' "$file" 2>/dev/null || true)
        if [ -n "$matches" ]; then
            while IFS= read -r match; do
                echo "PRODUCTION: $file:$match"
                FAIL=1
            done <<< "$matches"
        fi
    fi
done < <(find . -name '*.rs' -not -path '*/target/*' -not -path '*/tests/*' -not -path '*/test_*' -not -name '*_test.rs')

if [ "$FAIL" -eq 1 ]; then
    echo ""
    echo "ERROR: Found .lock().unwrap() in production code."
    echo "Use .lock().unwrap_or_else(|err| err.into_inner()) instead."
    echo "See ci/check-mutex-unwrap.sh for details."
    exit 1
fi

echo "OK: No .lock().unwrap() in production code."
