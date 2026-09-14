import importlib.util
import unittest
from pathlib import Path


PATH = Path(__file__).with_name("retirement-qualification-summary.py")
SPEC = importlib.util.spec_from_file_location("retirement_summary", PATH)
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class RetirementSummaryTests(unittest.TestCase):
    def test_missing_evidence_is_pending_for_all_families_and_gates(self):
        result = module.summarize()
        self.assertFalse(result["passed"])
        self.assertEqual(result["authority_family_count"], len(module.AUTHORITY_FAMILIES))
        self.assertEqual(result["expected_authority_family_count"], len(module.AUTHORITY_FAMILIES))
        self.assertFalse(result["family_inventory_complete"])
        self.assertFalse(result["source"]["audit_validated"])
        self.assertEqual(result["global_gate_count"], 7)
        self.assertTrue(all(item["status"] == "pending" for item in result["authority_families"]))
        self.assertTrue(all(item["status"] == "pending" for item in result["global_gates"]))

    def test_passed_audit_still_requires_operational_rehearsals(self):
        audit = {
            "passed": True,
            "authority_family_count": len(module.AUTHORITY_FAMILIES),
            "families": [{"family": family, "status": "passed", "source_count": 1,
                          "target_count": 1} for family in module.AUTHORITY_FAMILIES],
            "gates": {name: {"status": "passed", "evidence_id": name}
                      for name in module.GLOBAL_GATE_NAMES},
        }
        result = module.summarize(audit)
        self.assertFalse(result["passed"])
        self.assertTrue(result["family_inventory_complete"])
        self.assertTrue(all(item["status"] == "passed" for item in result["authority_families"]))
        self.assertTrue(all(item["status"] == "passed" for item in result["global_gates"]))
        self.assertFalse(result["rehearsal_gates"]["backup_restore"].get("status") == "passed")

    def test_unvalidated_or_partial_audit_cannot_pass(self):
        operational = {
            name: {"status": "passed"}
            for name in ("backup_restore", "rollback", "postgresql_unavailable_restart", "durable_checkpoint")
        }
        complete = {
            "families": [{"family": family, "status": "passed"} for family in module.AUTHORITY_FAMILIES],
            "gates": {name: {"status": "passed"} for name in module.GLOBAL_GATE_NAMES},
            "authority_family_count": len(module.AUTHORITY_FAMILIES),
        }
        self.assertFalse(module.summarize(complete, operational)["passed"])
        complete["passed"] = True
        complete["families"].pop()
        self.assertFalse(module.summarize(complete, operational)["passed"])


if __name__ == "__main__":
    unittest.main()
