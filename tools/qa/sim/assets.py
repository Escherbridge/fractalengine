"""Deterministic glTF 2.0 (.glb) generator: buildings, trees, vehicles and a texture-size ladder; see tools/qa/AGENTS.md §sim."""
import argparse
import hashlib
import json
import math
import os
import struct
import sys
import zlib

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
OUT_DIR = os.path.join(REPO_ROOT, ".qa", "assets", "generated")
GENERATOR = "fe-qa-sim assets.py v1"
LADDER = {"tier_100kb": 100_000, "tier_1mb": 1_000_000, "tier_5mb": 5_000_000, "tier_25mb": 25_000_000}
ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER = 34962, 34963
FLOAT, UINT32 = 5126, 5125


# -- PNG (stdlib zlib + numpy pixels) ---------------------------------------------------------------
def png_bytes(pixels):
    """Encode an HxWx3 uint8 array as PNG (filter 0 rows, zlib level 6)."""
    height, width, _channels = pixels.shape
    raw = np.zeros((height, width * 3 + 1), dtype=np.uint8)
    raw[:, 1:] = pixels.reshape(height, width * 3)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw.tobytes(), 6))
            + chunk(b"IEND", b""))


def facade_texture(size, seed, noise=0.0, wall=(176, 164, 150)):
    """Window-grid facade: wall colour, 8x8 window cells (dark glass, some lit), optional grime noise."""
    rng = np.random.default_rng(seed)
    image = np.empty((size, size, 3), dtype=np.float32)
    image[:] = wall
    cells = 8
    cell = size / cells
    yy, xx = np.mgrid[0:size, 0:size]
    in_x = ((xx % cell) > cell * 0.22) & ((xx % cell) < cell * 0.78)
    in_y = ((yy % cell) > cell * 0.18) & ((yy % cell) < cell * 0.70)
    lit = rng.random((cells, cells)) < 0.3
    lit_map = lit[(yy // cell).astype(int).clip(0, cells - 1), (xx // cell).astype(int).clip(0, cells - 1)]
    window = in_x & in_y
    image[window & ~lit_map] = (38, 56, 82)
    image[window & lit_map] = (236, 210, 120)
    if noise > 0:
        image += rng.normal(0.0, 128.0 * noise, image.shape)
    return np.clip(image, 0, 255).astype(np.uint8)


# -- geometry -----------------------------------------------------------------------------------------
class MeshBuilder:
    """Accumulates primitives (one material each) and serializes a single-mesh GLB."""

    def __init__(self, name):
        self.name = name
        self.primitives = []  # (positions, normals, uvs, indices, material)
        self.materials = []
        self.images = []

    def material(self, name, color=(1, 1, 1, 1), image=None, metallic=0.0, roughness=0.85):
        pbr = {"baseColorFactor": list(color), "metallicFactor": metallic, "roughnessFactor": roughness}
        if image is not None:
            self.images.append(image)
            pbr["baseColorTexture"] = {"index": len(self.images) - 1}
        self.materials.append({"name": name, "pbrMetallicRoughness": pbr})
        return len(self.materials) - 1

    def add(self, positions, normals, uvs, indices, material):
        self.primitives.append((np.asarray(positions, np.float32), np.asarray(normals, np.float32),
                                np.asarray(uvs, np.float32), np.asarray(indices, np.uint32), material))

    def box(self, center, size, material, uv_scale=3.0):
        cx, cy, cz = center
        sx, sy, sz = (s / 2 for s in size)
        faces = [((1, 0, 0), [(sx, -sy, sz), (sx, -sy, -sz), (sx, sy, -sz), (sx, sy, sz)], size[2], size[1]),
                 ((-1, 0, 0), [(-sx, -sy, -sz), (-sx, -sy, sz), (-sx, sy, sz), (-sx, sy, -sz)], size[2], size[1]),
                 ((0, 1, 0), [(-sx, sy, sz), (sx, sy, sz), (sx, sy, -sz), (-sx, sy, -sz)], size[0], size[2]),
                 ((0, -1, 0), [(-sx, -sy, -sz), (sx, -sy, -sz), (sx, -sy, sz), (-sx, -sy, sz)], size[0], size[2]),
                 ((0, 0, 1), [(-sx, -sy, sz), (sx, -sy, sz), (sx, sy, sz), (-sx, sy, sz)], size[0], size[1]),
                 ((0, 0, -1), [(sx, -sy, -sz), (-sx, -sy, -sz), (-sx, sy, -sz), (sx, sy, -sz)], size[0], size[1])]
        positions, normals, uvs, indices = [], [], [], []
        for normal, corners, width, height in faces:
            base = len(positions)
            u, v = width / uv_scale, height / uv_scale
            for corner, uv in zip(corners, [(0, v), (u, v), (u, 0), (0, 0)]):
                positions.append((cx + corner[0], cy + corner[1], cz + corner[2]))
                normals.append(normal)
                uvs.append(uv)
            indices += [base, base + 1, base + 2, base, base + 2, base + 3]
        self.add(positions, normals, uvs, indices, material)

    def cylinder(self, center, radius, height, material, segments=16, axis="y", top_radius=None):
        """Cylinder (or cone when top_radius=0) with caps; axis y or x."""
        top_radius = radius if top_radius is None else top_radius
        positions, normals, uvs, indices = [], [], [], []
        slope = (radius - top_radius) / height
        for i in range(segments + 1):
            angle = 2 * math.pi * i / segments
            c, s = math.cos(angle), math.sin(angle)
            length = math.sqrt(1 + slope * slope)
            normal = (c / length, slope / length, s / length)
            positions += [(radius * c, -height / 2, radius * s), (top_radius * c, height / 2, top_radius * s)]
            normals += [normal, normal]
            uvs += [(i / segments, 1), (i / segments, 0)]
        for i in range(segments):
            a = 2 * i
            indices += [a, a + 1, a + 2, a + 1, a + 3, a + 2]
        for y, r, ny in ((-height / 2, radius, -1), (height / 2, top_radius, 1)):
            if r <= 0:
                continue
            center_index = len(positions)
            positions.append((0, y, 0))
            normals.append((0, ny, 0))
            uvs.append((0.5, 0.5))
            for i in range(segments + 1):
                angle = 2 * math.pi * i / segments
                positions.append((r * math.cos(angle), y, r * math.sin(angle)))
                normals.append((0, ny, 0))
                uvs.append((0.5 + 0.5 * math.cos(angle), 0.5 + 0.5 * math.sin(angle)))
            for i in range(segments):
                first, second = center_index + 1 + i, center_index + 2 + i
                indices += [center_index, second, first] if ny > 0 else [center_index, first, second]
        positions, normals = np.asarray(positions, np.float32), np.asarray(normals, np.float32)
        if axis == "x":  # rotate +90 deg about z: (x, y, z) -> (-y, x, z)
            positions = np.stack([-positions[:, 1], positions[:, 0], positions[:, 2]], axis=1)
            normals = np.stack([-normals[:, 1], normals[:, 0], normals[:, 2]], axis=1)
        positions = positions + np.asarray(center, np.float32)
        self.add(positions, normals, uvs, indices, material)

    def glb(self):
        buffer = bytearray()
        views, accessors, primitives = [], [], []

        def push(data, target=None):
            while len(buffer) % 4:
                buffer.append(0)
            view = {"buffer": 0, "byteOffset": len(buffer), "byteLength": len(data)}
            if target:
                view["target"] = target
            buffer.extend(data)
            views.append(view)
            return len(views) - 1

        def accessor(array, kind, target, component):
            view = push(array.tobytes(), target)
            entry = {"bufferView": view, "componentType": component, "count": int(array.shape[0]), "type": kind}
            if kind == "VEC3" and target == ARRAY_BUFFER:
                entry["min"] = [float(v) for v in array.min(axis=0)]
                entry["max"] = [float(v) for v in array.max(axis=0)]
            accessors.append(entry)
            return len(accessors) - 1

        for positions, normals, uvs, indices, material in self.primitives:
            primitives.append({"attributes": {"POSITION": accessor(positions, "VEC3", ARRAY_BUFFER, FLOAT),
                                              "NORMAL": accessor(normals, "VEC3", ARRAY_BUFFER, FLOAT),
                                              "TEXCOORD_0": accessor(uvs, "VEC2", ARRAY_BUFFER, FLOAT)},
                               "indices": accessor(indices, "SCALAR", ELEMENT_ARRAY_BUFFER, UINT32),
                               "material": material, "mode": 4})
        images = [{"bufferView": push(png), "mimeType": "image/png"} for png in self.images]
        document = {"asset": {"version": "2.0", "generator": GENERATOR}, "scene": 0,
                    "scenes": [{"nodes": [0]}], "nodes": [{"name": self.name, "mesh": 0}],
                    "meshes": [{"name": self.name, "primitives": primitives}], "materials": self.materials,
                    "accessors": accessors, "bufferViews": views, "buffers": [{"byteLength": len(buffer)}]}
        if images:
            document["images"] = images
            document["samplers"] = [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}]
            document["textures"] = [{"sampler": 0, "source": i} for i in range(len(images))]
        while len(buffer) % 4:
            buffer.append(0)
        text = json.dumps(document, separators=(",", ":")).encode("utf-8")
        text += b" " * ((4 - len(text) % 4) % 4)
        total = 12 + 8 + len(text) + 8 + len(buffer)
        return (struct.pack("<III", 0x46546C67, 2, total) + struct.pack("<II", len(text), 0x4E4F534A) + text
                + struct.pack("<II", len(buffer), 0x004E4942) + bytes(buffer))


def validate_glb(data):
    """Structural check of a GLB container + accessor/bufferView bounds; returns the JSON document."""
    magic, version, total = struct.unpack_from("<III", data, 0)
    if magic != 0x46546C67 or version != 2 or total != len(data):
        raise ValueError("bad GLB header")
    json_len, json_type = struct.unpack_from("<II", data, 12)
    if json_type != 0x4E4F534A:
        raise ValueError("first chunk is not JSON")
    document = json.loads(data[20:20 + json_len])
    bin_len, bin_type = struct.unpack_from("<II", data, 20 + json_len)
    if bin_type != 0x004E4942 or 28 + json_len + bin_len != total:
        raise ValueError("bad BIN chunk")
    for view in document["bufferViews"]:
        if view["byteOffset"] + view["byteLength"] > bin_len:
            raise ValueError("bufferView outside BIN chunk")
    sizes = {"SCALAR": 1, "VEC2": 2, "VEC3": 3}
    for accessor in document["accessors"]:
        view = document["bufferViews"][accessor["bufferView"]]
        if accessor["count"] * sizes[accessor["type"]] * 4 > view["byteLength"]:
            raise ValueError("accessor overruns its bufferView")
    return document


# -- models ------------------------------------------------------------------------------------------
def building(seed, texture_px=256, noise=0.0):
    rng = np.random.default_rng(seed)
    mesh = MeshBuilder("building-%d" % seed)
    wall = tuple(int(c) for c in rng.integers(120, 210, 3))
    facade = mesh.material("facade", image=png_bytes(facade_texture(texture_px, seed, noise, wall)))
    roof = mesh.material("roof", color=(0.25, 0.25, 0.27, 1))
    width, depth = float(rng.uniform(8, 14)), float(rng.uniform(8, 14))
    height = float(rng.uniform(12, 40))
    mesh.box((0, height / 2, 0), (width, height, depth), facade)
    y = height
    for _tier in range(int(rng.integers(0, 3))):
        width, depth = width * 0.7, depth * 0.7
        extra = float(rng.uniform(4, 12))
        mesh.box((0, y + extra / 2, 0), (width, extra, depth), facade)
        y += extra
    mesh.box((0, y + 0.25, 0), (width * 1.02, 0.5, depth * 1.02), roof)
    return mesh


def tree(seed):
    rng = np.random.default_rng(seed)
    mesh = MeshBuilder("tree-%d" % seed)
    bark = mesh.material("bark", color=(0.36, 0.25, 0.14, 1))
    leaves = mesh.material("leaves", color=(0.16, float(rng.uniform(0.4, 0.6)), 0.18, 1))
    trunk = float(rng.uniform(1.5, 3))
    crown = float(rng.uniform(4, 8))
    mesh.cylinder((0, trunk / 2, 0), 0.25, trunk, bark, segments=12)
    mesh.cylinder((0, trunk + crown / 2, 0), crown * 0.35, crown, leaves, segments=16, top_radius=0.0)
    return mesh


def vehicle(seed):
    rng = np.random.default_rng(seed)
    mesh = MeshBuilder("vehicle-%d" % seed)
    paint = mesh.material("paint", color=tuple(float(c) for c in rng.uniform(0.1, 0.9, 3)) + (1,), metallic=0.4, roughness=0.4)
    glass = mesh.material("glass", color=(0.1, 0.15, 0.2, 1), roughness=0.1)
    rubber = mesh.material("rubber", color=(0.05, 0.05, 0.05, 1))
    mesh.box((0, 0.9, 0), (1.9, 0.9, 4.4), paint)
    mesh.box((0, 1.65, -0.2), (1.7, 0.6, 2.2), glass)
    for x in (-0.95, 0.95):
        for z in (-1.4, 1.4):
            mesh.cylinder((x, 0.38, z), 0.38, 0.3, rubber, segments=14, axis="x")
    return mesh


def write_asset(key, kind, mesh, out_dir, extra=None):
    data = mesh.glb()
    validate_glb(data)
    path = os.path.join(out_dir, key + ".glb")
    with open(path, "wb") as handle:
        handle.write(data)
    entry = {"key": key, "kind": kind, "file": key + ".glb", "path": path, "size": len(data),
             "sha256": hashlib.sha256(data).hexdigest(), "blake3": _blake3(data),
             "triangles": int(sum(len(p[3]) for p in mesh.primitives) // 3), "license": "generated (CC0)"}
    entry.update(extra or {})
    return entry


# Portal fixture archives are built by tools/qa/portal/build_bundles.py (sealed with a run-local publisher); this module
# only locates and verifies them so scenarios resolve `$assets.portal.<key>.path` (see tools/qa/portal/AGENTS.md section builder).
PORTAL_OUT_DIR = os.path.join(REPO_ROOT, ".qa", "assets", "portal")
PORTAL_MANIFEST_SCHEMA = "fe-qa-portal-assets/1"
PORTAL_KEYS = ("dashboard", "dashboard_unsigned", "docs_site", "actions_v1", "actions_v2", "actions_v3", "escape", "storm",
               "ladder_tier_100kb", "ladder_tier_1mb", "ladder_tier_5mb", "ladder_tier_25mb", "ladder_tier_40mb_overcap")


def portal_manifest(out_dir=PORTAL_OUT_DIR):
    """The portal asset manifest as {key: entry}; {} when build_bundles.py has not run."""
    try:
        with open(os.path.join(out_dir, "manifest.json"), encoding="utf-8") as handle:
            manifest = json.load(handle)
    except (OSError, ValueError):
        return {}
    if manifest.get("schema") != PORTAL_MANIFEST_SCHEMA:
        return {}
    return {entry["key"]: entry for entry in manifest.get("assets", [])}


def portal_missing(out_dir=PORTAL_OUT_DIR):
    """Keys scenarios reference that are absent from the manifest or whose archive file is gone or has the wrong size."""
    entries = portal_manifest(out_dir)
    missing = []
    for key in PORTAL_KEYS:
        entry = entries.get(key)
        if not entry or not os.path.isfile(entry.get("path", "")) or os.path.getsize(entry["path"]) != entry.get("bytes"):
            missing.append(key)
    return missing


def portal_ladder_targets():
    """Tier name to target archive bytes, shared with build_bundles.py so portal tiers track the asset ladder."""
    return {"ladder_" + tier: size for tier, size in LADDER.items()}


def _blake3(data):
    try:
        import blake3
    except ImportError:
        return None
    return blake3.blake3(data).hexdigest()


def ladder_asset(key, target, out_dir):
    """A building whose noisy facade texture sizes the GLB near the target byte count (chunked-transfer ladder)."""
    size = max(64, int(math.sqrt(target / 3)))
    for _attempt in range(5):
        mesh = building(4242, texture_px=size, noise=1.0)
        produced = len(mesh.glb())
        if abs(produced - target) / target < 0.08:
            break
        size = max(64, int(size * math.sqrt(target / produced)))
    return write_asset(key, "ladder", mesh, out_dir, {"tier_target_bytes": target, "texture_px": size})


def generate(out_dir=OUT_DIR, ladder=True):
    os.makedirs(out_dir, exist_ok=True)
    entries = []
    for index in range(5):
        entries.append(write_asset("building_%d" % index, "building", building(100 + index), out_dir, {"texture_px": 256}))
    for index in range(3):
        entries.append(write_asset("tree_%d" % index, "tree", tree(200 + index), out_dir))
    for index in range(3):
        entries.append(write_asset("vehicle_%d" % index, "vehicle", vehicle(300 + index), out_dir))
    if ladder:
        for key, target in LADDER.items():
            entries.append(ladder_asset(key, target, out_dir))
    manifest = {"schema": "fe-qa-sim-assets-v1", "generator": GENERATOR, "assets": entries}
    with open(os.path.join(out_dir, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=1)
    return manifest


def main(argv=None):
    parser = argparse.ArgumentParser(description="Generate deterministic test GLBs + manifest.json.")
    parser.add_argument("--out", default=OUT_DIR)
    parser.add_argument("--no-ladder", action="store_true", help="skip the 100 KB..25 MB texture ladder")
    parser.add_argument("--check-portal", action="store_true", help="list missing portal archives and exit (non-zero when any)")
    args = parser.parse_args(argv)
    if args.check_portal:
        missing = portal_missing()
        print("missing portal archives: %s" % (", ".join(missing) if missing else "none"))
        return 1 if missing else 0
    manifest = generate(args.out, not args.no_ladder)
    for entry in manifest["assets"]:
        print("%-12s %-10s %10d bytes  tris %-5d sha256 %s" % (entry["key"], entry["kind"], entry["size"],
                                                               entry["triangles"], entry["sha256"][:16]))
    print(os.path.join(args.out, "manifest.json"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
