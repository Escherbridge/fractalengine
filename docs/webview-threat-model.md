# WebView Threat Model

## SSRF (Server-Side Request Forgery)
- **Attack:** WebView navigates to internal service via URL injection
- **Mitigation:** navigation_handler checks ALL urls via is_url_allowed() before loading. localhost and all RFC 1918 ranges blocked unconditionally.

## XSS via IPC
- **Attack:** Malicious page injects JS to call native IPC with crafted payload
- **Mitigation:** Only typed BrowserCommand enum accepted. No raw eval. No string injection. All IPC messages deserialized strictly via serde.

## Credential Harvesting
- **Attack:** External URL phishing for FractalEngine credentials
- **Mitigation:** Non-dismissible trust bar injected into every WebView page showing domain + "External Website" label. Users cannot remove it.

## Localhost Bypass via DNS Rebinding
- **Attack:** External domain resolves to 127.0.0.1 after DNS TTL expires
- **Mitigation:** Block by resolved IP, not by hostname only. Post-navigation IP check deferred to v2 (known gap).

## Oversized Content
- **Attack:** WebView loading multi-GB external resource to exhaust memory
- **Mitigation:** Browser-level limit deferred to v2. Known gap.
