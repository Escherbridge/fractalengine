# fe-identity/src — keystore + token rationale

## §secret-store (`secret_store/`)

`SecretStore` is a pluggable trait for string secrets keyed by
`(service, account)`; implementations are `Send + Sync + 'static` so they can
be shared across threads as `Arc<dyn SecretStore>`. Three backends:

- `InMemoryBackend` — always compiled; tests use it to avoid touching the OS
  keychain.
- `OsKeystoreBackend` (`secret_store/os_keystore.rs`) — wraps
  `keyring::Entry`; compiled only under the `keyring` feature so headless and
  CI builds don't require a platform keystore.
- `EnvBackend` — maps `(service, account)` to env var
  `FE_SECRET_{SERVICE}_{ACCOUNT}` (components uppercased, non-alphanumerics
  replaced with `_`). Reads prefer the real environment, then fall back to a
  runtime-written in-memory override map; `set` writes only to that map and
  never mutates the process environment (env mutation is process-global and
  race-prone).

`keychain.rs` layers node-keypair persistence on top of the trait: the
32-byte Ed25519 seed is hex-encoded and stored under service
`"fractalengine"`, keyed by node id.

## §api-token (`api_token.rs`)

`ApiClaims` is a deliberately separate JWT type from `FractalClaims`
(`jwt.rs`) for the Realtime API Gateway:

- TTL up to 30 days (`MAX_API_TOKEN_TTL_SECS`) vs. the 300-second session
  token.
- `scope` authorizes at any hierarchy level (`"VERSE#v1"`,
  `"VERSE#v1-FRACTAL#f1"`, ...), not just petal.
- `token_type: "api"` discriminates from session tokens so middleware can
  route without trial-decoding both claim shapes.
- `jti` gives each token a unique id for server-side revocation tracking.

Tokens are Ed25519-signed with the node keypair; minting rejects empty scopes
and TTLs over the maximum.
