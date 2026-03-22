#!/usr/bin/env bash
set -e
echo "=== cargo audit ==="
cargo audit
echo "=== unwrap/expect audit ==="
HITS=$(grep -rn 'unwrap()\|\.expect(' --include='*.rs' \
  fe-runtime fe-identity fe-database fe-network fe-renderer fe-auth fe-webview fe-sync fe-ui fractalengine \
  | grep -v '#\[cfg(test)\]' | grep -v '// SAFETY' | wc -l)
echo "Production unwrap/expect occurrences: $HITS"
if [ "$HITS" -gt "0" ]; then echo "FAIL: unwrap/expect found in production code"; exit 1; fi
echo "=== block_on audit ==="
if grep -rn 'block_on' --include='*.rs' fe-runtime/src fractalengine/src fe-ui/src; then
  echo "FAIL: block_on found in Bevy crates"; exit 1
fi
echo "All checks passed."
