"""Shared bounded inventory admission; see AGENTS.md."""

from pathlib import Path
import time

from .contract import decode, require


LIMITS_PATH = Path(__file__).with_name("limits.json")


def ordinary(path):
    return not path.is_symlink() and not getattr(path.lstat(), "st_file_attributes", 0) & 0x400


def limits():
    value = decode(LIMITS_PATH.read_bytes(), 1024)
    require(set(value) == {"schema", "jsonl_files", "incarnation_directories", "start_directories"}
            and value["schema"] == "fe-native-history-limits-v1"
            and all(type(value[key]) is int and 0 < value[key] <= 256
                    for key in ("jsonl_files", "incarnation_directories", "start_directories")), "native_history_limits_refused")
    return value


def admit(counts, additions, budget):
    for key in ("jsonl_files", "incarnation_directories", "start_directories"):
        require(type(counts[key]) is int and counts[key] >= 0
                and type(additions.get(key, 0)) is int and additions.get(key, 0) >= 0
                and counts[key] + additions.get(key, 0) <= budget[key], "native_history_capacity")


def inventory(directory, deadline):
    budget = limits()
    counts = {key: 0 for key in ("jsonl_files", "incarnation_directories", "start_directories")}
    for name in ("native-observer", "native-observer-observe", "starts"):
        root = directory / name
        if not root.exists():
            continue
        require(ordinary(root) and root.is_dir(), "native_history_alias_refused")
        for incarnation in root.iterdir():
            require(time.monotonic() < deadline, "operation_deadline")
            require(ordinary(incarnation) and incarnation.is_dir(),
                    "native_history_layout_refused")
            key = "start_directories" if name == "starts" else "incarnation_directories"
            counts[key] += 1
            admit(counts, {}, budget)
            if name == "starts":
                continue
            # Only the producer's fixed two-level layout is accepted.
            for child in incarnation.iterdir():
                require(time.monotonic() < deadline, "operation_deadline")
                require(ordinary(child), "native_history_alias_refused")
                if child.is_dir():
                    require(child.name == "workers", "native_history_layout_refused")
                    for receipt in child.iterdir():
                        require(time.monotonic() < deadline, "operation_deadline")
                        require(ordinary(receipt) and receipt.is_file()
                                and receipt.suffix == ".jsonl", "native_history_layout_refused")
                        counts["jsonl_files"] += 1
                        admit(counts, {}, budget)
                else:
                    require(child.name in {"attempt.jsonl", "observer.lock"} and child.is_file(), "native_history_layout_refused")
                    counts["jsonl_files"] += int(child.name == "attempt.jsonl")
                    admit(counts, {}, budget)
    return counts, budget


def admit_operation(directory, native_dir, deadline):
    counts, budget = inventory(directory, deadline)
    admit(counts, {"jsonl_files": 1, "incarnation_directories": int(not native_dir.exists())}, budget)
