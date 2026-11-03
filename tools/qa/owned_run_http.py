"""Pinned owned-run HTTP streams; see AGENTS.md section invitation transport."""

import ctypes
from ctypes import wintypes as wt
import http.client
import ipaddress
import json
from pathlib import Path
import socket
import struct
import urllib.parse

from owned_stdout_windows import WindowsLease, PinnedFile, canonical

MAX_TABLE_BYTES = 1024 * 1024
MAX_RESPONSE_BYTES = 16 * 1024 * 1024
MAX_TOKEN_BYTES = 1024 * 1024


class Tcp4Row(ctypes.Structure):
    _fields_ = [(name, wt.DWORD) for name in ("state", "local_address", "local_port", "remote_address", "remote_port", "pid")]


class Tcp6Row(ctypes.Structure):
    _fields_ = [("local_address", wt.BYTE * 16), ("local_scope", wt.DWORD), ("local_port", wt.DWORD),
                ("remote_address", wt.BYTE * 16), ("remote_scope", wt.DWORD), ("remote_port", wt.DWORD),
                ("state", wt.DWORD), ("pid", wt.DWORD)]


def normalized_address(value):
    address = ipaddress.ip_address(value)
    return str(address.ipv4_mapped or address) if address.version == 6 else str(address)


def server_tuple_owned(rows, client_local, server_peer, expected_pid):
    expected = (normalized_address(server_peer[0]), server_peer[1], normalized_address(client_local[0]), client_local[1])
    owners = {row[4] for row in rows if row[:4] == expected}
    return owners == {expected_pid}


class WindowsTcpOwners:
    def __init__(self):
        self.api = ctypes.WinDLL("iphlpapi", use_last_error=True)
        self.api.GetExtendedTcpTable.argtypes = [ctypes.c_void_p, ctypes.POINTER(wt.DWORD), wt.BOOL, wt.ULONG, ctypes.c_int, wt.ULONG]
        self.api.GetExtendedTcpTable.restype = wt.DWORD

    def rows(self):
        result = []
        for family, row_type in ((socket.AF_INET, Tcp4Row), (socket.AF_INET6, Tcp6Row)):
            size = wt.DWORD()
            code = self.api.GetExtendedTcpTable(None, ctypes.byref(size), False, family, 4, 0)
            if code not in (0, 122):
                raise ValueError("owned socket table is unavailable")
            for _ in range(3):
                if not 4 <= size.value <= MAX_TABLE_BYTES:
                    raise ValueError("owned socket table exceeds its fixed bound")
                buffer = ctypes.create_string_buffer(size.value)
                code = self.api.GetExtendedTcpTable(buffer, ctypes.byref(size), False, family, 4, 0)
                if code != 122:
                    break
            if code != 0:
                raise ValueError("owned socket table changed or is unavailable")
            count = struct.unpack_from("<I", buffer.raw)[0]
            if count > (size.value - 4) // ctypes.sizeof(row_type):
                raise ValueError("owned socket table is malformed")
            for index in range(count):
                row = row_type.from_buffer_copy(buffer, 4 + index * ctypes.sizeof(row_type))
                if row.state != 5:
                    continue
                if family == socket.AF_INET:
                    local = socket.inet_ntop(family, struct.pack("<I", row.local_address))
                    remote = socket.inet_ntop(family, struct.pack("<I", row.remote_address))
                else:
                    local = socket.inet_ntop(family, bytes(row.local_address))
                    remote = socket.inet_ntop(family, bytes(row.remote_address))
                    if row.local_scope or row.remote_scope:
                        continue
                result.append((normalized_address(local), socket.ntohs(row.local_port & 0xFFFF),
                               normalized_address(remote), socket.ntohs(row.remote_port & 0xFFFF), row.pid))
        return result


class PinnedConnection(http.client.HTTPConnection):
    def __init__(self, owner, timeout):
        super().__init__(owner.host, owner.port, timeout=timeout)
        self.owner = owner

    def connect(self):
        self.owner.check_live()
        super().connect()
        self.owner.prove_socket(self.sock)

    def send(self, data):
        if self.sock is None:
            self.connect()
        self.owner.prove_socket(self.sock)
        super().send(data)


def token_object(raw):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate token snapshot key")
            result[key] = value
        return result

    def reject_constant(_):
        raise ValueError("nonfinite token snapshot number")

    try:
        value = json.loads(raw.decode("utf-8-sig"), object_pairs_hook=unique_object,
                           parse_constant=reject_constant)
    except (ValueError, UnicodeError):
        raise ValueError("owned token snapshot is malformed") from None
    if not isinstance(value, dict) or not isinstance(value.get("tokens"), dict) or not isinstance(value.get("roles"), dict):
        raise ValueError("owned token snapshot is malformed")
    return value


def token_snapshot(files,path):
    # FILE_SHARE_READ | FILE_SHARE_DELETE retains one version while allowing atomic publication.
    handle=files.api.CreateFileW(str(path),0x80000000,5,None,3,0x00200000,None)
    if handle in (None,ctypes.c_void_p(-1).value):
        raise ValueError("owned token snapshot is unavailable")
    try:
        pinned=PinnedFile(files,handle)
        info=pinned.information()
        size = pinned.size()
        if info.attributes & (0x400|0x10) or not 0 < size <= MAX_TOKEN_BYTES:
            raise ValueError("owned token snapshot is redirected or exceeds its bound")
        resolved=ctypes.create_unicode_buffer(32768)
        length=files.api.GetFinalPathNameByHandleW(handle,resolved,len(resolved),0)
        if not 0 < length < len(resolved) or canonical(resolved.value.removeprefix("\\\\?\\")) != path:
            raise ValueError("owned token snapshot resolved through an alias")
        raw = pinned.read(0, size)
        if len(raw) != size:
            raise ValueError("owned token snapshot read was incomplete")
        return token_object(raw)
    finally:
        files.api.CloseHandle(handle)


class OwnedRun:
    def __init__(self, files, run):
        run_directory = canonical(run["run_dir"])
        record_file = files.file(run_directory / "current.json")
        if record_file.size() > 256 * 1024:
            raise ValueError("owned run metadata exceeds its bound")
        record = json.loads(record_file.read(0, record_file.size()).decode("utf-8-sig"))
        if not isinstance(record, dict) or canonical(record.get("run_dir", "")) != run_directory:
            raise ValueError("owned run metadata identifies another directory")
        identity = record.get("process_identity")
        if identity != run.get("process_identity") or record.get("api_base") != run.get("api_base"):
            raise ValueError("owned run metadata changed before pinning")
        files.live_process(identity)
        origin = urllib.parse.urlsplit(record["api_base"])
        try:
            host = ipaddress.ip_address(origin.hostname)
            port = origin.port
        except (ValueError, TypeError):
            raise ValueError("owned API requires a numeric loopback origin") from None
        if origin.scheme != "http" or not host.is_loopback or not port or origin.username or origin.password or origin.path not in ("", "/") or origin.query or origin.fragment:
            raise ValueError("owned API requires a canonical numeric loopback origin")
        tokens = canonical(record.get("tokens_path") or run_directory / "tokens.json")
        if tokens != run_directory / "tokens.json":
            raise ValueError("owned tokens must use the exact run-local token file")
        self.tokens=token_snapshot(files,tokens)
        self.files, self.identity = files, dict(identity)
        self.run_directory, self.host, self.port = run_directory, str(host), port
        self.run = {**record, "run_dir": str(run_directory), "tokens_path": str(tokens)}
        self.owners = WindowsTcpOwners()

    def check_live(self):
        self.files.check_live()

    def prove_socket(self, connection):
        self.check_live()
        if not server_tuple_owned(self.owners.rows(), connection.getsockname(), connection.getpeername(), self.identity["pid"]):
            raise ValueError("the retained API socket does not belong to the exact owned app")
        self.check_live()

    def request(self, method, path, token=None, body=None, timeout=40.0, join_input_lease=None):
        if method not in ("GET", "POST") or not path.startswith("/api/v1/") or path.startswith("//") or any(c in path for c in "\r\n"):
            raise ValueError("owned HTTP requires a fixed relative API request")
        data = None if body is None else json.dumps(body).encode("utf-8")
        headers = {"Connection": "close"}
        if join_input_lease is not None:
            if not isinstance(join_input_lease,str) or len(join_input_lease)>64 or not join_input_lease.startswith("join-input-") or not all(c.isascii() and (c.isalnum() or c=="-") for c in join_input_lease):
                raise ValueError("invalid guarded Join constraint")
            headers["X-FE-Join-Lease"]=join_input_lease
        if token:
            headers["Authorization"] = "Bearer " + token
        if data is not None:
            headers["Content-Type"] = "application/json"
        connection = PinnedConnection(self, timeout)
        try:
            connection.connect()
            connection.request(method, path, body=data, headers=headers)
            response = connection.getresponse()
            raw = response.read(MAX_RESPONSE_BYTES + 1)
            if len(raw) > MAX_RESPONSE_BYTES:
                raise ValueError("owned API response exceeds its fixed bound")
            self.check_live()
            return response.status, dict(response.headers), raw
        finally:
            connection.close()


class OwnedRunTransport:
    def __init__(self, runs):
        self.files = WindowsLease()
        self.original_runs = runs
        self.runs = []

    def __enter__(self):
        self.files.__enter__()
        try:
            self.runs = [OwnedRun(self.files, run) for run in self.original_runs]
            if len({run.identity["pid"] for run in self.runs}) != len(self.runs):
                raise ValueError("owned runs must identify distinct exact app lifetimes")
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *args):
        self.files.__exit__(*args)

    def check_live(self):
        self.files.check_live()

    def __call__(self, run, method, path, token=None, body=None, timeout=40.0, join_input_lease=None):
        directory = canonical(run["run_dir"])
        matches = [owned for owned in self.runs if owned.run_directory == directory]
        if len(matches) != 1:
            raise ValueError("request does not identify one pinned owned run")
        return matches[0].request(method, path, token, body, timeout, join_input_lease)
