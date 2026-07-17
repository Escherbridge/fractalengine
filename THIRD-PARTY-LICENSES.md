# Third-Party Licenses

FractalEngine is licensed under Apache-2.0 (see `LICENSE-APACHE`).
That license covers FractalEngine's own source code only.
The binaries additionally embed third-party dependencies under their own
licenses. Most are permissively licensed (MIT / Apache-2.0 / BSD / ISC /
Zlib / Unicode / MPL-2.0); the one notable exception is called out below.

## SurrealDB — Business Source License 1.1

The `surrealdb` and `surrealdb-core` crates (version 3.0.5 at time of
writing) are licensed under the **Business Source License 1.1 (BUSL-1.1)**,
Licensor SurrealDB Ltd. FractalEngine embeds the SurrealDB storage engine
(`kv-surrealkv` / `kv-mem`) directly into both shipped binaries
(`fractalengine` and `fe-relay`) — it is not an optional or external service.

What BUSL-1.1 means in practice:

- **It is source-available, not OSI-approved open source.** The
  Apache-2.0 license on FractalEngine's own code does not extend
  to the embedded SurrealDB engine.
- **Broad use is permitted, with one carve-out.** BUSL-1.1 grants the right
  to copy, modify, and use the licensed work, limited by an Additional Use
  Grant defined by the Licensor. SurrealDB's grant permits production use
  except for offering a commercial, hosted, competing database service
  (i.e. you may not resell SurrealDB itself as a DBaaS). This restriction
  binds anyone who redistributes FractalEngine binaries.
- **Each release converts to Apache-2.0 on its Change Date.** BUSL licenses
  specify a Change Date (typically four years after release for SurrealDB
  versions) after which that version becomes available under the Change
  License, Apache-2.0.
- **Redistributors must review the license themselves.** The authoritative
  terms are the `LICENSE` file shipped inside the `surrealdb` and
  `surrealdb-core` crates and at <https://github.com/surrealdb/surrealdb>.
  The summary above is informational, not legal advice.

## Full inventory

The complete third-party license inventory is generated from `Cargo.lock`
rather than maintained by hand:

- `cargo deny check licenses` — enforces the license allowlist in
  `deny.toml` (which carries an explicit BUSL-1.1 exception scoped to the
  `surrealdb*` crates).
- `cargo about generate about.hbs > third-party-inventory.html` — produces
  the full per-crate license text listing (requires
  `cargo install cargo-about`).
