# Canonical-log specification notes

`operation-envelope.md` is the normative source for the v1 operation artifact.
The JSON fixture and its dependency-free Node integrity check are an
input/output contract for future implementations, not a production codec. Run
`node --test docs/spec/canonical-log/operation-envelope-v1.test.mjs` when
changing either fixture. Keep protocol rationale in the specification's
**Design notes** section and keep normative rules numbered and terse.

`capabilities-and-revocation.md` is the normative SPEC-3 source for canonical
capability chains, scope epochs, revocation, and blinded discovery labels. Its
listed conformance tests are acceptance requirements for a future implementation;
they do not authorize network wiring or select an AEAD/key-distribution suite.

This directory deliberately contains no networking, materializer, or key
distribution implementation. Changes to cryptographic suites, capability
semantics, segment layout, and branch retention belong to their respective
SPEC documents and require owner ratification.
