# General Code Style Principles

This document outlines general coding principles that apply across all modules in FractalEngine.

## Readability

- Code should be easy to read and understand by humans.
- Avoid overly clever or obscure constructs.
- Prefer explicit over implicit — make intent visible in the code itself.

## Consistency

- Follow existing patterns in the codebase.
- Maintain consistent formatting, naming, and structure.
- When in doubt, match the surrounding code.

## Simplicity

- Prefer simple solutions over complex ones.
- Break down complex problems into smaller, manageable parts.
- The right amount of abstraction is the minimum needed for the current task.

## Maintainability

- Write code that is easy to modify and extend.
- Minimize dependencies and coupling between modules.
- Prefer composition over inheritance.

## Documentation

- Document *why* something is done, not just *what*.
- Keep documentation up-to-date with code changes.
- Every public API surface must have a doc comment.

## Safety & Security (Priority Rules for FractalEngine)

These rules are non-negotiable and take precedence over all other style guidance.

### Network Safety
- **No unsigned messages accepted.** All gossip payloads must carry an ed25519 signature. Reject unsigned or unverifiable messages at the ingest boundary — never pass them into application logic.
- **No localhost or RFC 1918 URLs in WebView.** Block `127.0.0.1`, `localhost`, `10.x.x.x`, `172.16.x.x–172.31.x.x`, and `192.168.x.x` unconditionally in the WebView navigation handler.
- **No raw eval.** The JS-to-Rust IPC bridge is a typed command enum. No `eval()`, `Function()`, or dynamic code execution across the WebView boundary.
- **Rate-limit all peer inputs.** Every inbound channel from a peer has a configurable cap. Drop the oldest messages on overflow — never block the render loop waiting for a slow peer.

### RBAC Transparency
- **RBAC is enforced at the database layer only.** Bevy systems must never implement permission checks. If a system receives data from SurrealDB, it is already authorised. Do not add runtime permission checks in ECS systems.
- **Every role assignment and revocation is logged.** Write to the op-log before applying the change. The log entry must exist even if the subsequent write fails.
- **Revocations propagate immediately.** A revoked session must be broadcast via iroh-gossip before the local SessionCache is updated. Order: sign revocation → broadcast → flush cache.

### Cryptographic Transparency
- **Use `verify_strict()` not `verify()`** from ed25519-dalek. Always.
- **Never expose raw private key material** in logs, error messages, or UI surfaces.
- **JWT `sub` field must always be `did:key:<multibase_pub>`.** A JWT without a DID-compatible subject is invalid.

### Failure Transparency
- **Log all security-relevant events** to the Node's internal log: peer connect/disconnect, JWT issue/verify/reject, role assign/revoke, revocation broadcast, WebView navigation blocked.
- **Never silently swallow errors** in security-critical paths. If a signature verification fails, log it with the peer's public key and the reason. Do not just return `false`.
