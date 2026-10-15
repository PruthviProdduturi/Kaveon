"""The retirement and its rollback must agree about what can be put back.

Three bounds contradicted each other across these modules. Two of them were
right for what they governed and said so nowhere, and one was simply the wrong
unit:

* The retirement validates reported row counts up to 10_000_000_000, while the
  exact-row restore refuses past 10_000 -- so the rollback precondition looked
  unsatisfiable exactly when a retirement had been large. It is not: these are
  generated families whose rollback is a proven deterministic rebuild. Nothing
  recorded which path a given run left available.
* The restore caps its own duration at 900s while a rollback control validates
  up to 3600s elsewhere, so a control could be valid in general and refused
  here with a message that named neither ceiling.
* `kaveondb_recovery_evidence` bounded a backup's OBJECT inventory by its
  RECORD bound, accepting a claim of a million objects when the backup can
  enumerate 10_000 -- a hundred times what the producer can emit.
"""

import sys
import unittest
from types import SimpleNamespace

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from services import kaveondb_recovery_evidence as evidence
from services import postgresql_special_family_restore as restore
from services import postgresql_special_family_retirement as retirement


class ThereIsOneCapacityNotTwo(unittest.TestCase):
    def test_the_restore_consumes_the_retirement_s_capacity(self):
        """One number in one place: the duplicate pair is what read as a
        contradiction, and either could have drifted from the other."""
        self.assertEqual(restore.MAX_ROWS, retirement.EXACT_ROW_ROLLBACK_CAPACITY)

    def test_the_path_follows_the_population(self):
        capacity = retirement.EXACT_ROW_ROLLBACK_CAPACITY
        self.assertEqual(retirement.rollback_path(0), "exact_row_restore")
        self.assertEqual(retirement.rollback_path(capacity), "exact_row_restore")
        self.assertEqual(retirement.rollback_path(capacity + 1), "deterministic_rebuild")
        self.assertEqual(retirement.rollback_path(10_000_000_000), "deterministic_rebuild")

    def test_an_inexact_total_is_refused(self):
        """The path is a claim about a population, so a guess at the population
        is not something to answer."""
        for bad in (None, -1, 1.5, "100"):
            with self.subTest(total=bad):
                with self.assertRaises(RuntimeError):
                    retirement.rollback_path(bad)


class TheObjectInventoryIsBoundedByItsProducer(unittest.TestCase):
    def test_objects_are_not_bounded_by_the_record_bound(self):
        from services import kaveondb_backup
        self.assertEqual(evidence.MAX_OBJECTS, kaveondb_backup.MAX_OBJECTS)
        self.assertLess(evidence.MAX_OBJECTS, evidence.MAX_RECORDS)

    def test_a_record_bound_stays_its_own_number(self):
        """A record is a product record and an object is a blob; one blob holds
        many records, so the two bounds are not meant to match."""
        self.assertEqual(evidence.MAX_RECORDS, 1_000_000)


class TheRestoreSaysWhichCeilingItHit(unittest.TestCase):
    def test_the_duration_refusal_names_both_ceilings(self):
        """A control valid against the 3600s general ceiling is refused here,
        and an operator mid-rollback needs to know that is by design."""
        self.assertEqual(restore.MAX_DURATION_SECONDS, 900)
        self.assertLess(restore.MAX_DURATION_SECONDS, 3600)

    def test_the_general_rollback_control_still_allows_an_hour(self):
        import inspect
        self.assertIn("3600", inspect.getsource(evidence))


if __name__ == "__main__":
    unittest.main()
