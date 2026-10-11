#!/usr/bin/env bash
# Unwrap/expect census: lists every production `.unwrap()` / `.expect()` site in
# the workspace (lib + bin targets only, so #[cfg(test)] code is excluded by
# construction). Read-only: never edits code. See docs/unwrap-audit.md.
#
# Usage: scripts/unwrap-census.sh [OUTPUT_FILE]
#   OUTPUT_FILE  write the report there instead of stdout
#   TOOLCHAIN=1.99.0 scripts/unwrap-census.sh   # pin a rustup toolchain
set -euo pipefail

output_file="${1:-}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
toolchain_arg=()
if [[ -n "${TOOLCHAIN:-}" ]]; then toolchain_arg=("+${TOOLCHAIN}"); fi

raw_log="$(mktemp)"
trap 'rm -f "$raw_log"' EXIT

(cd "$repo_root" && cargo "${toolchain_arg[@]}" clippy --workspace --lib --bins \
    --message-format=short -- -A clippy::all -W clippy::unwrap_used -W clippy::expect_used) \
    >"$raw_log" 2>&1 || {
    echo "unwrap-census: cargo clippy failed; last lines:" >&2
    tail -20 "$raw_log" >&2
    exit 1
}

# `path:line:col: warning: used `unwrap()` on ...` -> `path:line:col kind`
sites="$(tr '\\' '/' <"$raw_log" \
    | sed -nE 's#^([^ :]+\.rs):([0-9]+):([0-9]+): warning: used `(unwrap|expect)\(\)`.*#\1:\2:\3 \4#p' \
    | sort -u)"

report() {
    local total unwraps expects
    total="$(printf '%s\n' "$sites" | grep -c . || true)"
    unwraps="$(printf '%s\n' "$sites" | grep -c ' unwrap$' || true)"
    expects="$(printf '%s\n' "$sites" | grep -c ' expect$' || true)"
    echo "# unwrap/expect census"
    echo "date:    $(date -u +%Y-%m-%dT%H:%MZ)"
    echo "commit:  $(git -C "$repo_root" rev-parse --short HEAD)$(git -C "$repo_root" diff --quiet 2>/dev/null || echo ' (dirty)')"
    echo "command: cargo clippy --workspace --lib --bins --message-format=short -- -A clippy::all -W clippy::unwrap_used -W clippy::expect_used"
    echo "total:   $total ($unwraps unwrap, $expects expect)"
    echo
    echo "## per crate (count crate)"
    printf '%s\n' "$sites" | grep . | cut -d/ -f1 | sort | uniq -c | sort -rn
    echo
    echo "## per file (count file)"
    printf '%s\n' "$sites" | grep . | cut -d: -f1 | sort | uniq -c | sort -rn
    echo
    echo "## sites (file:line:col kind)"
    printf '%s\n' "$sites"
}

if [[ -n "$output_file" ]]; then
    report >"$output_file"
    echo "unwrap-census: wrote $output_file" >&2
else
    report
fi
