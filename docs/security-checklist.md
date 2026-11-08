# FractalEngine Pre-Launch Security Checklist

## Operator Checklist (Run before opening to public peers)
- [ ] Rotate the Node's ed25519 keypair (if previously used in testing)
- [ ] Verify SURREAL_DATASTORE_SYNC_DATA is unset or `every` (both binaries default it to `every`; `never` risks losing recent writes and `true` is not a valid value)
- [ ] Confirm no default admin passwords exist (ed25519 key IS the admin credential)
- [ ] Test WebView denylist: navigate to http://127.0.0.1 → must be blocked
- [ ] Verify FE_SYNC_RELAY points at your own iroh relay (or is `disabled` for LAN-only mode)
- [ ] Set JWT lifetime to ≤300s in production config
- [ ] Confirm session TTL cache is flushed on node restart
- [ ] Audit custom roles: no role should have permissions beyond its intended scope
- [ ] Run: cargo audit — zero high-severity vulnerabilities before launch

## Developer Checklist (Run before each release)
- [ ] cargo audit — zero high/critical CVEs
- [ ] cargo clippy -- -D warnings — clean
- [ ] grep -rn 'unwrap()\|expect(' src/ --include='*.rs' | grep -v '#\[cfg(test)\]' — empty
- [ ] grep -rn 'block_on' crates/ --include='*.rs' — empty (no Tokio blocking in Bevy)
- [ ] All gossip inbound paths call verify() before any logic
- [ ] docs/webview-threat-model.md up to date
