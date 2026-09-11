import hashlib
import json
import unittest
from unittest.mock import call, patch

from fastapi import HTTPException

from services import product_replay, product_store


def event(operation="create", sequence=1, document=None):
    document = document or {"id": "42", "name": "Orders"}
    payload = json.dumps(document, sort_keys=True, separators=(",", ":"))
    return {
        "source_sequence": sequence,
        "event_id": f"00000000-0000-4000-8000-{sequence:012d}",
        "family": "datasets",
        "operation": operation,
        "record_id": "42",
        "payload_json": payload,
        "payload_sha256": hashlib.sha256(payload.encode()).hexdigest(),
        "actor_principal": "editor@example.com",
        "owner_principal": "owner@example.com",
    }


class ProductReplayTests(unittest.TestCase):
    def test_create_uses_owner_identity_and_returns_committed_generation(self):
        source = event()
        with patch.object(product_replay.product_store, "read", return_value=None) as read, \
             patch.object(product_replay.product_store, "transact", return_value={"generation": 9}) as transact:
            self.assertEqual(product_replay.apply_event(source), 9)
        read.assert_called_once_with("dataset", "42", "owner@example.com", "Admin")
        mutation = transact.call_args.args[0][0]
        self.assertEqual(mutation, product_store.ProductMutation(
            "create", "dataset", "42", {"id": "42", "name": "Orders"}
        ))
        self.assertEqual(transact.call_args.args[1:], ("owner@example.com", "Admin"))

    def test_lost_update_response_resolves_only_from_exact_committed_document(self):
        source = event("update")
        before = {"revision": 2, "generation": 4, "document": {"id": "42", "name": "Old"}}
        after = {"revision": 3, "generation": 5, "document": {"id": "42", "name": "Orders"}}
        with patch.object(product_replay.product_store, "read", side_effect=[before, after]), \
             patch.object(product_replay.product_store, "transact", side_effect=HTTPException(409, "conflict")):
            self.assertEqual(product_replay.apply_event(source), 5)

    def test_conflict_with_different_target_content_fails_closed(self):
        source = event("update")
        target = {"revision": 2, "generation": 4, "document": {"id": "42", "name": "Other"}}
        with patch.object(product_replay.product_store, "read", side_effect=[target, target]), \
             patch.object(product_replay.product_store, "transact", side_effect=HTTPException(409, "conflict")):
            with self.assertRaisesRegex(RuntimeError, "did not resolve"):
                product_replay.apply_event(source)

    def test_tampered_payload_fails_before_target_access(self):
        source = event()
        source["payload_json"] = '{"name":"tampered"}'
        with patch.object(product_replay.product_store, "read") as read:
            with self.assertRaisesRegex(RuntimeError, "hash mismatch"):
                product_replay.apply_event(source)
        read.assert_not_called()

    def test_replay_acknowledges_in_order_and_stops_at_first_failure(self):
        events = [event(sequence=1), event(sequence=2), event(sequence=3)]
        with patch.object(product_replay.product_outbox, "pending", return_value=events), \
             patch.object(product_replay, "apply_event", side_effect=[7, RuntimeError("blocked")]) as apply, \
             patch.object(product_replay.product_outbox, "mark_applied") as mark, \
             patch.object(product_replay.product_outbox, "record_failure") as failure:
            with self.assertRaisesRegex(RuntimeError, "blocked"):
                product_replay.replay_pending()
        self.assertEqual(apply.call_count, 2)
        mark.assert_called_once_with(events[0]["event_id"], events[0]["payload_sha256"], 7)
        failure.assert_called_once_with(
            events[1]["event_id"], events[1]["payload_sha256"], "reconciliation_failed"
        )

    def test_already_deleted_target_is_an_idempotent_success(self):
        source = event("delete", document={"id": "42", "deleted": True})
        with patch.object(product_replay.product_store, "read", return_value=None), \
             patch.object(product_replay.product_store, "transact") as transact:
            self.assertIsNone(product_replay.apply_event(source))
        transact.assert_not_called()


if __name__ == "__main__":
    unittest.main()
