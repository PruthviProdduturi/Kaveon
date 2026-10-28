"""An unbounded SELECT * must not be able to read a whole table.

`SELECT * FROM events.daily_user_activity` is a billion rows. What stopped it
was the coordinator's result disk quota, after a quarter of a gigabyte had
been written, for someone who wanted to see the shape of the table.

The cap is announced, not silent. A cap the caller is not told about returns a
different answer than the one asked for, which is the fault `TOP` had when it
parsed and was discarded.
"""

import sys
import unittest
from types import SimpleNamespace

if "pyodbc" not in sys.modules:
    sys.modules["pyodbc"] = SimpleNamespace(Error=Exception)

from routers.lab import LAB_DEFAULT_ROW_CAP, default_row_cap


class OnlyABareUnboundedStarIsCapped(unittest.TestCase):
    def test_a_bare_star_is_capped(self):
        sql, applied = default_row_cap("SELECT * FROM events.daily_user_activity")
        self.assertTrue(applied)
        self.assertTrue(sql.endswith(f"LIMIT {LAB_DEFAULT_ROW_CAP}"))

    def test_a_qualified_star_is_capped(self):
        sql, applied = default_row_cap("SELECT e.* FROM events e")
        self.assertTrue(applied)
        self.assertTrue(sql.endswith(f"LIMIT {LAB_DEFAULT_ROW_CAP}"))

    def test_distinct_and_odd_spacing_are_still_recognised(self):
        sql, applied = default_row_cap("select  distinct  *  from  t")
        self.assertTrue(applied)
        self.assertTrue(sql.endswith(f"LIMIT {LAB_DEFAULT_ROW_CAP}"))

    def test_a_statement_that_bounds_itself_is_left_alone(self):
        """Taken at its word, in all three spellings the Engine accepts."""
        for statement in ("SELECT * FROM t LIMIT 5",
                          "SELECT TOP 1 * FROM t",
                          "SELECT * FROM t FETCH FIRST 10 ROWS ONLY"):
            with self.subTest(statement=statement):
                sql, applied = default_row_cap(statement)
                self.assertFalse(applied, statement)
                self.assertEqual(sql, statement)

    def test_naming_columns_is_taken_as_meaning_it(self):
        """The guard is against the accidental whole table, not a policy about
        result size -- a caller who names columns has said what they want."""
        for statement in ("SELECT user_id FROM t",
                          "SELECT COUNT(*) FROM t",
                          "SELECT surface, SUM(actions) FROM t GROUP BY surface"):
            with self.subTest(statement=statement):
                _, applied = default_row_cap(statement)
                self.assertFalse(applied, statement)

    def test_several_statements_are_not_this_guards_business(self):
        sql = "SELECT * FROM a; SELECT * FROM b"
        self.assertEqual(default_row_cap(sql), (sql, False))

    def test_a_trailing_semicolon_does_not_defeat_the_cap(self):
        """The statement the architect typed ended in one."""
        sql, applied = default_row_cap("SELECT * FROM events.daily_user_activity;")
        self.assertTrue(applied)
        self.assertTrue(sql.endswith(f"LIMIT {LAB_DEFAULT_ROW_CAP}"))

    def test_empty_input_is_returned_unchanged(self):
        self.assertEqual(default_row_cap(""), ("", False))


if __name__ == "__main__":
    unittest.main()
