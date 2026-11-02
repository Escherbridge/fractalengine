#!/usr/bin/env python3
"""Serve bounded, run-owned WEB and sealed registry fixtures; see AGENTS.md."""

import argparse
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import re
import socket
import stat
import threading
import time
import urllib.parse
import zipfile


REPO = Path(__file__).resolve().parents[2]
RUN_ROOT = REPO / ".qa" / "run"
HEADER_MEMBERS = (
    "entries.json", "license.json", "schema.json", "terrain/tileset_meta.json",
    "terrain/config.json", "members.json", "chunks/index.json",
)
MAX_ARCHIVE = 8 * 1024 * 1024
MAX_MEMBER = 2 * 1024 * 1024
MAX_REQUESTS = 2000


def confined(path, root):
    candidate = Path(os.path.abspath(path))
    boundary = Path(os.path.abspath(root))
    if candidate == boundary or boundary not in candidate.parents:
        raise ValueError("fixture path must be beneath its owned boundary")
    for part in (candidate, *candidate.parents):
        if (part.is_symlink() or (part.exists() and
                getattr(part.lstat(), "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT)):
            raise ValueError("fixture paths cannot traverse redirected directories")
        if part == boundary:
            break
    if candidate.resolve() != candidate:
        raise ValueError("fixture path resolution changed")
    return candidate


def read_object(path):
    if path.stat().st_size > MAX_MEMBER:
        raise ValueError("fixture JSON exceeds its size cap")
    value = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError("fixture JSON must be an object")
    return value


class Archive:
    def __init__(self, path, sha256):
        if not 0 < path.stat().st_size <= MAX_ARCHIVE:
            raise ValueError("fixture archive must be nonempty and at most 8 MiB")
        self.raw = path.read_bytes()
        if hashlib.sha256(self.raw).hexdigest() != sha256:
            raise ValueError("sealed fixture archive differs from preparation receipt")
        self.members = {}
        total = 0
        with zipfile.ZipFile(io.BytesIO(self.raw)) as package:
            if len(package.infolist()) > 256:
                raise ValueError("fixture archive has too many members")
            for entry in package.infolist():
                if entry.is_dir():
                    continue
                if entry.filename in self.members or entry.file_size > MAX_MEMBER:
                    raise ValueError("duplicate or oversized fixture member")
                if any(part in ("", ".", "..") for part in entry.filename.split("/")):
                    raise ValueError("fixture contains an invalid archive member path")
                total += entry.file_size
                if total > MAX_ARCHIVE:
                    raise ValueError("fixture archive exceeds decompression budget")
                with package.open(entry) as member:
                    data = member.read(MAX_MEMBER + 1)
                if len(data) != entry.file_size or len(data) > MAX_MEMBER:
                    raise ValueError("fixture archive member exceeded declared bounds")
                self.members[entry.filename] = data
        self.manifest = json.loads(self.members["manifest.json"])
        if (self.manifest.get("hexon_type") != "terrain_tileset"
                or not self.manifest.get("integrity") or not self.manifest.get("signature")):
            raise ValueError("fixture requires a genuinely sealed TerrainTileset")
        self.hexon_id = self.manifest["hexon_id"]
        self.version = self.manifest["version"]
        if not all(re.fullmatch(r"[A-Za-z0-9_.+-]{1,128}", value)
                   for value in (self.hexon_id, self.version)):
            raise ValueError("invalid fixture URI")
        if "chunks/index.json" in self.members:
            raise ValueError("controlled fixture expects the small monolithic QA source")
        self.uri = self.hexon_id + "@" + self.version
        damaged = io.BytesIO()
        damaged_member = next((name for name in self.members if name.startswith("terrain/tiles/")), None)
        if not damaged_member or not self.members[damaged_member]:
            raise ValueError("fixture must contain a real elevation tile")
        with zipfile.ZipFile(damaged, "w", compression=zipfile.ZIP_STORED) as package:
            for name, data in self.members.items():
                if name == damaged_member:
                    data = bytes([data[0] ^ 1]) + data[1:]
                package.writestr(name, data)
        self.corrupt_raw = damaged.getvalue()
        self.header = {
            "v": 1, "uri": self.uri,
            "manifest": self.members["manifest.json"].decode("utf-8"),
            "header_members": {name: self.members[name].decode("utf-8")
                               for name in HEADER_MEMBERS if name in self.members},
            "archive_size": len(self.raw), "layout": {"kind": "monolithic"},
        }
        self.summary = {name: self.manifest.get(name) for name in (
            "hexon_id", "version", "hexon_type", "name", "description", "tags", "publisher_did")}
        self.summary["size_bytes"] = len(self.raw)

    def matches(self, query):
        if set(query) - {"q", "tags", "type"} or any(len(v) != 1 for v in query.values()):
            return False
        needle = query.get("q", [""])[0].lower()
        fields = (self.hexon_id, self.manifest.get("name") or "", self.manifest.get("description") or "")
        tags = [tag.strip() for tag in query.get("tags", [""])[0].split(",") if tag.strip()]
        return (any(needle in field.lower() for field in fields)
                and all(tag in self.manifest.get("tags", []) for tag in tags)
                and query.get("type", [self.manifest["hexon_type"]])[0] == self.manifest["hexon_type"])


class FixtureServer(http.server.HTTPServer):
    allow_reuse_address = False

    def __init__(self, *args, **kwargs):
        self._connection_deadlines = {}
        self._deadline_lock = threading.Lock()
        super().__init__(*args, **kwargs)

    def server_bind(self):
        if hasattr(socket, "SO_EXCLUSIVEADDRUSE"):
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        super().server_bind()

    def get_request(self):
        connection, address = super().get_request()
        self.request_count += 1
        connection.settimeout(2)
        self.arm_deadline(connection, min(time.monotonic() + 2, self.deadline))
        return connection, address

    def arm_deadline(self, connection, deadline):
        token = object()
        timer = threading.Timer(max(0, deadline - time.monotonic()), self.expire_connection,
                                args=(connection, token))
        timer.daemon = True
        with self._deadline_lock:
            previous = self._connection_deadlines.pop(connection, None)
            if previous:
                previous[1].cancel()
            self._connection_deadlines[connection] = (token, timer)
            timer.start()

    def expire_connection(self, connection, token):
        with self._deadline_lock:
            current = self._connection_deadlines.get(connection)
            if not current or current[0] is not token:
                return
            del self._connection_deadlines[connection]
        try:
            connection.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        connection.close()

    def close_request(self, request):
        with self._deadline_lock:
            previous = self._connection_deadlines.pop(request, None)
            if previous:
                previous[1].cancel()
        super().close_request(request)


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "FEControlledFixture/1"

    def log_message(self, *_args):
        pass

    def handle_one_request(self):
        if self.server.request_count > MAX_REQUESTS or time.monotonic() >= self.server.deadline:
            self.close_connection = True
            return
        try:
            super().handle_one_request()
        except OSError:
            pass
        finally:
            self.close_connection = True

    def parse_request(self):
        valid = super().parse_request()
        if valid:
            self.server.arm_deadline(self.connection, self.server.deadline)
        return valid

    def do_GET(self):
        if (len(self.path) > 2048 or self.headers.get("Authorization")
                or self.headers.get("Host") != self.server.origin.removeprefix("http://")):
            self.send_error(400)
            return
        parsed = urllib.parse.urlsplit(self.path)
        if parsed.scheme or parsed.netloc or parsed.fragment:
            self.send_error(400)
            return
        path = urllib.parse.unquote(parsed.path)
        if path == "/health" and not parsed.query:
            self.respond_json({"schema": "fe-qa-controlled-fixtures-v1", "instance": self.server.instance})
            return
        if path in ("/web/one", "/web/two", "/web/three") and not parsed.query:
            label = path.rsplit("/", 1)[1]
            page = ("<!doctype html><html><head><meta charset=utf-8><title>QA Portal " + label
                    + "</title></head><body><h1>QA Portal " + label + "</h1>"
                    + '<a href="/web/two">Second fixture page</a> '
                    + '<a href="/web/three">Third fixture page</a></body></html>')
            self.respond(page.encode("utf-8"), "text/html; charset=utf-8")
            return
        archive = self.server.archive
        if path == "/api/v1/hexons/search":
            query = urllib.parse.parse_qs(parsed.query, keep_blank_values=True)
            self.respond_json([archive.summary] if archive.matches(query) else [])
            return
        prefix = "/api/v1/hexons/"
        parts = path[len(prefix):].split("/") if path.startswith(prefix) else []
        if parsed.query or not parts or parts[0] not in (archive.hexon_id, archive.uri):
            self.send_error(404)
            return
        tail = parts[1:]
        if not tail:
            self.respond_json(archive.manifest)
        elif tail == ["meta"]:
            self.respond_json(archive.header)
        elif tail == ["entries"]:
            self.respond_json(json.loads(archive.members["entries.json"]))
        elif tail in (["download"], ["chunk", "0"]):
            data = archive.corrupt_raw if self.server.mode == "corrupt" else archive.raw
            self.respond(data, "application/octet-stream", binary=True)
        elif tail and tail[0] == "member" and "/".join(tail[1:]) in archive.members:
            self.respond_binary(archive.members["/".join(tail[1:])])
        else:
            self.send_error(404)

    def respond_json(self, value):
        self.respond(json.dumps({"ok": True, "data": value, "error": None}).encode("utf-8"), "application/json")

    def respond_binary(self, data):
        if self.server.mode == "corrupt":
            data = data[:-1] + bytes([data[-1] ^ 1])
        self.respond(data, "application/octet-stream", binary=True)

    def respond(self, data, content_type, binary=False):
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        if binary:
            self.send_header("Content-Disposition", 'attachment; filename="qa-registry.hexon"')
        else:
            self.send_header("Content-Security-Policy", "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
        self.end_headers()
        try:
            for offset in range(0, len(data), 512):
                if time.monotonic() >= self.server.deadline:
                    return
                self.wfile.write(data[offset:offset + 512])
                self.wfile.flush()
                if binary and self.server.mode == "slow":
                    time.sleep(0.25)
        except (BrokenPipeError, ConnectionResetError, socket.timeout):
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True)
    args = parser.parse_args()
    config_path = confined(args.config, RUN_ROOT)
    run = config_path.parents[2]
    if run.parent != RUN_ROOT or config_path.parent.parent != run / "controlled-fixtures":
        raise ValueError("config must be in one direct owned run's fixture instance")
    config = read_object(config_path)
    current = read_object(confined(run / "current.json", RUN_ROOT))
    if (Path(current["run_dir"]) != run or config.get("schema") != "fe-qa-controlled-fixtures-v1"
            or config.get("mode") not in ("normal", "slow", "corrupt")
            or type(config.get("port")) is not int or not 1024 <= config["port"] <= 65535
            or type(config.get("lifetime_seconds")) is not int or not 30 <= config["lifetime_seconds"] <= 3600
            or not re.fullmatch("[a-f0-9]{32}", config.get("instance", ""))):
        raise ValueError("invalid owned fixture configuration")
    origin = "http://127.0.0.1:" + str(config["port"])
    if origin == current["api_base"].rstrip("/"):
        raise ValueError("fixture port cannot be the app API port")
    archive_path = confined(config["archive_path"], config_path.parent)
    archive = Archive(archive_path, config["archive_sha256"])
    with FixtureServer(("127.0.0.1", config["port"]), Handler) as server:
        server.origin = origin
        server.instance = config["instance"]
        server.archive = archive
        server.mode = config["mode"]
        server.request_count = 0
        server.deadline = time.monotonic() + config["lifetime_seconds"]
        server.timeout = 0.25
        ready = {"schema": config["schema"], "instance": config["instance"], "origin": origin,
                 "uri": archive.uri, "archive_sha256": config["archive_sha256"], "pid": os.getpid()}
        ready_path = confined(config_path.parent / "ready.json", config_path.parent)
        with ready_path.open("x", encoding="utf-8") as handle:
            json.dump(ready, handle)
        while time.monotonic() < server.deadline and server.request_count < MAX_REQUESTS:
            server.handle_request()


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, zipfile.BadZipFile, UnicodeError) as error:
        raise SystemExit("controlled fixture refused: " + str(error))
