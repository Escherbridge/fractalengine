#!/usr/bin/env python3
"""Package a built binary into a user-facing release archive (.zip for Windows, .tar.gz otherwise).

Usage (from CI, see .github/workflows/build-artifacts.yml and release.yml):
    python3 scripts/package_release.py --kind gui --platform linux-x86_64 \
        --binary target/release/fractalengine --out-dir dist

Layout and rationale: packaging/README.md.
"""

from __future__ import annotations

import argparse
import os
import shutil
import sys
import tarfile
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PACKAGING_DIR = REPO_ROOT / "packaging"

# (source in the repo, destination inside the archive). The GUI resolves
# `<exe dir>/assets` when it exists (fe-runtime src/AGENTS.md §packaged-layout).
GUI_ASSET_TREES = [("fractalengine/assets/samples", "assets/samples")]
LICENSE_FILES = ["LICENSE-APACHE", "THIRD-PARTY-LICENSES.md"]
ARCHIVE_STEM = {"gui": "fractalengine", "relay": "fe-relay"}


def windows_text(platform: str) -> dict[str, str]:
    """Platform-specific QUICKSTART fragments for Windows archives."""
    return {
        "RUN_GUI": (
            "  1. Extract the whole zip (not just the .exe) to a folder you can\n"
            "     write to, for example Documents\\FractalEngine.\n"
            "  2. Double-click fractalengine.exe. If SmartScreen warns, choose\n"
            "     \"More info\" > \"Run anyway\".\n"
            "\n"
            "  To see logs, run it from a terminal instead:\n"
            "      cd <folder>\n"
            "      .\\fractalengine.exe"
        ),
        "RUN_RELAY": (
            "  Extract the zip, open a terminal in the extracted folder and run:\n"
            "      .\\fe-relay.exe"
        ),
        "REQUIREMENTS": (
            "  * Windows 10 (1803 or later) or Windows 11, " + platform.split("-", 1)[1] + ".\n"
            "  * The Microsoft Edge WebView2 runtime. Windows 11 includes it. On\n"
            "    Windows 10, install it from\n"
            "    https://developer.microsoft.com/microsoft-edge/webview2/\n"
            "  * A GPU with DirectX 12 or Vulkan drivers."
        ),
        "BLOBS": "%LOCALAPPDATA%\\fractalengine\\blobs\\",
        "SETTINGS": "%APPDATA%\\fractalengine\\settings.ron",
        "KEYCHAIN": "Windows Credential Manager",
    }


def unix_text(platform: str) -> dict[str, str]:
    """Platform-specific QUICKSTART fragments for Linux archives."""
    return {
        "RUN_GUI": (
            "      tar -xzf fractalengine-" + platform + ".tar.gz\n"
            "      cd fractalengine-" + platform + "\n"
            "      ./fractalengine"
        ),
        "RUN_RELAY": (
            "      tar -xzf fe-relay-" + platform + ".tar.gz\n"
            "      cd fe-relay-" + platform + "\n"
            "      ./fe-relay"
        ),
        "REQUIREMENTS": (
            "  * 64-bit Linux with glibc 2.39 or newer (Ubuntu 24.04, Debian 13,\n"
            "    Fedora 40 or later). The binary is built on Ubuntu 24.04.\n"
            "  * X11 or Wayland, plus Vulkan drivers (Mesa or your GPU vendor's).\n"
            "  * Runtime libraries. On Ubuntu or Debian:\n"
            "      sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0 libxkbcommon-x11-0 \\\n"
            "                       libasound2t64 libudev1 libvulkan1 mesa-vulkan-drivers\n"
            "    (On Debian 12 and older, the ALSA package is libasound2.)\n"
            "  * A Secret Service keyring, such as GNOME Keyring or KWallet, to keep\n"
            "    the node identity between launches. Without one, the app warns and\n"
            "    uses a new identity on every start."
        ),
        "BLOBS": "~/.local/share/fractalengine/blobs/",
        "SETTINGS": "~/.config/fractalengine/settings.ron",
        "KEYCHAIN": "Secret Service",
    }


def render_quickstart(kind: str, platform: str) -> str:
    """Fill the QUICKSTART template for this archive."""
    is_windows = platform.startswith("windows")
    text = windows_text(platform) if is_windows else unix_text(platform)
    template = (PACKAGING_DIR / f"QUICKSTART-{kind}.txt").read_text(encoding="utf-8")
    sha = os.environ.get("GITHUB_SHA", "local build")
    ref = os.environ.get("GITHUB_REF_NAME", "")
    build = f"{sha} ({ref})" if ref else sha
    run = text["RUN_GUI"] if kind == "gui" else text["RUN_RELAY"]
    values = {
        "PLATFORM": platform,
        "BUILD": build,
        "RUN": run,
        "REQUIREMENTS": text["REQUIREMENTS"],
        "BLOBS": text["BLOBS"],
        "SETTINGS": text["SETTINGS"],
        "KEYCHAIN": text["KEYCHAIN"],
    }
    for key, value in values.items():
        template = template.replace(f"@@{key}@@", value)
    leftover = [part for part in template.split("@@")[1::2] if part.isupper()]
    if leftover:
        sys.exit(f"QUICKSTART-{kind}.txt has unfilled placeholders: {leftover}")
    return template.replace("\n", "\r\n") if is_windows else template


def stage(kind: str, platform: str, binary: Path, staging: Path) -> None:
    """Copy the binary, runtime assets, licences and QUICKSTART into `staging`."""
    if staging.exists():
        shutil.rmtree(staging)
    staging.mkdir(parents=True)
    shutil.copy2(binary, staging / binary.name)
    if kind == "gui":
        for source, destination in GUI_ASSET_TREES:
            source_dir = REPO_ROOT / source
            if not source_dir.is_dir():
                sys.exit(f"missing runtime asset tree: {source_dir}")
            shutil.copytree(source_dir, staging / destination)
    for name in LICENSE_FILES:
        shutil.copy2(REPO_ROOT / name, staging / name)
    quickstart = render_quickstart(kind, platform)
    (staging / "QUICKSTART.txt").write_bytes(quickstart.encode("utf-8"))


def write_zip(staging: Path, archive: Path) -> None:
    """Zip `staging` under a top-level folder of the same name."""
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for path in sorted(staging.rglob("*")):
            zf.write(path, Path(staging.name) / path.relative_to(staging))


def write_tar_gz(staging: Path, archive: Path, binary_name: str) -> None:
    """Tar+gzip `staging` with the binary marked executable and neutral ownership."""

    def normalize(info: tarfile.TarInfo) -> tarfile.TarInfo:
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        is_binary = info.name == f"{staging.name}/{binary_name}"
        info.mode = 0o755 if info.isdir() or is_binary else 0o644
        return info

    with tarfile.open(archive, "w:gz", compresslevel=9) as tf:
        tf.add(staging, arcname=staging.name, filter=normalize)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--kind", choices=sorted(ARCHIVE_STEM), required=True)
    parser.add_argument("--platform", required=True, help="e.g. windows-x86_64, linux-musl-aarch64")
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    args = parser.parse_args()

    if not args.binary.is_file():
        sys.exit(f"binary not found: {args.binary}")
    name = f"{ARCHIVE_STEM[args.kind]}-{args.platform}"
    args.out_dir.mkdir(parents=True, exist_ok=True)
    staging = args.out_dir / ".staging" / name
    stage(args.kind, args.platform, args.binary, staging)

    if args.platform.startswith("windows"):
        archive = args.out_dir / f"{name}.zip"
        write_zip(staging, archive)
    else:
        archive = args.out_dir / f"{name}.tar.gz"
        write_tar_gz(staging, archive, args.binary.name)
    shutil.rmtree(args.out_dir / ".staging")
    print(f"{archive} ({archive.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
