"""Pinned Win32 files and process lifetimes for owned QA logs; see AGENTS.md."""

import ctypes
from ctypes import wintypes as wt
import os
from pathlib import Path
import re


class FileInformation(ctypes.Structure):
    _fields_ = [("attributes", wt.DWORD), ("created", wt.FILETIME), ("accessed", wt.FILETIME),
                ("written", wt.FILETIME), ("volume", wt.DWORD), ("size_high", wt.DWORD),
                ("size_low", wt.DWORD), ("links", wt.DWORD), ("index_high", wt.DWORD),
                ("index_low", wt.DWORD)]


def canonical(path):
    path = Path(path)
    if (not path.is_absolute() or not re.fullmatch(r"[A-Za-z]:", path.drive)
            or ".." in path.parts):
        raise ValueError("owned logs require an absolute local drive path")
    for part in path.parts[1:]:
        if (":" in part or part.endswith((".", " "))
                or part.split(".")[0].upper() in {"CON", "PRN", "AUX", "NUL", "CLOCK$", *(
                    prefix + str(number) for prefix in ("COM", "LPT") for number in range(1, 10))}):
            raise ValueError("owned log path has a reserved or redirected component")
    return Path(os.path.normpath(path))


class PinnedFile:
    def __init__(self, lease, handle):
        self.lease, self.handle = lease, handle

    def information(self):
        info = FileInformation()
        if not self.lease.api.GetFileInformationByHandle(self.handle, ctypes.byref(info)):
            raise ValueError("could not inspect pinned log file")
        return info

    def identity(self):
        info = self.information()
        return {"volume": f"{info.volume:08x}", "file_id": f"{info.index_high:08x}{info.index_low:08x}"}

    def size(self):
        info = self.information()
        return (info.size_high << 32) | info.size_low

    def read(self, offset, length):
        if not self.lease.api.SetFilePointerEx(self.handle, offset, None, 0):
            raise ValueError("could not seek pinned log file")
        chunks = []
        while length:
            count = min(length, 65536)
            buffer, actual = ctypes.create_string_buffer(count), wt.DWORD()
            if not self.lease.api.ReadFile(self.handle, buffer, count, ctypes.byref(actual), None):
                raise ValueError("could not read pinned log file")
            if actual.value != count:
                raise ValueError("pinned log file was truncated")
            chunks.append(buffer.raw[:actual.value])
            length -= actual.value
        return b"".join(chunks)

    def write(self, data):
        actual, buffer = wt.DWORD(), ctypes.create_string_buffer(data)
        if (not self.lease.api.WriteFile(self.handle, buffer, len(data), ctypes.byref(actual), None)
                or actual.value != len(data) or not self.lease.api.FlushFileBuffers(self.handle)):
            raise ValueError("could not finish exclusive log receipt")


class WindowsLease:
    def __init__(self):
        if os.name != "nt":
            raise ValueError("owned stdout capture and verification require Windows")
        self.api = ctypes.WinDLL("kernel32", use_last_error=True)
        signatures = {
            "CreateFileW": ([wt.LPCWSTR, wt.DWORD, wt.DWORD, wt.LPVOID, wt.DWORD, wt.DWORD, wt.HANDLE], wt.HANDLE),
            "CloseHandle": ([wt.HANDLE], wt.BOOL),
            "GetFileInformationByHandle": ([wt.HANDLE, ctypes.POINTER(FileInformation)], wt.BOOL),
            "GetFinalPathNameByHandleW": ([wt.HANDLE, wt.LPWSTR, wt.DWORD, wt.DWORD], wt.DWORD),
            "SetFilePointerEx": ([wt.HANDLE, ctypes.c_longlong, wt.LPVOID, wt.DWORD], wt.BOOL),
            "ReadFile": ([wt.HANDLE, wt.LPVOID, wt.DWORD, ctypes.POINTER(wt.DWORD), wt.LPVOID], wt.BOOL),
            "WriteFile": ([wt.HANDLE, wt.LPCVOID, wt.DWORD, ctypes.POINTER(wt.DWORD), wt.LPVOID], wt.BOOL),
            "FlushFileBuffers": ([wt.HANDLE], wt.BOOL),
            "OpenProcess": ([wt.DWORD, wt.BOOL, wt.DWORD], wt.HANDLE),
            "WaitForSingleObject": ([wt.HANDLE, wt.DWORD], wt.DWORD),
            "QueryFullProcessImageNameW": ([wt.HANDLE, wt.DWORD, wt.LPWSTR, ctypes.POINTER(wt.DWORD)], wt.BOOL),
            "GetProcessTimes": ([wt.HANDLE, *([ctypes.POINTER(wt.FILETIME)] * 4)], wt.BOOL),
        }
        for name, (arguments, result) in signatures.items():
            function = getattr(self.api, name)
            function.argtypes, function.restype = arguments, result
        self.handles, self.files, self.processes = [], {}, []

    def __enter__(self):
        return self

    def __exit__(self, *_):
        for handle in reversed(self.handles):
            self.api.CloseHandle(handle)

    def _open(self, path, directory=False, create=False):
        path = canonical(path)
        key = os.path.normcase(str(path))
        if key in self.files:
            if create:
                raise ValueError("log artifact already exists")
            return self.files[key]
        access = 0xC0000000 if create else 0x80000000
        # Allow the existing log writer, but deny deletion/renaming while pinned.
        share = 1 if create else 3
        flags = 0x00200000 | (0x02000000 if directory else 0)
        handle = self.api.CreateFileW(str(path), access, share, None, 1 if create else 3, flags, None)
        if handle in (None, ctypes.c_void_p(-1).value):
            raise ValueError("could not exclusively pin owned path or create a new artifact")
        self.handles.append(handle)
        pinned = PinnedFile(self, handle)
        info = pinned.information()
        if info.attributes & 0x400 or bool(info.attributes & 0x10) != directory:
            raise ValueError("owned path is a reparse point or has the wrong type")
        buffer = ctypes.create_unicode_buffer(32768)
        length = self.api.GetFinalPathNameByHandleW(handle, buffer, len(buffer), 0)
        if not 0 < length < len(buffer):
            raise ValueError("could not resolve pinned owned path")
        final = buffer.value.removeprefix("\\\\?\\")
        if os.path.normcase(final) != key:
            raise ValueError("owned path resolved through an alias")
        self.files[key] = pinned
        return pinned

    def file(self, path, create=False):
        path = canonical(path)
        for parent in reversed(path.parents):
            self._open(parent, directory=True)
        return self._open(path, create=create)

    def live_process(self, identity):
        if (not isinstance(identity, dict) or type(identity.get("pid")) is not int or not 0 < identity["pid"] <= 0xFFFFFFFF
                or not isinstance(identity.get("start_ticks"), str) or not identity["start_ticks"].isdigit()):
            raise ValueError("missing exact owned process identity")
        executable = canonical(identity.get("exe_path", ""))
        self.file(executable)
        handle = self.api.OpenProcess(0x1000 | 0x100000, False, identity["pid"])
        if not handle:
            raise ValueError("exact owned app process is unavailable")
        self.handles.append(handle)
        self.processes.append(handle)
        self.check_live()
        buffer, length = ctypes.create_unicode_buffer(32768), wt.DWORD(32768)
        if not self.api.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(length)):
            raise ValueError("could not verify owned executable")
        times = [wt.FILETIME() for _ in range(4)]
        if not self.api.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
            raise ValueError("could not verify owned process creation time")
        creation = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
        if (os.path.normcase(buffer.value) != os.path.normcase(str(executable))
                or creation + 504911232000000000 != int(identity["start_ticks"])):
            raise ValueError("PID lifetime or executable no longer matches the owned app")

    def check_live(self):
        if any(self.api.WaitForSingleObject(handle, 0) != 258 for handle in self.processes):
            raise ValueError("owned app exited during log observation")
