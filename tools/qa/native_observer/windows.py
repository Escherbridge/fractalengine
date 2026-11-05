"""Pinned native observer files and exact worker lifetime; see AGENTS.md."""

import ctypes
from ctypes import wintypes as wt
import os
import subprocess
import sys
import threading
import time

from owned_stdout_windows import WindowsLease, PinnedFile, canonical
from .contract import MAX_OUTPUT, Refused, require


class NativeFile(PinnedFile):
    def append_durable(self, raw, expected_size, maximum):
        require(self.size() == expected_size and expected_size + len(raw) <= maximum, "durable_file_changed")
        require(self.lease.api.SetFilePointerEx(self.handle, expected_size, None, 0), "durable_seek_refused")
        self.write(raw)
        require(self.size() == expected_size + len(raw), "durable_write_unconfirmed")


class NativeLease(WindowsLease):
    def __init__(self):
        super().__init__()
        self.excluded = set()
        self.retained_unknown = False
        self.api.GetTickCount64.argtypes, self.api.GetTickCount64.restype = [], ctypes.c_ulonglong
        self.api.GetCurrentProcess.argtypes, self.api.GetCurrentProcess.restype = [], wt.HANDLE
        self.api.CreateDirectoryW.argtypes, self.api.CreateDirectoryW.restype = [wt.LPCWSTR, wt.LPVOID], wt.BOOL
        self.api.SetHandleInformation.argtypes = [wt.HANDLE, wt.DWORD, wt.DWORD]
        self.api.SetHandleInformation.restype = wt.BOOL

    def __exit__(self, *args):
        if not self.retained_unknown:
            super().__exit__(*args)

    def directory(self, path, create=False):
        path = canonical(path)
        for parent in reversed(path.parents):
            self._open(parent, directory=True)
        if create and not path.exists():
            require(self.api.CreateDirectoryW(str(path), None), "observer_directory_unavailable")
        return self._open(path, directory=True)

    def owned_file(self, path, *, exclusive=False, create=False, receipt=False):
        path = canonical(path)
        self.directory(path.parent)
        key = os.path.normcase(str(path))
        require(key not in self.files, "native_file_role_changed")
        access = 0xC0000000 if exclusive or create else 0x80000000
        share = 1 if receipt or not exclusive else 0
        handle = self.api.CreateFileW(str(path), access, share, None, 1 if create else 3, 0x00200000, None)
        require(handle not in (None, ctypes.c_void_p(-1).value), "native_file_or_lease_unavailable")
        self.handles.append(handle)
        file = NativeFile(self, handle)
        info = file.information()
        require(not info.attributes & (0x400 | 0x10) and info.links == 1, "native_file_alias_refused")
        buffer = ctypes.create_unicode_buffer(32768)
        length = self.api.GetFinalPathNameByHandleW(handle, buffer, len(buffer), 0)
        require(0 < length < len(buffer) and os.path.normcase(buffer.value.removeprefix("\\\\?\\")) == key,
                "native_file_path_changed")
        self.files[key], self.roles[key] = file, (False, "native")
        if receipt:
            self.excluded.add(int(handle))
        return file

    def pin(self, path):
        file = self.file(path)
        require(file.information().links == 1, "native_file_alias_refused")
        return file

    def process_identity(self, handle):
        times = [wt.FILETIME() for _ in range(4)]
        require(self.api.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)), "process_time_unavailable")
        buffer, size = ctypes.create_unicode_buffer(32768), wt.DWORD(32768)
        require(self.api.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size)), "process_image_unavailable")
        born = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
        return {"pid": int(self.api.GetProcessId(handle)), "start_ticks": str(born + 504911232000000000),
                "exe_path": str(canonical(buffer.value))}

    def same_interactive_user(self):
        require(len(self.processes) == 1, "one_owned_process_required")
        advapi = ctypes.WinDLL("advapi32", use_last_error=True)
        advapi.OpenProcessToken.argtypes = [wt.HANDLE, wt.DWORD, ctypes.POINTER(wt.HANDLE)]
        advapi.OpenProcessToken.restype = wt.BOOL
        advapi.GetTokenInformation.argtypes = [wt.HANDLE, ctypes.c_int, wt.LPVOID, wt.DWORD, ctypes.POINTER(wt.DWORD)]
        advapi.GetTokenInformation.restype = wt.BOOL
        advapi.EqualSid.argtypes, advapi.EqualSid.restype = [wt.LPVOID, wt.LPVOID], wt.BOOL
        advapi.GetSidSubAuthorityCount.argtypes = [wt.LPVOID]
        advapi.GetSidSubAuthorityCount.restype = ctypes.POINTER(ctypes.c_ubyte)
        advapi.GetSidSubAuthority.argtypes = [wt.LPVOID, wt.DWORD]
        advapi.GetSidSubAuthority.restype = ctypes.POINTER(wt.DWORD)
        tokens = []

        def information(token, kind):
            needed = wt.DWORD()
            advapi.GetTokenInformation(token, kind, None, 0, ctypes.byref(needed))
            require(0 < needed.value <= 8192, "interactive_token_refused")
            value = ctypes.create_string_buffer(needed.value)
            require(advapi.GetTokenInformation(token, kind, value, len(value), ctypes.byref(needed)), "interactive_token_refused")
            return value

        try:
            for process in (self.api.GetCurrentProcess(), self.processes[0]):
                token = wt.HANDLE()
                require(advapi.OpenProcessToken(process, 8, ctypes.byref(token)), "interactive_token_refused")
                tokens.append(token)
            users = [information(token, 1) for token in tokens]
            sids = [ctypes.cast(value, ctypes.POINTER(wt.LPVOID))[0] for value in users]
            require(advapi.EqualSid(*sids), "cross_user_refused")
            sessions = [ctypes.cast(information(token, 12), ctypes.POINTER(wt.DWORD))[0] for token in tokens]
            require(sessions[0] == sessions[1] and sessions[0] > 0, "interactive_session_refused")
            for token in tokens:
                label = information(token, 25)
                sid = ctypes.cast(label, ctypes.POINTER(wt.LPVOID))[0]
                count = advapi.GetSidSubAuthorityCount(sid)[0]
                require(count > 0 and advapi.GetSidSubAuthority(sid, count - 1)[0] == 0x2000,
                        "nonmedium_integrity_refused")
                require(ctypes.cast(information(token, 26), ctypes.POINTER(wt.DWORD))[0] == 0,
                        "uiaccess_refused")
        finally:
            for token in tokens:
                self.api.CloseHandle(token)

    def inherited(self):
        handles = [int(handle) for handle in self.handles if int(handle) not in self.excluded]
        require(len(handles) <= 128, "inherited_handle_cap")
        return handles


class WorkerResult:
    def __init__(self, process, identity, raw, exit_code, code):
        self.process, self.identity, self.raw = process, identity, raw
        self.exit_code, self.code = exit_code, code


# Keep owners alive when a caller embeds fe.main instead of exiting its process.
UNKNOWN_LIFETIMES = []
UNKNOWN_IO = []


def run_worker(lease, helper, payload, deadline, on_started):
    require(os.name == "nt" and sys.maxsize > 0xFFFFFFFF, "windows_x64_required")
    require(time.monotonic() < deadline, "operation_deadline")
    handles = lease.inherited()
    startup = subprocess.STARTUPINFO()
    startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 0
    startup.lpAttributeList = {"handle_list": handles}
    started, stopping = threading.Event(), threading.Event()
    start_state = {"process": None, "failed": False}

    def start():
        try:
            for handle in handles:
                require(lease.api.SetHandleInformation(handle, 1, 1), "handle_inheritance_refused")
            start_state["process"] = subprocess.Popen(
                [str(helper)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                close_fds=True, startupinfo=startup, creationflags=subprocess.CREATE_NO_WINDOW,
                cwd=str(helper.parent), env={"SystemRoot": os.environ.get("SystemRoot", "C:\\Windows")})
        except BaseException:
            start_state["failed"] = True
        finally:
            for handle in handles:
                if not lease.api.SetHandleInformation(handle, 1, 0):
                    start_state["failed"] = True
            started.set()
        if stopping.is_set() and start_state["process"] is not None:
            late = start_state["process"]
            try:
                late.stdin.close()
                late.kill()
                late.wait()
            except BaseException:
                pass

    start_thread = threading.Thread(target=start, daemon=True)
    start_thread.start()
    if not started.wait(timeout=max(0, deadline - 0.2 - time.monotonic())):
        stopping.set()
        lease.retained_unknown = True
        UNKNOWN_LIFETIMES.append((lease, start_state, start_thread))
        raise Refused("worker_start_lifetime_unknown")
    process = start_state["process"]
    if process is None:
        lease.retained_unknown = True
        UNKNOWN_LIFETIMES.append((lease, start_state, start_thread))
        raise Refused("worker_start_lifetime_unknown")
    identity = None
    failure = threading.Event()
    output = bytearray()

    def collect(pipe, maximum, destination=None):
        try:
            total = 0
            while True:
                chunk = pipe.read(min(4096, maximum - total + 1))
                if not chunk:
                    return
                total += len(chunk)
                if total > maximum:
                    failure.set()
                    return
                if destination is not None:
                    destination.extend(chunk)
        except (OSError, ValueError):
            failure.set()

    def send():
        try:
            process.stdin.write(payload)
            process.stdin.flush()
            process.stdin.close()
        except (OSError, ValueError):
            failure.set()

    threads = []
    deadline_stop_requested = False
    try:
        require(not start_state["failed"] and time.monotonic() < deadline - 0.2, "worker_start_refused")
        identity = lease.process_identity(process._handle)
        require(identity["pid"] == process.pid and os.path.normcase(identity["exe_path"]) == os.path.normcase(str(helper)),
                "worker_identity_changed")
        on_started(identity)
        for target, arguments in ((collect, (process.stdout, MAX_OUTPUT, output)),
                                  (collect, (process.stderr, 0)), (send, ())):
            thread = threading.Thread(target=target, args=arguments, daemon=True)
            thread.start()
            threads.append(thread)
        while process.poll() is None and not failure.is_set() and time.monotonic() < deadline - 0.2:
            failure.wait(min(0.01, max(0, deadline - 0.2 - time.monotonic())))
        if process.poll() is None:
            deadline_stop_requested = not failure.is_set()
            process.kill()
        remaining = max(0, deadline - time.monotonic())
        process.wait(timeout=remaining)
        for thread in threads:
            thread.join(timeout=max(0, deadline - time.monotonic()))
        code = ("worker_output_refused" if failure.is_set() or any(thread.is_alive() for thread in threads)
                else "worker_deadline_stop_requested" if deadline_stop_requested else "worker_exited")
        return WorkerResult(process, identity, bytes(output), process.returncode, code)
    except BaseException:
        try:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=max(0, deadline - time.monotonic()))
        except BaseException:
            lease.retained_unknown = True
            UNKNOWN_LIFETIMES.append((lease, process, identity))
            raise Refused("worker_lifetime_unknown") from None
        return WorkerResult(process, identity, b"", process.returncode, "worker_failed_after_start")
    finally:
        if process.poll() is not None and not any(thread.is_alive() for thread in threads):
            for pipe in (process.stdin, process.stdout, process.stderr):
                try:
                    pipe.close()
                except (OSError, ValueError):
                    pass
        elif process.poll() is not None:
            UNKNOWN_IO.append((process, threads))
