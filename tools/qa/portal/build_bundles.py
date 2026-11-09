#!/usr/bin/env python3
"""Deterministic QA portal bundle builder (plan T5.2); see tools/qa/portal/AGENTS.md section builder."""
import argparse
import glob
import hashlib
import json
import os
import secrets
import subprocess
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
BUNDLES_DIR = os.path.join(HERE, "bundles")
INDEX_PATH = os.path.join(BUNDLES_DIR, "bundles.json")
OUT_DIR = os.path.join(REPO_ROOT, ".qa", "assets", "portal")
STAGE_GLOB = os.path.join(REPO_ROOT, ".qa", "stage", "*")
SEAL_KEY_ENV = "FE_SECRET_HEXON_SEAL_KEY"
SEAL_TIMEOUT_S = 120
ZIP_EPOCH = (1980, 1, 1, 0, 0, 0)  # fixed member timestamp: same sources give the same unsealed bytes
UNSEALED_PUBLISHER = "did:key:z6MkQaPortalUnsealedFixture"  # placeholder; seal_hexon overwrites it
MANIFEST_SCHEMA_VERSION = "1.0.0"
MANIFEST_CREATED = "2026-11-08T00:00:00Z"
GENERATOR = "fe-qa-portal build_bundles.py v1"
ASSET_SCHEMA = "fe-qa-portal-assets/1"
PAD_MEMBER = "web/pad.bin"
EMPTY_CATALOG = b'{"entries": []}'


def load_ladder():
    """The simulator's size ladder, so portal tiers track tools/qa/sim/assets.py."""
    sys.path.insert(0, os.path.join(HERE, "..", "sim"))
    from assets import LADDER  # imported lazily: assets.py needs numpy
    return dict(LADDER)


def read_bytes(path):
    with open(path, "rb") as handle:
        return handle.read()


def web_members(source_dir):
    """Sorted {archive path: bytes} for every file under <source_dir>/web."""
    root = os.path.join(source_dir, "web")
    members = {}
    for folder, _dirs, files in os.walk(root):
        for name in files:
            full = os.path.join(folder, name)
            relative = os.path.relpath(full, source_dir).replace(os.sep, "/")
            members[relative] = read_bytes(full)
    return dict(sorted(members.items()))


def extension_document(package_id, version, portal):
    return {"manifest_version": 2, "id": package_id, "version": version, "kind": "web-bundle", "portal": portal}


def hexon_manifest(package_id, version, title):
    return {
        "schema_version": MANIFEST_SCHEMA_VERSION, "hexon_id": package_id, "hexon_type": "plugin",
        "publisher_did": UNSEALED_PUBLISHER, "version": version, "name": title,
        "created_at": MANIFEST_CREATED, "updated_at": MANIFEST_CREATED, "tags": ["qa", "portal"],
    }


def pack(package_id, version, portal, members):
    """One unsealed plugin-family archive; members written in a fixed order with a fixed timestamp."""
    import io
    buffer = io.BytesIO()
    header = [
        ("manifest.json", json.dumps(hexon_manifest(package_id, version, portal["title"]), indent=2, sort_keys=True).encode()),
        ("entries.json", EMPTY_CATALOG),
        ("plugin/extension.json", json.dumps(extension_document(package_id, version, portal), sort_keys=True).encode()),
    ]
    with zipfile.ZipFile(buffer, "w") as archive:
        for path, data in header + list(members.items()):
            info = zipfile.ZipInfo(path, date_time=ZIP_EPOCH)
            info.compress_type = zipfile.ZIP_STORED if path == PAD_MEMBER else zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, data)
    return buffer.getvalue()


def find_sealer(explicit):
    """The staged seal_hexon.exe, hash-checked against its stage.json (controlled-fixtures precedent)."""
    if explicit:
        return explicit
    candidates = sorted(glob.glob(os.path.join(STAGE_GLOB, "seal_hexon.exe")), key=os.path.getmtime, reverse=True)
    for candidate in candidates:
        stage_json = os.path.join(os.path.dirname(candidate), "stage.json")
        try:
            expected = json.loads(read_bytes(stage_json).decode("utf-8-sig")).get("seal_hexon_sha256", "")
        except (OSError, ValueError):
            continue
        if hashlib.sha256(read_bytes(candidate)).hexdigest().lower() == expected.lower():
            return candidate
    raise SystemExit("no hash-verified staged seal_hexon.exe under .qa/stage; run tools/qa/stage.ps1 or pass --sealer")


def seal(sealer, seed_hex, source_path, sealed_path):
    """Run the production sealer with the run-local seed in the environment only; never on argv or disk."""
    environment = dict(os.environ)
    environment[SEAL_KEY_ENV] = seed_hex
    result = subprocess.run([sealer, source_path, sealed_path], env=environment, capture_output=True,
                            timeout=SEAL_TIMEOUT_S, check=False)
    if result.returncode != 0:
        raise SystemExit("seal_hexon failed for %s: %s" % (os.path.basename(source_path), result.stderr.decode(errors="replace")[:300]))


def blake3_hex(data):
    """BLAKE3 when the optional module exists, else sha256 under a distinct field name (see entry())."""
    try:
        import blake3
        return "blake3", blake3.blake3(data).hexdigest()
    except ImportError:
        return "sha256", hashlib.sha256(data).hexdigest()


def entry(key, path, package_id, version, sealed, journeys, note=None):
    data = read_bytes(path)
    algo, digest = blake3_hex(data)
    record = {"key": key, "path": path, "kind": "portal-bundle", "bytes": len(data), "package_id": package_id,
              "version": version, "sealed": sealed, "journeys": journeys, algo: digest}
    if note:
        record["note"] = note
    return record


def padded_members(base_members, base_archive_size, target_bytes):
    """docs-site members plus one incompressible pad member so the archive lands near `target_bytes`."""
    pad = max(0, target_bytes - base_archive_size)
    seed = ("fe-qa-portal-pad:%d" % target_bytes).encode()
    members = dict(base_members)
    members[PAD_MEMBER] = hashlib.shake_256(seed).digest(pad) if pad else b""
    return dict(sorted(members.items()))


def build_variant(name, spec, variant_key, variant, out_dir, sealer, seed_hex):
    source_dir = os.path.join(BUNDLES_DIR, name)
    portal = json.loads(read_bytes(os.path.join(source_dir, variant["manifest"])).decode("utf-8"))
    version = variant.get("version", spec["version"])
    members = web_members(source_dir)
    for path, text in variant.get("extra_member", {}).items():
        members[path] = text.encode()
    members = dict(sorted(members.items()))
    sealed = variant.get("seal", True)
    stem = name.replace("-", "_") + ("" if variant_key == "signed" else "_" + variant_key)
    source_path = os.path.join(out_dir, stem + ".source.hexon")
    final_path = os.path.join(out_dir, stem + ".hexon")
    with open(source_path, "wb") as handle:
        handle.write(pack(spec["package_id"], version, portal, members))
    if sealed:
        seal(sealer, seed_hex, source_path, final_path)
        os.remove(source_path)
    else:
        os.replace(source_path, final_path)
    return entry(stem, final_path, spec["package_id"], version, sealed, spec.get("journeys", []), variant.get("note"))


def build_ladder(spec, out_dir, sealer, seed_hex):
    """docs-site shape plus the padding member, one archive per tier and one over-cap build."""
    source_dir = os.path.join(BUNDLES_DIR, "docs-site")
    portal = json.loads(read_bytes(os.path.join(source_dir, "portal.json")).decode("utf-8"))
    base_members = web_members(source_dir)
    base_size = len(pack(spec["package_id"], spec["version"], portal, base_members))
    ladder = load_ladder()
    targets = [(tier, ladder[tier]) for tier in spec["tiers"]]
    targets.append((spec["over_cap"]["name"], spec["over_cap"]["size"]))
    entries = []
    for tier, size in targets:
        members = padded_members(base_members, base_size, size)
        source_path = os.path.join(out_dir, "ladder_%s.source.hexon" % tier)
        final_path = os.path.join(out_dir, "ladder_%s.hexon" % tier)
        with open(source_path, "wb") as handle:
            handle.write(pack(spec["package_id"], spec["version"], portal, members))
        seal(sealer, seed_hex, source_path, final_path)
        os.remove(source_path)
        note = "target %d bytes; over the default 32 MiB cap" % size if tier == spec["over_cap"]["name"] else "target %d bytes" % size
        entries.append(entry("ladder_" + tier, final_path, spec["package_id"], spec["version"], True, spec.get("journeys", []), note))
    return entries


def build_all(out_dir, sealer, seed_hex, only):
    os.makedirs(out_dir, exist_ok=True)
    index = json.loads(read_bytes(INDEX_PATH).decode("utf-8"))
    entries = []
    for name, spec in index["bundles"].items():
        if only and name not in only:
            continue
        if spec.get("generated"):
            entries.extend(build_ladder(spec, out_dir, sealer, seed_hex))
            continue
        for variant_key, variant in spec["variants"].items():
            entries.append(build_variant(name, spec, variant_key, variant, out_dir, sealer, seed_hex))
    manifest = {"schema": ASSET_SCHEMA, "generator": GENERATOR, "assets": entries}
    with open(os.path.join(out_dir, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=1)
    return manifest


def main(argv=None):
    parser = argparse.ArgumentParser(description="Build the QA portal fixture bundles and their asset manifest.")
    parser.add_argument("--out", default=OUT_DIR, help="output directory (default .qa/assets/portal)")
    parser.add_argument("--sealer", help="path to seal_hexon.exe (default: the hash-verified staged helper)")
    parser.add_argument("--seed-hex", help="64-hex publisher seed for reproducible runs (default: random, run-local)")
    parser.add_argument("--only", nargs="*", help="bundle names to build (default all)")
    args = parser.parse_args(argv)
    seed_hex = args.seed_hex or secrets.token_hex(32)
    if len(seed_hex) != 64 or any(c not in "0123456789abcdefABCDEF" for c in seed_hex):
        raise SystemExit("--seed-hex must be 64 hex characters")
    manifest = build_all(args.out, find_sealer(args.sealer), seed_hex, set(args.only or []))
    for record in manifest["assets"]:
        print("%-34s %10d bytes  sealed=%s" % (record["key"], record["bytes"], record["sealed"]))
    print(os.path.join(args.out, "manifest.json"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
