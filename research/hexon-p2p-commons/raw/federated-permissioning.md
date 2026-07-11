# RAW: Federated / Decentralized Permissioning — UCAN, ACL-in-CRDT, Matrix state-res

Fetch date: 2026-07-11
Intent: hardest limits of permissioning a federated commons; concrete failure modes.

## UCAN (User Controlled Authorization Network) — capability model + revocation limits
Sources: https://ucan.xyz/ , https://ucan.xyz/revocation/ , https://ucan.xyz/specification/

- UCANs = decentralized, capability-based auth tokens: public-key-verifiable, delegable,
  expressive, extensible via **chained certificates + DIDs**. Verifiable **without contacting the
  issuer** (offline-first). Directly maps to hexon's "policy-pattern auth" + did:key identity.
- Delegation: chain + combine UCANs, custom constraints, **time limits**, precise scope.
- REVOCATION — the hard part (verbatim caveats):
  * "Revocation SHOULD be considered the **last line of defense** against abuse." Prefer **expiry**
    and **reduced capability scope** proactively.
  * Revocations are **eventually consistent, gossip-distributed** block lists keyed by the delegation's
    canonical CID. "MAY operate in fully eventually consistent contexts."
  * "Revocations MUST be **immutable and irreversible**." A mistaken revocation → reissue a NEW
    delegation (you cannot un-revoke). Monotonic.
  * "**No temporal guarantees**": "unable to guarantee delivery in a certain time bound"; out-of-order
    delivery is typical; **malicious actors may strategically DELAY revealing capabilities.**
  * Trades guaranteed revocation enforcement for offline-first usability (unlike E-lang ocaps with
    active network proxies / caretaker pattern).
- HARD LIMIT #1 (revocation): In an offline-first capability system you CANNOT bound the time to
  revoke. A holder who is offline (or adversarially withholding) keeps effective access until they
  sync the revocation. This is unavoidable, matches the p2p-mycelium finding "retain what already
  synced." Mitigation = short-lived caps + proactive re-delegation, NOT reliance on revocation.

## ACL-in-CRDT — Keyhive (Ink & Switch) — the canonical hard problem
Sources: https://www.inkandswitch.com/keyhive/notebook/ , /02/ (BeeKEM), Kleppmann DCGKA

- Core difficulty: "access control must **travel with the data itself** and work **without a central
  guard**." No single source of truth about "who can do what at any given time."
- THE TWO CANONICAL FAILURE MODES:
  1. **Dual-admin mutual revocation**: two admins concurrently revoke each other — which wins?
     No central authority to break the tie.
  2. **Revoked actor's concurrent write** ("back-dating"): a user is revoked but concurrently makes
     a write; because causal order differs across replicas, their op might survive on some peers.
     "The system must prevent back-dating — where a revoked user's operations survive because they
     were causally ordered differently across replicas."
- Keyhive's resolution: rejects consensus (needs central authority); accepts Automerge **causal
  consistency**; **remove ops blank nodes in the group key tree, and remove paths are blanked AFTER
  all other concurrent ops are merged** — so a revoked user cannot exploit concurrent timing.
- BeeKEM: Continuous Group Key Agreement for local-first E2E groups; **~thousands of members**,
  **log performance common case / linear worst case**, forward secrecy + post-compromise security.
  Kleppmann's DCGKA is decentralized (no trusted server) but **linear** not log.
- STATUS (as of 2025): **pre-alpha**, code on GitHub (beelay-core, keyhive_core, keyhive_wasm),
  explicit "⚠️ DO NOT use in production ⚠️", no security audit; next phase = security review.
  => HARD LIMIT #2 (governance): the STATE OF THE ART for decentralized ACLs is pre-alpha,
  unaudited research. There is NO production-proven library for "convergent capabilities" today
  (July 2026). A federated commons that needs revocable roles is building on research, not
  commodity infrastructure. Confidence HIGH.

## p2panda access control — "strong removal" resolver (a concrete, shipping-ish design)
Source: https://p2panda.org/2025/07/28/access-control.html

- Same problem framing: peers receive group updates in arbitrary order, "potentially years of delay
  between action and notification"; only a **partial order** is available.
- Named failure case (verbatim): Duck demotes Penguin (manager); before learning it, Penguin
  promotes Parrot to manager, Parrot promotes others. Whose actions survive?
- Solution = **causal DAG of cryptographically-signed group control operations** (each has group id,
  action, `previous`, `dependencies`) + a pluggable **Resolver** trait. Default = **"strong removal"**:
  * removal/demotion of a manager **invalidates that member's concurrent actions**;
  * **mutual removals invalidate BOTH parties' concurrent work**;
  * dependent operations transitively invalidated;
  * concurrency "bubbles" found via **depth-first search**, then rules filter invalid ops.
- p2panda concedes the ruleset "isn't universal" → different trust contexts need custom resolvers.
- => This is the most directly transplantable design for hexon's policy-pattern auth: model role
  grants/revokes as signed ops in the hexon op-log DAG, and apply a strong-removal resolver at
  materialization time (invalidate concurrent ops by a revoked author). It gives DETERMINISTIC,
  convergent auth over a partial order WITHOUT consensus. Confidence HIGH this is the right pattern.

## Matrix state resolution — CAUTIONARY TALE (state-reset CVEs, Project Hydra 2025)
Sources: https://matrix.org/blog/2025/07/security-predisclosure/ ,
https://matrix.org/blog/2025/08/project-hydra-improving-state-res/ ,
https://github.com/matrix-org/synapse/issues/15987 ("Another case of state reset in State Res v2"),
https://www.csoonline.com/article/4040136/ (CSO Online)

- Matrix uses **State Resolution v2** to merge divergent room state across federated homeservers —
  the closest large-scale analog to "converge auth/membership state across a federation."
- 2025: **two high-severity protocol vulns** (CVE-2025-49090 + one unallocated). Coordinated
  cross-implementation security release Jul 22–Aug 11 2025. "Project Hydra" launched to fix state res.
- ATTACK: "a malicious homeserver operator corrupting the chatroom's state by **resetting it to a
  prior value (e.g. reverting access control or room membership to an earlier configuration)**."
  I.e. **state reset** can **roll back permission/membership changes** — a revoked user or removed
  admin could be silently restored by replaying/forcing older state.
- Scope: only exploitable by **servers that previously participated** in the room; not by arbitrary
  network attackers. Fix requires **room version 12** upgrade.
- "state reset" (synapse#15987) is a RECURRING class, not a one-off — the algorithm can produce
  "unexpected results" merging concurrent state. Matrix invested 6+ months + a named project.
- => CAUTIONARY LESSON #3: getting decentralized **auth-state convergence** right is a decade-long,
  CVE-generating problem even for a well-resourced, widely-deployed protocol. "State reset that
  reverts access control" is EXACTLY the failure hexon's op-log auth must design against — the
  strong-removal resolver + immutable-monotonic revocation (never un-revoke) is the guard. A naive
  LWW over auth fields would REINTRODUCE the Matrix state-reset bug (older grant wins by timestamp
  skew → revoked user restored). Auth MUST NOT be plain LWW.
