"""Join every non-owner peer to an owner verse through the real GUI invite flow; see AGENTS.md §peers."""

import argparse
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
FE = [sys.executable, os.path.join(HERE, "fe.py")]


def fe(peer, *args, check=True):
    """Run one fe.py command bound to a peers.ps1 member; returns parsed JSON lines."""
    out = subprocess.run(FE + ["--peer", peer] + list(args), capture_output=True, text=True,
                         encoding="utf-8", cwd=REPO_ROOT, env={**os.environ, "MSYS_NO_PATHCONV": "1"})
    if check and out.returncode != 0:
        raise SystemExit("fe.py --peer %s %s failed (%d): %s %s" % (peer, " ".join(args), out.returncode,
                                                                   out.stdout.strip()[-400:], out.stderr.strip()[-400:]))
    lines = []
    for line in out.stdout.splitlines():
        try:
            lines.append(json.loads(line))
        except ValueError:
            pass
    return lines


def focus(peer):
    subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
                    os.path.join(HERE, "peers.ps1"), "-Action", "Focus", "-Peer", peer],
                   check=True, capture_output=True, cwd=REPO_ROOT)


def peers_record():
    with open(os.path.join(REPO_ROOT, ".qa", "peers-current"), encoding="utf-8-sig") as handle:
        owner_dir = handle.read().strip()
    with open(os.path.join(owner_dir, "peers.json"), encoding="utf-8-sig") as handle:
        return json.load(handle)


def wait_role(peer_dir, verse_id, role, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with open(os.path.join(peer_dir, "tokens.json"), encoding="utf-8-sig") as handle:
                if json.load(handle).get("roles", {}).get(verse_id) == role:
                    return True
        except (OSError, ValueError):
            pass
        time.sleep(1.0)
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verse-name", default="QA Basic")
    parser.add_argument("--timeout", type=float, default=90.0)
    args = parser.parse_args()
    record = peers_record()
    owner = record["members"][0]
    verse = args.verse_name
    with open(os.path.join(owner["run_dir"], "current.json"), encoding="utf-8-sig") as handle:
        verse_id = json.load(handle)["fixture_detail"]["verse_id"]
    fe("owner", "click", "--name", "Open verse " + verse)
    fe("owner", "click", "--name", "Options menu for verse " + verse)
    fe("owner", "click", "--name", "Options… verse " + verse)
    fe("owner", "click", "--name", "Access", "--role", "button")
    results = []
    for member in record["members"][1:]:
        fe("owner", "click", "--name", "Invitation recipient DID")
        fe("owner", "key", "ctrl+a")
        fe("owner", "type", member["public_did"])
        fe("owner", "click", "--name", "Scoped invite role")
        fe("owner", "click", "--name", member["role"], "--role", "button")
        fe(member["name"], "click", "--name", "Join verse by invite")
        fe("owner", "click", "--name", "Generate invite for this entry")
        focus(member["name"])
        transfer = fe("owner", "transfer-invite", "--recipient-peer", member["name"])[0]
        joined = wait_role(member["run_dir"], verse_id, member["role"], args.timeout)
        results.append({"peer": member["name"], "role": member["role"], "transfer": transfer.get("transfer"),
                        "generation": transfer.get("generation"), "role_token_published": joined})
    fe("owner", "click", "--name", "Close window", check=False)
    print(json.dumps({"verse_id": verse_id, "joins": results}, indent=1))
    return 0 if all(r["role_token_published"] for r in results) else 2


if __name__ == "__main__":
    sys.exit(main())
