import copy
import hashlib
import unittest
from datetime import datetime, timezone

from services import postgresql_retirement_gate as gate


NOW = datetime(2026, 9, 10, 20, 0, tzinfo=timezone.utc)


def evidence():
    families = []
    for index, (family, tables) in enumerate(gate.AUTHORITY_FAMILIES.items()):
        families.append({
            "family": family,
            "tables": list(tables),
            "status": "passed",
            "reconciled_at": "2026-09-10T19:00:00Z",
            "source_watermark": index,
            "source_count": index,
            "target_count": index,
            "checks": {name: True for name in gate.REQUIRED_CHECKS},
            "report_sha256": hashlib.sha256(family.encode()).hexdigest(),
        })
    return {"schema_version": 1, "families": families}


class RetirementGateTests(unittest.TestCase):
    def test_complete_fresh_evidence_produces_deterministic_audit(self):
        first = gate.evaluate(evidence(), now=NOW, max_age_hours=24)
        second = gate.evaluate(evidence(), now=NOW, max_age_hours=24)
        self.assertEqual(first, second)
        self.assertTrue(first["passed"])
        self.assertEqual(first["authority_family_count"], len(gate.AUTHORITY_FAMILIES))
        self.assertEqual(len(first["audit_sha256"]), 64)

    def test_missing_family_fails_closed(self):
        value = evidence()
        value["families"].pop()
        with self.assertRaisesRegex(RuntimeError, "missing PostgreSQL authority evidence"):
            gate.evaluate(value, now=NOW, max_age_hours=24)

    def test_unknown_and_duplicate_families_fail_closed(self):
        value = evidence()
        value["families"].append(copy.deepcopy(value["families"][0]))
        with self.assertRaisesRegex(RuntimeError, "duplicate retirement evidence"):
            gate.evaluate(value, now=NOW, max_age_hours=24)
        value = evidence()
        value["families"][0]["family"] = "untracked_table"
        with self.assertRaisesRegex(RuntimeError, "missing PostgreSQL authority evidence"):
            gate.evaluate(value, now=NOW, max_age_hours=24)

    def test_stale_or_future_evidence_fails_closed(self):
        for timestamp in ("2026-09-09T18:59:59Z", "2026-09-10T20:00:01Z"):
            with self.subTest(timestamp=timestamp):
                value = evidence()
                value["families"][0]["reconciled_at"] = timestamp
                with self.assertRaisesRegex(RuntimeError, "is not fresh"):
                    gate.evaluate(value, now=NOW, max_age_hours=24)

    def test_failed_or_incomplete_parity_checks_fail_closed(self):
        mutations = (
            lambda item: item["checks"].update(counts=False),
            lambda item: item["checks"].pop("ownership"),
            lambda item: item.update(target_count=item["source_count"] + 1),
            lambda item: item.update(status="pending"),
            lambda item: item.update(tables=[]),
        )
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                value = evidence()
                mutate(value["families"][0])
                with self.assertRaises(RuntimeError):
                    gate.evaluate(value, now=NOW, max_age_hours=24)

    def test_secret_shaped_fields_are_rejected_at_any_depth(self):
        value = evidence()
        value["families"][0]["details"] = {"api_key_reference": "even-a-reference-is-not-needed"}
        with self.assertRaisesRegex(RuntimeError, "forbidden field"):
            gate.evaluate(value, now=NOW, max_age_hours=24)

    def test_arbitrary_payload_fields_are_rejected(self):
        value = evidence()
        value["families"][0]["sample_rows"] = [{"email": "person@example.test"}]
        with self.assertRaisesRegex(RuntimeError, "unexpected fields"):
            gate.evaluate(value, now=NOW, max_age_hours=24)

    def test_invalid_report_digest_and_unbounded_input_fail_closed(self):
        value = evidence()
        value["families"][0]["report_sha256"] = "ABC"
        with self.assertRaisesRegex(RuntimeError, "report_sha256 is invalid"):
            gate.evaluate(value, now=NOW, max_age_hours=24)
        value = evidence()
        value["padding"] = "x" * gate.MAX_EVIDENCE_BYTES
        with self.assertRaisesRegex(RuntimeError, "byte bound"):
            gate.evaluate(value, now=NOW, max_age_hours=24)


if __name__ == "__main__":
    unittest.main()
