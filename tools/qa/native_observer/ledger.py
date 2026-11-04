"""One durable process-incarnation attempt; see AGENTS.md."""

from .contract import (LEDGER_SCHEMA, MAX_LEDGER, digest, encode, fixture_name, hex_text, read_ledger, require)


class AttemptLedger:
    def __init__(self, file, bootstrap, expected_file_id):
        self.file, self.bootstrap = file, bootstrap
        actual = file.identity()
        require(actual["volume"] + actual["file_id"] == expected_file_id, "ledger_file_replaced")
        self.records = self.read()

    def read(self):
        size = self.file.size()
        require(0 < size <= MAX_LEDGER, "ledger_partial_or_oversize")
        return read_ledger(self.file.read(0, size), self.bootstrap)

    def reserve(self, operation, expected_name):
        require(hex_text(operation, 32), "reservation_operation_refused")
        require(len(self.read()) == 1, "attempt_already_spent")
        record = {"schema": LEDGER_SCHEMA, "kind": "reserved", "operation": operation,
                  "mode": "invoke_visible_petal_v1", "run_id": self.bootstrap["run_id"],
                  "pid": self.bootstrap["pid"], "birth_filetime": self.bootstrap["birth_filetime"],
                  "expected_name_sha256": digest(fixture_name(expected_name))}
        original = encode(self.bootstrap) + b"\n"
        reserved = original + encode(record) + b"\n"
        require(self.file.read(0, self.file.size()) == original, "ledger_changed_before_reservation")
        self.file.append_durable(encode(record) + b"\n", len(original), MAX_LEDGER)
        require(self.file.read(0, self.file.size()) == reserved, "ledger_reservation_unconfirmed")
        self.records = read_ledger(reserved, self.bootstrap)
        return digest(reserved)

    def spent(self):
        return len(self.read()) > 1
