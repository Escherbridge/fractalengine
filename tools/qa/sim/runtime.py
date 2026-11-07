"""Shared run state: one process clock, JSONL recorder, live observation store, cadence overrides; see tools/qa/AGENTS.md §sim."""
import json
import os
import threading
import time

from predicates import normalize


class Clock:
    """One perf_counter clock for driver, prober and analyzer; wall time is derived from one anchor pair."""

    def __init__(self):
        self.perf0 = time.perf_counter()
        self.wall0 = time.time()

    def now(self):
        return time.perf_counter()

    def wall(self, perf):
        return self.wall0 + (perf - self.perf0)

    def sleep_until(self, target):
        """Sleep to a perf_counter instant; spin the last 2 ms so barrier fires land within ~1 ms."""
        while True:
            remaining = target - time.perf_counter()
            if remaining <= 0:
                return
            if remaining > 0.003:
                time.sleep(remaining - 0.002)


class Recorder:
    """Append-only JSONL streams under the run dir (events, observations, expectations, steps)."""

    STREAMS = ("events", "observations", "expectations", "steps")

    def __init__(self, run_dir):
        self.run_dir = run_dir
        self._locks = {name: threading.Lock() for name in self.STREAMS}
        self._files = {name: open(os.path.join(run_dir, name + ".jsonl"), "a", encoding="utf-8")
                       for name in self.STREAMS}

    def write(self, stream, record):
        line = json.dumps(record, separators=(",", ":"), default=str)
        with self._locks[stream]:
            self._files[stream].write(line + "\n")
            self._files[stream].flush()

    def close(self):
        for name, handle in self._files.items():
            with self._locks[name]:
                handle.flush()
                handle.close()


class LiveStore:
    """Latest observation per (peer, source probe, target key) for the driver's early-settle check."""

    def __init__(self):
        self._lock = threading.Lock()
        self._latest = {}
        self._ticks = {}

    def update(self, peer, probe, key, t, value):
        with self._lock:
            self._latest[(peer, probe, key)] = (t, value)

    def tick(self, peer, t):
        with self._lock:
            self._ticks[peer] = t

    def get(self, peer, probe, key):
        with self._lock:
            return self._latest.get((peer, probe, key))

    def last_tick(self, peer):
        with self._lock:
            return self._ticks.get(peer)


class Cadence:
    """Scenario cadence with per-step overrides (0 pauses a probe family)."""

    def __init__(self, base):
        self._base = dict(base)
        self._override = {}
        self._step_index = -1
        self._lock = threading.Lock()

    def set_override(self, override):
        with self._lock:
            self._override = dict(override or {})

    def set_step(self, index):
        """Probers poll only targets of steps that have started (plus the scenario watch list)."""
        with self._lock:
            self._step_index = index

    def step_index(self):
        with self._lock:
            return self._step_index

    def get(self, key):
        with self._lock:
            return self._override.get(key, self._base[key])

    def as_dict(self):
        with self._lock:
            return {**self._base, **self._override}


def small(value, limit=4096):
    """value if its canonical JSON fits the observation budget, else None (digest still recorded)."""
    text = json.dumps(normalize(value), sort_keys=True, default=str)
    return normalize(value) if len(text) <= limit else None
