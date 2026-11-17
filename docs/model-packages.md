# Publishing 3D Model Packages

Model packages distribute reusable `.glb` assets with verifiable publisher signatures, licence metadata, and preview thumbnails as sealed `.hexon` archives (`hexon_type = "model"`).

## 1. Folder Layout

Prepare one folder per model release:

```text
my-infra-kit/
  library.toml
  pylon.glb
  pylon.png
  transformer.glb
  transformer.png
```

All paths referenced in `library.toml` must stay inside the folder (no symlinks, parent-traversal segments, or unlisted `.glb` files).

### Thumbnail Requirements
- Format: PNG signature verified before full allocation.
- Limits: at most `256 KiB` uncompressed (`262144` bytes) and `512 x 512` pixels.

### GLB Requirements
- Valid binary glTF 2.0 container (`glTF`, version `2`, JSON chunk first, well-formed BIN chunk bounds).
- Embedded buffers only — external `uri` references are rejected at pack and install time.

## 2. `library.toml` Format

```toml
id = "acme.infra-kit"
version = "1.0.0"
name = "Acme Infrastructure Kit"
description = "Substation and transmission line props."
tags = ["infrastructure", "utility", "energy"]

[license]
type = "free"          # "free" or "attribution" (never omitted for model packs)
spdx = "CC0-1.0"
# attribution = "Required non-empty visible credit text when type = \"attribution\""

[[model]]
file = "pylon.glb"
thumbnail = "pylon.png"
label = "Lattice Pylon"
tags = ["grid", "tower"]
auto_scale = false

[[model]]
file = "transformer.glb"
thumbnail = "transformer.png"
label = "Pad-Mounted Transformer"
tags = ["substation"]
```

- **Licences**:
  - `free`: public domain or CC0 content (`spdx = "CC0-1.0"`).
  - `attribution`: requires a non-empty `attribution` string with visible characters; shown on Browse cards, Detail views, and Credits.
  - `paid` licences are not supported for standalone model packs and are refused at install.

## 3. Signing Keys & Packing

Generate an Ed25519 publisher keypair if you do not have one:

```bash
fe-ext keygen --out publisher.key
```

Seal and sign the folder into `<id>-<version>.hexon`:

```bash
fe-ext model pack ./my-infra-kit --key publisher.key --out ./dist/
```

`fe-ext model pack` validates `library.toml`, verifies every GLB and PNG thumbnail against format limits, embeds the preview members under `preview/<entry>.png`, seals all members under canonical manifest integrity hashes, signs with your Ed25519 key, and re-verifies the finished `.hexon` archive before writing it atomically.

Verify the finished file independently:

```bash
fe-ext inspect ./dist/acme.infra-kit-1.0.0.hexon
```

## 4. Submitting to the Registry

Outside publishers do not need a registry upload token:

1. Pack and sign the `.hexon` file with your own key so your `publisher_did` remains in the signed manifest.
2. Submit the signed `.hexon` file (plus preview info) via the operator web form.
3. The registry operator uploads the signed archive via `POST /api/v1/hexons/publish` with the publish bearer token (`HEXA_PUBLISH_TOKEN` / `HEXON_TOKEN`); publisher ownership (`publisher_did` continuity on future versions) stays bound to your signing key.
