"""Loopback fixtures for the extension journeys: signed static index and revocation list, MQTT 3.1.1 broker (QoS 0/1), HTTP sink; see tools/qa/sim/driver.py ext ops."""
import hashlib
import json
import os
import socket
import socketserver
import struct
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOOPBACK_HOST = "127.0.0.1"
INDEX_DOMAIN = b"fractalengine/catalog-index/v1\n"  # fe-hexon registry_source INDEX_SIGNING_DOMAIN + LF
# Two keys, never one: an index key that is also a trust root is refused (fe-hexon verify_index IndexKeyMisused,
# store-client steer 2026-11-08 20:45), so the index key is pinned per source and the revocation key is the trust root.
INDEX_KEY_ID = "fe-sim-index"
REVOCATION_KEY_ID = "fe-sim-revocation"
SOURCE_ID = "fe-sim-loopback"
DEFAULT_MQTT_PORT = int(os.environ.get("FE_SIM_MQTT_PORT", "18831"))
DEFAULT_HTTP_PORT = int(os.environ.get("FE_SIM_LOOPBACK_HTTP_PORT", "18832"))
DEFAULT_SINK_PORT = int(os.environ.get("FE_SIM_SINK_PORT", "18833"))


def canonical(value):
    """Sorted-key compact JSON, byte-equal to fe-plugin trust_roots write_canonical for ASCII documents."""
    if isinstance(value, dict):
        return "{" + ",".join(json.dumps(k) + ":" + canonical(value[k]) for k in sorted(value)) + "}"
    if isinstance(value, list):
        return "[" + ",".join(canonical(item) for item in value) + "]"
    return json.dumps(value, ensure_ascii=False)


class SigningRoot:
    """One run-seeded ed25519 key; `purpose` keeps the index key and the revocation root distinct."""

    def __init__(self, seed_text, purpose):
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        self._key = Ed25519PrivateKey.from_private_bytes(
            hashlib.sha256(("fe-sim-%s:%s" % (purpose, seed_text)).encode()).digest())
        self.public_hex = self._key.public_key().public_bytes(
            serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()

    def sign_hex(self, message):
        return self._key.sign(message).hex()


def signed_index(index_key, doc):
    """Index document signed as INDEX_KEY_ID: the domain line, then canonical JSON without `signature`."""
    doc = {k: v for k, v in doc.items() if k != "signature"}
    doc["signer_key_id"] = INDEX_KEY_ID
    doc["signature"] = index_key.sign_hex(INDEX_DOMAIN + canonical(doc).encode("utf-8"))
    return doc


def signed_revocation(revocation_root, entries, issued_at):
    """schema_version 1 revocation list; the signature covers canonical {entries, issued_at} only."""
    body = canonical({"entries": entries, "issued_at": issued_at}).encode("utf-8")
    return {"schema_version": 1, "issued_at": issued_at, "entries": entries, "signer_key_id": REVOCATION_KEY_ID,
            "signature": revocation_root.sign_hex(body)}


def blake3_hex(data):
    try:
        import blake3
    except ImportError as error:
        raise RuntimeError("the signed index needs the blake3 package for detail_hash (pip install blake3)") from error
    return blake3.blake3(data).hexdigest()


class StaticSite:
    """Serves a path -> (bytes, content-type) map over HTTP on loopback; the map can change while it runs."""

    def __init__(self, port):
        self.files = {}
        self.requests = []
        self._lock = threading.Lock()
        site = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                with site._lock:
                    entry = site.files.get(self.path.split("?")[0])
                    site.requests.append(("GET", self.path))
                if entry is None:
                    self.send_response(404)
                    self.end_headers()
                    return
                body, kind = entry
                self.send_response(200)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        self.server = ThreadingHTTPServer((LOOPBACK_HOST, port), Handler)
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True, name="loopback-site")

    def put(self, path, body, kind="application/json"):
        with self._lock:
            self.files[path] = (body if isinstance(body, bytes) else body.encode("utf-8"), kind)

    def start(self):
        self.thread.start()

    def stop(self):
        self.server.shutdown()
        self.server.server_close()


class HttpSink:
    """Records every request body and header (an egress target); headers are kept raw so a run can prove credentials were stripped."""

    def __init__(self, port):
        self.received = []
        self._lock = threading.Lock()
        sink = self

        class Handler(BaseHTTPRequestHandler):
            def _take(self):
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length) if length else b""
                with sink._lock:
                    sink.received.append({"t": time.time(), "method": self.command, "path": self.path,
                                          "headers": {k.lower(): v for k, v in self.headers.items()}, "body": body})
                self.send_response(200)
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"{}")

            do_POST = do_PUT = do_GET = _take  # noqa: N815

            def log_message(self, *_args):
                pass

        self.server = ThreadingHTTPServer((LOOPBACK_HOST, port), Handler)
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True, name="loopback-sink")

    def start(self):
        self.thread.start()

    def stop(self):
        self.server.shutdown()
        self.server.server_close()


# -- MQTT 3.1.1 broker -------------------------------------------------------------------------------
CONNECT, CONNACK, PUBLISH, PUBACK, SUBSCRIBE, SUBACK, UNSUBSCRIBE, UNSUBACK, PINGREQ, PINGRESP, DISCONNECT = (
    1, 2, 3, 4, 8, 9, 10, 11, 12, 13, 14)


def topic_matches(filter_text, topic):
    """MQTT filter match: `+` one level, `#` the rest."""
    want, have = filter_text.split("/"), topic.split("/")
    for index, part in enumerate(want):
        if part == "#":
            return True
        if index >= len(have) or (part != "+" and part != have[index]):
            return False
    return len(want) == len(have)


def _encode_length(size):
    out = bytearray()
    while True:
        digit, size = size % 128, size // 128
        out.append(digit | (0x80 if size else 0))
        if not size:
            return bytes(out)


def _packet(kind, flags, body):
    return bytes([(kind << 4) | flags]) + _encode_length(len(body)) + body


def _read_exact(sock, count):
    data = b""
    while len(data) < count:
        chunk = sock.recv(count - len(data))
        if not chunk:
            raise ConnectionError("closed")
        data += chunk
    return data


def _read_packet(sock):
    first = _read_exact(sock, 1)[0]
    size, shift = 0, 0
    while True:
        byte = _read_exact(sock, 1)[0]
        size |= (byte & 0x7F) << shift
        if not byte & 0x80:
            break
        shift += 7
        if shift > 21:
            raise ConnectionError("malformed length")
    return first >> 4, first & 0x0F, _read_exact(sock, size)


def _utf8(buffer, at):
    (size,) = struct.unpack_from(">H", buffer, at)
    return buffer[at + 2:at + 2 + size].decode("utf-8"), at + 2 + size


class MqttBroker:
    """Minimal in-process broker: CONNECT, SUBSCRIBE with wildcards, PUBLISH QoS 0/1, PINGREQ; `inject` is a virtual device publish."""

    def __init__(self, port):
        self.received = []  # client publishes (e.g. the host's egress): {topic, payload, qos, client}
        self.connects = []
        self._sessions = []  # [sock, client_id, [(filter, qos)], send_lock]
        self._lock = threading.Lock()
        self._packet_id = 0
        broker = self

        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                broker._serve(self.request)

        class Server(socketserver.ThreadingTCPServer):
            allow_reuse_address = True
            daemon_threads = True

        self.server = Server((LOOPBACK_HOST, port), Handler)
        self.port = self.server.server_address[1]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True, name="loopback-mqtt")

    def start(self):
        self.thread.start()

    def stop(self):
        self.server.shutdown()
        self.server.server_close()
        with self._lock:
            for session in self._sessions:
                try:
                    session[0].close()
                except OSError:
                    pass

    def subscribers(self, topic):
        with self._lock:
            return sum(1 for s in self._sessions if any(topic_matches(f, topic) for f, _q in s[2]))

    def inject(self, topic, payload, qos=0):
        """Publish as a virtual device; returns how many subscriber sessions it was delivered to."""
        return self._route(topic, payload if isinstance(payload, bytes) else payload.encode("utf-8"), qos, None)

    def _next_id(self):
        with self._lock:
            self._packet_id = self._packet_id % 65535 + 1
            return self._packet_id

    def _route(self, topic, payload, qos, origin):
        best = {}  # one delivery per session at its best matching subscription QoS
        with self._lock:
            for s in self._sessions:
                if s is origin:
                    continue
                for f, q in s[2]:
                    if topic_matches(f, topic):
                        best[id(s)] = (s, max(q, best[id(s)][1]) if id(s) in best else q)
        delivered = 0
        for session, granted in best.values():
            effective = min(qos, granted, 1)
            body = struct.pack(">H", len(topic.encode())) + topic.encode()
            if effective:
                body += struct.pack(">H", self._next_id())
            try:
                with session[3]:
                    session[0].sendall(_packet(PUBLISH, effective << 1, body + payload))
                delivered += 1
            except OSError:
                pass
        return delivered

    def _serve(self, sock):
        session = [sock, None, [], threading.Lock()]
        try:
            kind, _flags, body = _read_packet(sock)
            if kind != CONNECT:
                return
            _proto, at = _utf8(body, 0)
            client_id, _ = _utf8(body, at + 4)
            session[1] = client_id
            self.connects.append({"t": time.time(), "client": client_id})
            sock.sendall(_packet(CONNACK, 0, b"\x00\x00"))
            with self._lock:
                self._sessions.append(session)
            while True:
                kind, flags, body = _read_packet(sock)
                if kind == PUBLISH:
                    qos = (flags >> 1) & 3
                    topic, at = _utf8(body, 0)
                    if qos:
                        (packet_id,) = struct.unpack_from(">H", body, at)
                        at += 2
                        sock.sendall(_packet(PUBACK, 0, struct.pack(">H", packet_id)))
                    payload = body[at:]
                    self.received.append({"t": time.time(), "topic": topic, "payload": payload, "qos": qos, "client": client_id})
                    self._route(topic, payload, qos, session)
                elif kind == SUBSCRIBE:
                    (packet_id,) = struct.unpack_from(">H", body, 0)
                    at, granted = 2, b""
                    while at < len(body):
                        filter_text, at = _utf8(body, at)
                        qos = min(body[at], 1)
                        at += 1
                        with self._lock:
                            session[2] = [(f, q) for f, q in session[2] if f != filter_text] + [(filter_text, qos)]
                        granted += bytes([qos])
                    sock.sendall(_packet(SUBACK, 0, struct.pack(">H", packet_id) + granted))
                elif kind == UNSUBSCRIBE:
                    (packet_id,) = struct.unpack_from(">H", body, 0)
                    at = 2
                    while at < len(body):
                        filter_text, at = _utf8(body, at)
                        with self._lock:
                            session[2] = [(f, q) for f, q in session[2] if f != filter_text]
                    sock.sendall(_packet(UNSUBACK, 0, struct.pack(">H", packet_id)))
                elif kind == PINGREQ:
                    sock.sendall(_packet(PINGRESP, 0, b""))
                elif kind == DISCONNECT:
                    return
                # PUBACK from a subscriber needs no action: delivery is fire-and-forget here
        except (ConnectionError, OSError, struct.error, UnicodeDecodeError):
            pass
        finally:
            with self._lock:
                if session in self._sessions:
                    self._sessions.remove(session)
            try:
                sock.close()
            except OSError:
                pass


class Loopback:
    """The run's loopback services; ports are fixed by default so a fixture build can name them before the run."""

    def __init__(self, run_dir, seed, mqtt_port=DEFAULT_MQTT_PORT, http_port=DEFAULT_HTTP_PORT, sink_port=DEFAULT_SINK_PORT):
        self.dir = os.path.join(run_dir, "loopback")
        os.makedirs(self.dir, exist_ok=True)
        self.index_key = SigningRoot(seed, "index")
        self.revocation_root = SigningRoot(seed, "revocation")
        self.site = StaticSite(http_port)
        self.sink = HttpSink(sink_port)
        self.broker = MqttBroker(mqtt_port)
        self.revocation_serial = 0
        self.revocation_entries = []
        self.started = False
        self.publish_revocation([], at=0)  # an empty, valid list is live from the first pull

    @property
    def revocation_url(self):
        return "http://%s:%d/revocation.json" % (LOOPBACK_HOST, self.site.port)

    @property
    def index_url(self):
        return "http://%s:%d/" % (LOOPBACK_HOST, self.site.port)

    @property
    def mqtt_url(self):
        return "mqtt://%s:%d" % (LOOPBACK_HOST, self.broker.port)

    @property
    def sink_url(self):
        return "http://%s:%d/" % (LOOPBACK_HOST, self.sink.port)

    def describe(self):
        """What the peers need to trust this run's revocation root and pin its index key (see write_extension_config)."""
        # role "publisher", not "dev": a dev root loads only with FE_EXT_DEV=1 in a debug build, so a release stage
        # would drop it silently and never accept a revocation list.
        trusted = ('[[publisher]]\nkey_id = "%s"\npublic_key = "%s"\nrevocation = true\nrevocation_url = "%s"\n'
                   'role = "publisher"\n' % (REVOCATION_KEY_ID, self.revocation_root.public_hex, self.revocation_url))
        source = {"id": SOURCE_ID, "label": "fe-sim loopback", "kind": "static", "base": self.index_url,
                  "index_keys": [{"key_id": INDEX_KEY_ID, "public_key": self.index_key.public_hex}]}
        return {"revocation_key_id": REVOCATION_KEY_ID, "revocation_public_key": self.revocation_root.public_hex,
                "index_key_id": INDEX_KEY_ID, "index_public_key": self.index_key.public_hex,
                "revocation_url": self.revocation_url, "index_url": self.index_url, "mqtt_url": self.mqtt_url,
                "sink_url": self.sink_url, "trusted_publisher_toml": trusted, "extension_sources": [source]}

    def write_extension_config(self, config_dir):
        """Write trusted_publishers.toml and extension_sources.json where the peers read them (FE_EXT_CONFIG_DIR)."""
        info = self.describe()
        os.makedirs(config_dir, exist_ok=True)
        with open(os.path.join(config_dir, "trusted_publishers.toml"), "w", encoding="utf-8", newline="\n") as handle:
            handle.write(info["trusted_publisher_toml"])
        with open(os.path.join(config_dir, "extension_sources.json"), "w", encoding="utf-8", newline="\n") as handle:
            json.dump(info["extension_sources"], handle, indent=1)
        return config_dir

    def start(self):
        if self.started:
            return self.describe()
        for service in (self.site, self.sink, self.broker):
            service.start()
        self.started = True
        info = self.describe()
        with open(os.path.join(self.dir, "loopback.json"), "w", encoding="utf-8") as handle:
            json.dump(info, handle, indent=1)
        return info

    def stop(self):
        if not self.started:
            return
        for service in (self.broker, self.sink, self.site):
            try:
                service.stop()
            except OSError:
                pass
        self.started = False

    def serve_index(self, entries, details, packages=None, issued_at=None):
        """Publish config.json + a signed index.json + package details/files. entries: index rows without detail_hash; details: {package_id: dict}; packages: {url path: bytes}."""
        rows = []
        for entry in entries:
            detail = json.dumps(details[entry["package_id"]], sort_keys=True).encode("utf-8")
            self.site.put("/packages/%s.json" % entry["package_id"], detail)
            rows.append({**entry, "detail_hash": blake3_hex(detail)})
        for path, data in (packages or {}).items():
            self.site.put(path if path.startswith("/") else "/" + path, data, "application/octet-stream")
        stamp = issued_at or time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        # index_key_id here is an unsigned hint; the host trusts only the key pinned in extension_sources.json.
        self.site.put("/config.json", json.dumps({"registry_protocol": 1, "index_key_id": INDEX_KEY_ID}))
        self.site.put("/index.json", json.dumps(signed_index(self.index_key, {"schema_version": 1, "issued_at": stamp, "entries": rows})))
        return self.index_url

    def publish_revocation(self, entries, at=None):
        """Replace the served list; issued_at is strictly newer each call (the host refuses a stale list)."""
        self.revocation_serial += 1
        stamp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime((at if at is not None else time.time()) + self.revocation_serial))
        full = [{"package_id": e["package_id"], "content_hash": e["content_hash"], "reason": e.get("reason", "fe-sim revocation"),
                 "revoked_at": e.get("revoked_at", stamp)} for e in entries]
        self.site.put("/revocation.json", json.dumps(signed_revocation(self.revocation_root, full, stamp)))
        self.revocation_entries = full
        return {"issued_at": stamp, "entries": len(full), "url": self.revocation_url, "signer_key_id": REVOCATION_KEY_ID}


def probe_port(port, timeout=0.5):
    """True when something accepts connections on the loopback port (a start_broker sanity check)."""
    with socket.socket() as sock:
        sock.settimeout(timeout)
        return sock.connect_ex((LOOPBACK_HOST, port)) == 0
