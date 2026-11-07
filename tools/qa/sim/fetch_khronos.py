"""Optional: cache a few KhronosGroup glTF-Sample-Assets GLBs (CC0 / CC-BY 4.0 only) with per-asset licences; see tools/qa/AGENTS.md §sim."""
import argparse
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
OUT_DIR = os.path.join(REPO_ROOT, ".qa", "assets", "khronos")
BASE = "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/%s/"
MODELS = ("Duck", "Avocado", "DamagedHelmet", "BoomBox")
ACCEPTED = (re.compile(r"\bCC0\b", re.I), re.compile(r"CC[- ]BY[- ]4\.0", re.I))
MAX_BYTES = 64 * 1024 * 1024
TIMEOUT_S = 30


def fetch(url, limit=MAX_BYTES):
    request = urllib.request.Request(url, headers={"User-Agent": "fe-qa-sim/1"})
    with urllib.request.urlopen(request, timeout=TIMEOUT_S) as response:
        data = response.read(limit + 1)
    if len(data) > limit:
        raise ValueError("%s exceeds %d bytes" % (url, limit))
    return data


def licence_of(model):
    """(licence strings, attribution lines, source) read from the model's metadata.json, else README.md."""
    try:
        meta = json.loads(fetch(BASE % model + "metadata.json", 1024 * 1024))
        legal = meta.get("legal") or []
        licences = sorted({str(entry.get("license") or entry.get("spdx") or "") for entry in legal if isinstance(entry, dict)} - {""})
        credits = ["%s (%s, %s)" % (entry.get("artist") or entry.get("owner") or "?", entry.get("license"), entry.get("year", "?"))
                   for entry in legal if isinstance(entry, dict)]
        if licences:
            return licences, credits, "metadata.json"
    except (urllib.error.URLError, ValueError, OSError, TimeoutError):
        pass
    readme = fetch(BASE % model + "README.md", 1024 * 1024).decode("utf-8", "replace")
    lines = [line.strip() for line in readme.splitlines() if re.search(r"licen[cs]e|CC0|CC[- ]BY", line, re.I)]
    return lines[:6], lines[:6], "README.md"


def main(argv=None):
    parser = argparse.ArgumentParser(description="Cache Khronos sample GLBs into .qa/assets/khronos (skips cleanly offline).")
    parser.add_argument("--out", default=OUT_DIR)
    parser.add_argument("--models", default=",".join(MODELS))
    args = parser.parse_args(argv)
    os.makedirs(args.out, exist_ok=True)
    entries, licences_md, skipped = [], ["# Khronos glTF-Sample-Assets cached for tools/qa/sim", "",
                                         "Source: https://github.com/KhronosGroup/glTF-Sample-Assets (fetched by fetch_khronos.py).", ""], []
    for model in [m.strip() for m in args.models.split(",") if m.strip()]:
        try:
            licences, credits, source = licence_of(model)
            # Every licence on the model must be CC0 or CC-BY 4.0 (a co-licensed CC-BY-NC part disqualifies it).
            if not licences or any(re.search(r"NC|-NC-", text) or not any(p.search(text) for p in ACCEPTED)
                                   for text in licences):
                skipped.append("%s: licence %s is not purely CC0/CC-BY 4.0" % (model, licences))
                continue
            data = fetch(BASE % model + "glTF-Binary/%s.glb" % model)
            if data[:4] != b"glTF":
                skipped.append("%s: not a GLB" % model)
                continue
            directory = os.path.join(args.out, model)
            os.makedirs(directory, exist_ok=True)
            path = os.path.join(directory, model + ".glb")
            with open(path, "wb") as handle:
                handle.write(data)
            entries.append({"key": model, "kind": "khronos", "file": model + "/" + model + ".glb", "path": path,
                            "size": len(data), "sha256": hashlib.sha256(data).hexdigest(), "blake3": _blake3(data),
                            "license": "; ".join(licences), "license_source": source, "credits": credits,
                            "url": BASE % model + "glTF-Binary/%s.glb" % model})
            licences_md += ["## %s" % model, "", "- licence: %s (from %s)" % ("; ".join(licences), source),
                            "- credits: %s" % ("; ".join(credits) or "see source"), "- url: %s" % entries[-1]["url"], ""]
        except (urllib.error.URLError, OSError, TimeoutError, ValueError) as error:
            skipped.append("%s: %s" % (model, error))
    if not entries and skipped and all("urlopen" in s or "getaddrinfo" in s or "timed out" in s for s in skipped):
        print("offline: nothing cached (%s)" % "; ".join(skipped))
        return 0
    with open(os.path.join(args.out, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump({"schema": "fe-qa-sim-assets-v1", "generator": "fetch_khronos.py", "assets": entries,
                   "skipped": skipped}, handle, indent=1)
    with open(os.path.join(args.out, "LICENSES.md"), "w", encoding="utf-8") as handle:
        handle.write("\n".join(licences_md + (["## Skipped", ""] + ["- " + s for s in skipped] if skipped else [])) + "\n")
    for entry in entries:
        print("%-14s %9d bytes  %s" % (entry["key"], entry["size"], entry["license"]))
    for line in skipped:
        print("skipped " + line)
    return 0


def _blake3(data):
    try:
        import blake3
    except ImportError:
        return None
    return blake3.blake3(data).hexdigest()


if __name__ == "__main__":
    sys.exit(main())
