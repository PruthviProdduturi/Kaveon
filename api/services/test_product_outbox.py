import hashlib
import sys
import unittest
from types import SimpleNamespace

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import product_outbox


class FakeTransaction:
    def __init__(self, row=None):
        self.row = row
        self.calls = []

    def query_one(self, sql, params):
        self.calls.append((sql, params))
        return self.row or {
            "source_sequence": 17,
            "family": params[1],
            "operation": params[2],
            "record_id": params[3],
            "payload_sha256": params[5],
        }


class ProductOutboxTests(unittest.TestCase):
    def test_canonical_payload_and_stable_event_are_persisted(self):
        transaction = FakeTransaction()
        event_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        event = product_outbox.enqueue(
            transaction,
            family="datasets",
            operation="update",
            record_id="42",
            payload={"z": 2, "name": "Café"},
            actor="alice@example.com",
            event_id=event_id,
        )
        params = transaction.calls[0][1]
        canonical = '{"name":"Café","z":2}'
        self.assertEqual(params[4], canonical)
        self.assertEqual(params[5], hashlib.sha256(canonical.encode()).hexdigest())
        self.assertEqual((event.event_id, event.source_sequence), (event_id, 17))

    def test_same_event_id_with_different_digest_is_rejected(self):
        transaction = FakeTransaction({
            "source_sequence": 17,
            "family": "datasets",
            "operation": "update",
            "record_id": "42",
            "payload_sha256": "0" * 64,
        })
        with self.assertRaisesRegex(ValueError, "different request"):
            product_outbox.enqueue(
                transaction,
                family="datasets",
                operation="update",
                record_id="42",
                payload={"name": "changed"},
                actor="alice@example.com",
                event_id="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            )

    def test_unsupported_family_fails_before_database_write(self):
        transaction = FakeTransaction()
        with self.assertRaisesRegex(ValueError, "unsupported"):
            product_outbox.enqueue(
                transaction,
                family="query_history",
                operation="create",
                record_id="42",
                payload={},
                actor="alice@example.com",
            )
        self.assertEqual(transaction.calls, [])

    def test_oversized_payload_fails_before_database_write(self):
        transaction = FakeTransaction()
        with self.assertRaisesRegex(ValueError, "transaction limit"):
            product_outbox.enqueue(
                transaction,
                family="charts",
                operation="create",
                record_id="chart-1",
                payload={"thumbnail": "x" * (product_outbox.MAX_PAYLOAD_BYTES + 1)},
                actor="alice@example.com",
            )
        self.assertEqual(transaction.calls, [])


if __name__ == "__main__":
    unittest.main()
