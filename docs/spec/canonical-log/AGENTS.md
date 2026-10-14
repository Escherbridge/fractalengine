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

`observation-plane.md` is the SPEC-10 source for the observation plane
(D-CL26): the kind-5 observation batch, the columnar observation hexon,
hydration, the `preview_kind` registry, observation source identity, and the
`iot_reading` graduation path. The owner pre-approved it on 2026-10-13 with
its recommended defaults (D-CL30..34, RUNBOOK §3 R7). Post-review is pending,
so a change from that review lands as a dated Errata entry, never as a silent
rewrite. Its wire numbers are provisional (D-CL34) and are registered with the
rest in `fe-canonical-log/src/AGENTS.md` §Provisional wire numbering, never in
a second table. It states the R5 ingestion model (generic asset,
device-payload records, format descriptors plus sandboxed decoders) as one
ingestion story. The hot-plane shapes it relies on are normative in the owning
crates' `AGENTS.md` files, not here. Its §2.3 current-code table is a snapshot
at `0848237`: re-verify a citation before building on it.

D-CL28 gates G3 and G6 landed as errata on 2026-10-13: SPEC-6 §3.4
(bulk-artifact lane) and SPEC-4 §7 rule 5 (observation input set). Like
G1/G2/G4/G5 they are owner-ratified under D-CL28 and carry dated Errata
entries.

This directory deliberately contains no networking, materializer, or key
distribution implementation. Changes to cryptographic suites, capability
semantics, segment layout, branch retention, and the observation plane belong
to their respective SPEC documents and require owner ratification.
