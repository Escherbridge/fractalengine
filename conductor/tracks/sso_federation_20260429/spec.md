# Track: SSO Federation — OIDC Provider Integration for External Authentication

## Summary

Allow verse owners to configure external SSO/OIDC identity providers (Okta, Authentik, Google, LinkedIn, Azure AD, Keycloak, custom) so that external users can authenticate to the FractalEngine API without needing a locally-minted API token. Uses the standard OIDC token exchange pattern: external users authenticate with their SSO provider, exchange the resulting `id_token` for a FractalEngine `ApiClaims` JWT, then use it normally.

**Key principle**: Zero changes to existing handlers. The entire API layer continues to operate on `ApiClaims` — SSO just becomes another way to obtain one.

## Motivation

- Verse owners want to invite collaborators who already have corporate/social identities
- IoT deployments in enterprises need SSO integration for compliance
- MCP AI agents running in corporate environments authenticate via the organization's IdP
- Avoids forcing every external user to coordinate out-of-band API token sharing

## Architecture

```
                     ┌─────────────────────┐
                     │  External IdP        │
                     │  (Okta, Google, etc.) │
                     └─────────┬───────────┘
                               │ id_token (OIDC JWT)
                               ▼
┌──────────────────────────────────────────────────────────┐
│  POST /api/v1/auth/exchange                              │
│  { "provider": "okta", "id_token": "eyJ..." }           │
│                                                          │
│  1. Look up provider config from DB (issuer, client_id)  │
│  2. Fetch JWKS from issuer's discovery endpoint          │
│  3. Verify id_token (signature, exp, aud, iss)           │
│  4. Extract identity (sub, email, groups)                │
│  5. Look up identity mapping → (scope, max_role)         │
│  6. Mint FractalEngine ApiClaims JWT                     │
│  7. Return { "token": "eyJ...", "expires_in": 3600 }    │
└──────────────────────────────────────────────────────────┘
                               │
                               ▼ FractalEngine JWT
                     ┌─────────────────────┐
                     │  Existing API layer  │
                     │  (REST, MCP, WS)     │
                     │  No changes needed   │
                     └─────────────────────┘
```

## Supported Providers

Any provider that implements OpenID Connect Discovery:

| Provider | Issuer URL Pattern | Notes |
|----------|-------------------|-------|
| Okta | `https://{domain}.okta.com` | Standard OIDC |
| Authentik | `https://{domain}/application/o/{app}/` | Self-hosted |
| Google | `https://accounts.google.com` | Standard OIDC |
| LinkedIn | `https://www.linkedin.com/oauth` | OIDC on OAuth2 |
| Azure AD / Entra | `https://login.microsoftonline.com/{tenant}/v2.0` | Multi-tenant |
| Auth0 | `https://{domain}.auth0.com/` | Standard OIDC |
| Keycloak | `https://{host}/realms/{realm}` | Self-hosted |
| Custom | Any URL with `/.well-known/openid-configuration` | OIDC-compliant |

## Data Model

### New SurrealDB Tables

```sql
-- Provider configuration (one per trusted IdP per verse)
DEFINE TABLE sso_provider SCHEMAFULL;
DEFINE FIELD provider_id ON sso_provider TYPE string;
DEFINE FIELD verse_id ON sso_provider TYPE string;
DEFINE FIELD display_name ON sso_provider TYPE string;
DEFINE FIELD issuer_url ON sso_provider TYPE string;
DEFINE FIELD client_id ON sso_provider TYPE string;
DEFINE FIELD allowed_domains ON sso_provider TYPE option<array<string>>;
DEFINE FIELD default_role ON sso_provider TYPE string DEFAULT 'viewer';
DEFINE FIELD default_scope ON sso_provider TYPE string;
DEFINE FIELD enabled ON sso_provider TYPE bool DEFAULT true;
DEFINE FIELD created_at ON sso_provider TYPE string;
DEFINE INDEX idx_sso_provider_id ON sso_provider FIELDS provider_id UNIQUE;
DEFINE INDEX idx_sso_provider_verse ON sso_provider FIELDS verse_id;

-- Identity mapping (optional overrides per external user)
DEFINE TABLE sso_identity_map SCHEMAFULL;
DEFINE FIELD provider_id ON sso_identity_map TYPE string;
DEFINE FIELD external_sub ON sso_identity_map TYPE string;
DEFINE FIELD external_email ON sso_identity_map TYPE option<string>;
DEFINE FIELD scope ON sso_identity_map TYPE string;
DEFINE FIELD max_role ON sso_identity_map TYPE string;
DEFINE FIELD display_name ON sso_identity_map TYPE option<string>;
DEFINE FIELD created_at ON sso_identity_map TYPE string;
DEFINE INDEX idx_sso_identity ON sso_identity_map FIELDS provider_id, external_sub UNIQUE;
```

### Provider Resolution Logic

```
1. Client sends { provider: "okta", id_token: "..." }
2. Look up sso_provider WHERE display_name = "okta" AND enabled = true
3. Verify id_token against issuer_url's JWKS
4. Extract: sub, email, groups (if present)
5. Check allowed_domains (if set, email domain must match)
6. Look up sso_identity_map WHERE provider_id AND external_sub
   - If found: use mapped scope + max_role
   - If not found: use provider's default_scope + default_role
7. Mint ApiClaims { sub: "sso:{provider}:{external_sub}", scope, max_role, ... }
```

The `sub` field uses a `sso:` prefix to distinguish SSO-originated tokens from native `did:key:` tokens. Downstream RBAC resolution works the same — the scope/role system is identity-agnostic.

## Phases

### Phase 1: Token Exchange Endpoint + Provider Config (Core)

**Files**: `fe-api/src/sso.rs` (new), `fe-api/src/server.rs`, `fe-database/src/sso_store.rs` (new)

- New `POST /api/v1/auth/exchange` endpoint (public, no auth required)
- `SsoProviderRecord` struct + CRUD in `sso_store.rs`
- OIDC discovery: fetch `/.well-known/openid-configuration` → `jwks_uri` → cache JWKS
- Token verification: validate signature, `iss`, `aud`, `exp`
- Identity extraction: `sub`, `email`, `groups` claims
- JWKS cache: in-memory with 1-hour TTL (avoids per-request HTTP fetch)
- Mint `ApiClaims` with `sub: "sso:{provider_id}:{external_sub}"`
- Default scope and role from provider config

**New dependency**: `openidconnect` crate (handles OIDC discovery, JWKS, token verification)

### Phase 2: Identity Mapping + Domain Restrictions

**Files**: `fe-database/src/sso_store.rs`, `fe-api/src/sso.rs`

- `SsoIdentityMapRecord` struct + CRUD
- Per-user scope/role overrides (lookup by `provider_id + external_sub`)
- Domain allowlist enforcement (reject emails outside allowed domains)
- Group claim mapping (OIDC `groups` claim → role escalation rules)
- Audit log: record each SSO exchange in `op_log` for security audit trail

### Phase 3: Admin UI — Provider Management in Egui

**Files**: `fe-ui/src/dialogs.rs` (Access tab extension), `fe-ui/src/plugin.rs`

The verse owner's desktop app gets a full SSO provider management panel in the entity settings dialog.

#### 3a: SSO Providers Section in Access Tab

New section in the existing Access tab (below "Generate Invite Link"):

```
┌─────────────────────────────────────────────────┐
│  Petal Settings — Genesis Petal           ✕     │
│                                                 │
│  General   Access   API                         │
│  ───────────────────────────────                │
│                                                 │
│  SSO Providers                                  │
│  ─────────────                                  │
│  Configure external identity providers for      │
│  this verse. Users can log in via these          │
│  providers to access the API.                   │
│                                                 │
│  ┌─ Okta (corp.okta.com) ──── [Enabled ▼] ──┐  │
│  │  Role: editor  │  Domains: @company.com   │  │
│  │  2 identity overrides        [Edit] [Del] │  │
│  └───────────────────────────────────────────┘  │
│  ┌─ Google ──────────────────── [Enabled ▼] ──┐ │
│  │  Role: viewer  │  Domains: any              │ │
│  │  0 identity overrides        [Edit] [Del]   │ │
│  └─────────────────────────────────────────────┘│
│                                                 │
│  [+ Add SSO Provider]                           │
│                                                 │
└─────────────────────────────────────────────────┘
```

- Provider list with enable/disable dropdown, role badge, domain filter display
- Delete with confirmation
- Edit opens a sub-dialog (Phase 3b)

#### 3b: Add/Edit Provider Dialog

```
┌─────────────────────────────────────────────────┐
│  Add SSO Provider                        ✕      │
│                                                 │
│  Display Name:   [Okta Corporate          ]     │
│  Issuer URL:     [https://corp.okta.com   ]     │
│  Client ID:      [0oa1b2c3d4e5f6g7h8     ]     │
│                                                 │
│  Default Role:   [editor    ▼]                  │
│  Allowed Domains (comma-separated):             │
│  [@company.com, @subsidiary.com           ]     │
│  (leave blank to allow all domains)             │
│                                                 │
│  [Test Connection]   [Save]   [Cancel]          │
│                                                 │
│  ✓ Connection OK — OIDC discovery succeeded     │
│    Issuer: https://corp.okta.com                │
│    JWKS URI found, 2 signing keys               │
│                                                 │
└─────────────────────────────────────────────────┘
```

- "Test Connection" fetches `{issuer_url}/.well-known/openid-configuration` and reports success/failure
- Validation: issuer URL must be HTTPS, client ID must not be empty

#### 3c: Identity Mapping Table

When the user clicks "Edit" on a provider, show the identity overrides:

```
┌─────────────────────────────────────────────────┐
│  Identity Mappings — Okta Corporate      ✕      │
│                                                 │
│  Override scope/role for specific users.        │
│  Users not listed get the provider default.     │
│                                                 │
│  Email/Sub          │ Scope       │ Role    │   │
│  ────────────────────────────────────────────   │
│  alice@company.com  │ VERSE#v1    │ manager │ ✕ │
│  bob@company.com    │ FRACTAL#f1  │ editor  │ ✕ │
│                                                 │
│  [+ Add Mapping]                                │
│                                                 │
│  Email/Sub: [______________]                    │
│  Scope:     [VERSE#v1     ▼]   Role: [editor▼] │
│  [Add]                                          │
│                                                 │
└─────────────────────────────────────────────────┘
```

**New ActiveDialog variants**:
```rust
AddSsoProvider {
    verse_id: String,
    name_buf: String,
    issuer_url_buf: String,
    client_id_buf: String,
    default_role_buf: String,
    allowed_domains_buf: String,
    test_result: Option<String>,
}
EditSsoIdentityMaps {
    provider_id: String,
    provider_name: String,
    mappings: Vec<SsoIdentityMapEntry>,
    new_sub_buf: String,
    new_scope_buf: String,
    new_role_buf: String,
}
```

### Phase 4: Login Portal — Server-Rendered Login Page

**Files**: `fe-api/src/login_page.rs` (new), `fe-api/src/server.rs`

This is the external-user-facing UI. When someone visits the API server in a browser, they see a login page listing available SSO providers.

#### Login Page Flow

```
Browser → GET /login
    ↓
┌─────────────────────────────────────────────────┐
│                                                 │
│            ╔═══════════════════╗                 │
│            ║  FractalEngine    ║                 │
│            ║  Verse: "HQ"     ║                 │
│            ╚═══════════════════╝                 │
│                                                 │
│  Sign in to access this verse's API:            │
│                                                 │
│  ┌────────────────────────────────────┐         │
│  │  🔵  Continue with Okta            │         │
│  └────────────────────────────────────┘         │
│  ┌────────────────────────────────────┐         │
│  │  🔴  Continue with Google          │         │
│  └────────────────────────────────────┘         │
│  ┌────────────────────────────────────┐         │
│  │  🟣  Continue with Authentik       │         │
│  └────────────────────────────────────┘         │
│                                                 │
│  ──────────── or ─────────────                  │
│                                                 │
│  API Token: [________________________________]  │
│  [Authenticate]                                 │
│                                                 │
│  ─────────────────────────────────────          │
│  API Docs: /api/v1/health                       │
│  MCP: POST /mcp                                 │
│  WebSocket: /ws                                 │
│                                                 │
└─────────────────────────────────────────────────┘
```

- Served as inline HTML from `fe-api` (no static file dependency, no JS framework)
- Lists all enabled SSO providers for the verse
- Each provider button links to `GET /api/v1/auth/sso/{provider}/authorize`
- Also accepts a raw API token (for programmatic access)
- After successful SSO login, redirects to a success page showing the minted JWT (one-time display, copy button)

#### Implementation

The login page is a single Rust function that generates HTML:

```rust
// fe-api/src/login_page.rs
pub async fn login_page(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<LoginParams>,
) -> impl IntoResponse {
    // Fetch enabled providers for the verse (or all if no verse specified)
    let providers = fetch_enabled_providers(&state, params.verse_id.as_deref()).await;

    Html(render_login_html(&providers, params.verse_id.as_deref()))
}

fn render_login_html(providers: &[SsoProviderInfo], verse_id: Option<&str>) -> String {
    // Minimal, clean HTML with inline CSS — no JS dependencies
    // Provider buttons, API token input, endpoint reference
}
```

#### Success Page

After OAuth2 callback completes:

```
┌─────────────────────────────────────────────────┐
│                                                 │
│  ✓ Authenticated as alice@company.com           │
│                                                 │
│  Your API token (copy now — shown once):        │
│  ┌─────────────────────────────────────────┐    │
│  │ eyJhbGciOiJFZERTQSIsInR5cCI6IkpXVCJ9.. │    │
│  │ ............                             │    │
│  └─────────────────────────────────────────┘    │
│  [📋 Copy Token]                                │
│                                                 │
│  Scope: VERSE#v1          Role: editor          │
│  Expires: 2026-05-29                            │
│                                                 │
│  Use this token with:                           │
│  curl -H "Authorization: Bearer <token>" \      │
│       http://127.0.0.1:8765/api/v1/hierarchy    │
│                                                 │
└─────────────────────────────────────────────────┘
```

### Phase 5: OAuth2 Authorization Code Flow (Backend)

**Files**: `fe-api/src/sso.rs`, `fe-api/src/server.rs`

- `GET /api/v1/auth/sso/{provider}/authorize` → redirect to IdP with PKCE
- `GET /api/v1/auth/sso/{provider}/callback` → handle code exchange
- Exchange authorization code for `id_token` via provider's token endpoint
- Verify `id_token`, resolve identity, mint FractalEngine JWT
- Redirect to success page with token in fragment (not query string for security)
- PKCE support for public clients (SPAs, CLI tools)
- CSRF protection via `state` parameter stored in short-lived cookie

### Phase 6: WebSocket + MCP SSO Auth

**Files**: `fe-api/src/ws.rs`, `fe-api/src/mcp.rs`

- WebSocket handshake accepts SSO id_tokens directly (in addition to FractalEngine JWTs)
- MCP initialize handshake supports SSO token exchange
- Token refresh flow: when FractalEngine JWT expires, client can re-exchange without full SSO re-auth (if SSO session is still valid)

## Security Considerations

- **No client_secret storage**: Only `client_id` is stored. The token exchange endpoint verifies id_tokens that the client already possesses — no authorization code flow in Phase 1 (that's Phase 5). This means no secrets to leak.
- **JWKS caching**: Cache with TTL to prevent per-request latency and rate limiting by the IdP. Invalidate on signature verification failure (key rotation).
- **Domain restriction**: `allowed_domains` prevents anyone with a Google account from accessing a verse — only `@company.com` emails are accepted.
- **Scope ceiling**: SSO-originated tokens cannot exceed the provider's `default_scope`. Individual identity mappings can narrow further but never widen beyond what the verse owner configured.
- **Token lifetime**: FractalEngine JWTs minted from SSO exchange use the same 30-day max TTL as native API tokens. The `jti` is tracked for revocation.
- **Audit trail**: Every SSO exchange is logged with external sub, provider, granted scope/role, and timestamp.
- **No SSO token forwarding**: External id_tokens are verified at the boundary and discarded. Only FractalEngine JWTs propagate internally.

## Dependencies

- **Depends on**: Realtime API Gateway (complete), API Auth Enforcement (complete)
- **Blocks**: nothing (additive feature)
- **New crate dependency**: `openidconnect` (OIDC discovery, JWKS, token verification)

## Estimated Scope

| Phase | New Code | Modified Code | Effort |
|-------|----------|---------------|--------|
| P1: Token exchange + OIDC | ~300 lines | ~20 lines (server.rs routes) | Medium |
| P2: Identity mapping + domains | ~150 lines | ~30 lines | Small |
| P3: Admin UI (egui dialogs) | ~400 lines | ~80 lines (dialogs.rs, plugin.rs) | Medium |
| P4: Login portal (HTML page) | ~350 lines | ~20 lines (server.rs routes) | Medium |
| P5: OAuth2 code flow (backend) | ~250 lines | ~20 lines | Medium |
| P6: WS + MCP SSO auth | ~80 lines | ~40 lines | Small |
| **Total** | **~1530 lines** | **~210 lines** | |

## DbCommand/DbResult Additions

```rust
// New DbCommand variants
CreateSsoProvider { verse_id, display_name, issuer_url, client_id, default_role, default_scope, allowed_domains }
UpdateSsoProvider { provider_id, enabled, default_role, allowed_domains }
DeleteSsoProvider { provider_id }
ListSsoProviders { verse_id }
CreateSsoIdentityMap { provider_id, external_sub, external_email, scope, max_role }
DeleteSsoIdentityMap { provider_id, external_sub }
ListSsoIdentityMaps { provider_id }

// New DbResult variants
SsoProviderCreated { provider_id, display_name }
SsoProviderUpdated { provider_id }
SsoProviderDeleted { provider_id }
SsoProvidersListed { providers: Vec<SsoProviderInfo> }
SsoIdentityMapCreated { provider_id, external_sub }
SsoIdentityMapDeleted { provider_id, external_sub }
SsoIdentityMapsListed { mappings: Vec<SsoIdentityMapInfo> }
```

## Test Plan

- Unit tests: OIDC token verification with mock JWKS
- Unit tests: provider config CRUD roundtrip
- Unit tests: identity mapping resolution (default vs override)
- Unit tests: domain restriction enforcement
- Unit tests: scope ceiling enforcement
- Integration test: full exchange flow with mock OIDC server
- Integration test: multiple providers on same verse
- Integration test: revocation of SSO-originated tokens
- Edge case: expired id_token rejected
- Edge case: unknown provider rejected
- Edge case: disabled provider rejected
- Edge case: email domain mismatch rejected
