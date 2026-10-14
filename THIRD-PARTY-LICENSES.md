# Third-Party Licenses

FractalEngine is licensed under Apache-2.0 (see `LICENSE-APACHE`).
That license covers FractalEngine's own source code only.
The binaries additionally embed third-party dependencies under their own
licenses. Most are permissively licensed (MIT / Apache-2.0 / BSD / ISC /
Zlib / Unicode / MPL-2.0); the one notable exception is called out below.

## Allowed licences (`deny.toml`)

`cargo deny check licenses` (run warn-only in CI) enforces this allow-list:

- **Core set:** MIT, Apache-2.0 (incl. `WITH LLVM-exception`), BSD-2-Clause,
  BSD-3-Clause, ISC, Zlib, Unicode-3.0, Unicode-DFS-2016, MPL-2.0.
- **Outlier licences, allowed 2026-10-12 (OSS-1.3 decision):**
  - Allowed for any crate: Unlicense, BSL-1.0 (Boost; not BUSL), CC0-1.0,
    MIT-0, CDLA-Permissive-2.0 (the Mozilla CA bundle data in `webpki-roots`
    and `webpki-root-certs`).
  - Font licences, allowed only for `epaint_default_fonts` (egui's bundled
    fonts): OFL-1.1 and Ubuntu-font-1.0.
- **Per-crate exception:** BUSL-1.1 for the `surrealdb*` crates only (below).

## SurrealDB — Business Source License 1.1

The `surrealdb` and `surrealdb-core` crates (version 3.0.5 at time of
writing), together with the engine's split-out `surrealdb-types`,
`surrealdb-types-derive` (3.0.5) and `surrealdb-protocol` (0.8.3) crates, are licensed under the **Business Source License 1.1 (BUSL-1.1)**,
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

## Sample 3D models (`fractalengine/assets/samples/`)

The source tree ships two small dev/test glTF models, both from the
[Khronos glTF-Sample-Assets](https://github.com/KhronosGroup/glTF-Sample-Assets)
repository. Their licences were checked against upstream on 2026-10-12. See
`fractalengine/assets/samples/README.md` for details.

- **`cameras.glb`**: "Cameras" by Marco Hutter (<https://github.com/javagl/>),
  dedicated to the public domain under
  [CC0 1.0 Universal](https://creativecommons.org/publicdomain/zero/1.0/legalcode).
  Repackaged from `.gltf` + `.bin` into a single `.glb`.
- **`info_panel.glb`**: "Box" © 2017 Cesium, licensed under
  [Creative Commons Attribution 4.0 International (CC BY 4.0)](https://creativecommons.org/licenses/by/4.0/legalcode).
  Source: <https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/Box>.
  The file was renamed but not modified.

## Full inventory

The complete third-party license inventory is generated from `Cargo.lock`
rather than maintained by hand:

- `cargo deny check licenses` — enforces the license allowlist in
  `deny.toml` (which carries an explicit BUSL-1.1 exception scoped to the
  `surrealdb*` crates).
- `cargo about generate about.hbs > third-party-inventory.html` — produces
  the full per-crate license text listing (requires
  `cargo install cargo-about`).
