# Canonical-log specification notes

`operation-envelope.md` is the normative source for the v1 operation artifact.
The JSON fixture and its dependency-free Node integrity check are an
input/output contract for future implementations, not a production codec. The
oracle (`operation-envelope-v1.test.mjs`) is kept in lockstep with the
normative profile: whenever a rule in the spec tightens, the oracle's decoder
MUST enforce the same bound, never a laxer one. The golden vectors in
`operation-envelope-v1.json` are frozen — if a vector test fails, the codec is
wrong, never the fixture. Run
`node --test docs/spec/canonical-log/operation-envelope-v1.test.mjs` when
changing either fixture. Keep protocol rationale in the specification's
**Design notes** section (including its **Errata** subsection for
owner-ratified spec changes) and keep normative rules numbered and terse.

`capabilities-and-revocation.md` is the normative SPEC-3 source for canonical
capability chains, scope epochs, revocation, and blinded discovery labels. Its
listed conformance tests are acceptance requirements for the implementation. The
AEAD and key-distribution suite is fixed by D-CL17 (XChaCha20-Poly1305 with an
X25519 HPKE-style scope-key wrap). Nothing here authorizes network wiring.

This directory deliberately contains no networking, materializer, or key
distribution implementation. Changes to cryptographic suites, capability
semantics, segment layout, and branch retention belong to their respective
SPEC documents and require owner ratification.
